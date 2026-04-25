//! Phase 7 integration tests — session persistence and resume.

use std::sync::Arc;

use amplifier_module_context_simple::SimpleContext;
use amplifier_module_orchestrator_loop_streaming::{HookRegistry, LoopConfig, LoopOrchestrator};
use amplifier_module_session_store::{
    FileSessionStore, SessionEvent, SessionMetadata, SessionStore,
};
use amplifier_module_tool_task::SubagentRunner;

// ---------------------------------------------------------------------------
// Minimal scripted mock provider
// ---------------------------------------------------------------------------

mod mock {
    use std::collections::HashMap;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Mutex;

    use amplifier_core::errors::ProviderError;
    use amplifier_core::messages::{ChatRequest, ChatResponse, ContentBlock, ToolCall};
    use amplifier_core::models::{ModelInfo, ProviderInfo};
    use amplifier_core::traits::Provider;

    pub struct ScriptedProvider {
        pub replies: Mutex<Vec<String>>,
    }
    impl ScriptedProvider {
        pub fn new(replies: Vec<&'static str>) -> Self {
            Self {
                replies: Mutex::new(replies.into_iter().map(String::from).collect()),
            }
        }
    }
    impl Provider for ScriptedProvider {
        fn name(&self) -> &str {
            "mock"
        }
        fn get_info(&self) -> ProviderInfo {
            ProviderInfo {
                id: "mock".to_string(),
                display_name: "mock".to_string(),
                credential_env_vars: vec![],
                capabilities: vec![],
                defaults: HashMap::new(),
                config_fields: vec![],
            }
        }
        fn list_models(
            &self,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<ModelInfo>, ProviderError>> + Send + '_>>
        {
            Box::pin(async { Ok(vec![]) })
        }
        fn complete(
            &self,
            _r: ChatRequest,
        ) -> Pin<Box<dyn Future<Output = Result<ChatResponse, ProviderError>> + Send + '_>>
        {
            let next = self.replies.lock().unwrap().remove(0);
            Box::pin(async move {
                Ok(ChatResponse {
                    content: vec![ContentBlock::Text {
                        text: next,
                        visibility: None,
                        extensions: HashMap::new(),
                    }],
                    tool_calls: None,
                    usage: None,
                    degradation: None,
                    finish_reason: Some("end_turn".into()),
                    metadata: None,
                    extensions: HashMap::new(),
                })
            })
        }
        fn parse_tool_calls(&self, _r: &ChatResponse) -> Vec<ToolCall> {
            vec![]
        }
    }
}

// ---------------------------------------------------------------------------
// crash_resume_end_to_end
// ---------------------------------------------------------------------------

#[tokio::test]
async fn crash_resume_end_to_end() {
    let tmp = tempfile::TempDir::new().unwrap();
    let store: Arc<dyn SessionStore> =
        Arc::new(FileSessionStore::new_with_root(tmp.path().to_path_buf()));
    let session_id = "explorer-1".to_string();

    // ---- Phase A: first process instance ----
    {
        let orch = LoopOrchestrator::new(LoopConfig::default());
        orch.attach_store(
            store.clone(),
            session_id.clone(),
            "explorer".into(),
            None,
        );
        let provider =
            Arc::new(mock::ScriptedProvider::new(vec!["found: a.rs, b.rs, c.rs"]));
        orch.register_provider(
            "mock".into(),
            provider as Arc<dyn amplifier_core::traits::Provider>,
        )
        .await;

        store
            .begin(
                &session_id,
                SessionMetadata {
                    session_id: session_id.clone(),
                    agent_name: "explorer".into(),
                    parent_id: None,
                    created: chrono::Utc::now().to_rfc3339(),
                    status: "active".into(),
                },
            )
            .await
            .unwrap();

        let mut ctx = SimpleContext::new(vec![]);
        let _ = orch
            .execute(
                "list all Rust files".into(),
                &mut ctx,
                &HookRegistry::new(),
                |_| {},
            )
            .await
            .unwrap();
        orch.finish_store("success").await.unwrap();
    }

    // ---- Verify on-disk state ----
    let body = std::fs::read_to_string(tmp.path().join(&session_id).join("events.jsonl"))
        .unwrap();
    for line in body.lines() {
        let _: SessionEvent =
            serde_json::from_str(line).expect("each line must be valid JSON");
    }
    assert!(
        body.contains("list all Rust files"),
        "user turn must be persisted"
    );
    assert!(body.contains("found: a.rs"), "assistant turn must be persisted");
    let idx = std::fs::read_to_string(tmp.path().join("index.jsonl")).unwrap();
    assert!(
        idx.contains("\"status\":\"success\""),
        "index must be updated to success"
    );

    // ---- Phase B: second process instance (resume) ----
    let orch2 = LoopOrchestrator::new(LoopConfig::default());
    orch2.attach_store(store.clone(), session_id.clone(), "explorer".into(), None);
    let provider2 = Arc::new(mock::ScriptedProvider::new(vec!["3 files total"]));
    orch2
        .register_provider(
            "mock".into(),
            provider2 as Arc<dyn amplifier_core::traits::Provider>,
        )
        .await;

    let result =
        SubagentRunner::resume(&orch2, &session_id, "now count them".into())
            .await
            .unwrap();
    assert_eq!(result.session_id, session_id);
    assert!(
        result.response.contains("3 files total"),
        "resume response must come from new provider"
    );

    let final_events = store.load(&session_id).await.unwrap();
    let user_msgs: Vec<_> = final_events
        .iter()
        .filter_map(|e| {
            if let SessionEvent::Turn { role, content, .. } = e {
                if role == "user" {
                    Some(content.as_str())
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect();
    assert!(user_msgs.iter().any(|m| m.contains("list all Rust files")));
    assert!(user_msgs.iter().any(|m| m.contains("now count them")));
}

// ---------------------------------------------------------------------------
// three_concurrent_sessions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn three_concurrent_sessions_dont_corrupt_each_other() {
    let tmp = tempfile::TempDir::new().unwrap();
    let store: Arc<dyn SessionStore> =
        Arc::new(FileSessionStore::new_with_root(tmp.path().to_path_buf()));

    let mut handles = vec![];
    for i in 0..3usize {
        let store = store.clone();
        let sid = format!("sess-{i}");
        handles.push(tokio::spawn(async move {
            store
                .begin(
                    &sid,
                    SessionMetadata {
                        session_id: sid.clone(),
                        agent_name: format!("agent-{i}"),
                        parent_id: None,
                        created: chrono::Utc::now().to_rfc3339(),
                        status: "active".into(),
                    },
                )
                .await
                .unwrap();
            for n in 0..20usize {
                store
                    .append(
                        &sid,
                        SessionEvent::Turn {
                            role: "user".into(),
                            content: format!("msg {i}-{n}"),
                            timestamp: chrono::Utc::now().to_rfc3339(),
                        },
                    )
                    .await
                    .unwrap();
            }
            store.finish(&sid, "success", 20).await.unwrap();
        }));
    }
    for h in handles {
        h.await.unwrap();
    }

    for i in 0..3usize {
        let sid = format!("sess-{i}");
        let events = store.load(&sid).await.unwrap();
        // session_start + 20 turns + session_end = 22
        assert_eq!(
            events.len(),
            22,
            "session {sid} must have 22 events, got {}",
            events.len()
        );
        // No cross-contamination
        for evt in &events[1..21] {
            if let SessionEvent::Turn { content, .. } = evt {
                assert!(
                    content.starts_with(&format!("msg {i}-")),
                    "session {sid} contaminated: {content}"
                );
            }
        }
    }

    let metas = store.list().await.unwrap();
    assert_eq!(metas.len(), 3);
    for m in &metas {
        assert_eq!(m.status, "success");
    }
}

// ---------------------------------------------------------------------------
// malformed_jsonl
// ---------------------------------------------------------------------------

#[tokio::test]
async fn malformed_events_jsonl_returns_clear_error() {
    let tmp = tempfile::TempDir::new().unwrap();
    let store: Arc<dyn SessionStore> =
        Arc::new(FileSessionStore::new_with_root(tmp.path().to_path_buf()));
    std::fs::create_dir_all(tmp.path().join("broken")).unwrap();
    std::fs::write(
        tmp.path().join("broken/events.jsonl"),
        "{\"type\":\"session_start\",\"session_id\":\"broken\",\"agent_name\":\"a\",\"timestamp\":\"t\"}\nnot json\n",
    )
    .unwrap();
    let err = store.load("broken").await.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("malformed") && msg.contains("line 2"),
        "expected line-2 error, got: {msg}"
    );
}
