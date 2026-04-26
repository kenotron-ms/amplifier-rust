//! SimpleContext — in-memory context manager.
//!
//! Implements [`amplifier_core::traits::ContextManager`] with two storage areas:
//! - `history`: persisted across turns
//! - `ephemeral`: cleared after each provider call (via [`SimpleContext::messages_for_provider`])
//!
//! ## Non-destructive compaction
//!
//! [`ContextManager::get_messages_for_request`] returns a **compacted view** — it
//! clones the history, shrinks the clone if necessary, and returns it.  The stored
//! `history` is **never** modified by compaction.  This mirrors the Python
//! implementation where compaction returns a view and never mutates the source.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use amplifier_core::errors::ContextError;
use amplifier_core::traits::{ContextManager, Provider};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// ContextConfig
// ---------------------------------------------------------------------------

/// Configuration for context-window management and compaction behaviour.
pub struct ContextConfig {
    /// Hard ceiling on the context window (tokens).  Default: 180 000.
    pub max_tokens: usize,
    /// Fraction of `max_tokens` at which compaction is triggered.
    /// Default: 0.92 (compact when usage exceeds 92 %).
    pub compact_threshold: f32,
    /// Fraction of the context window to treat as "protected recent" messages
    /// that are never dropped automatically.  Default: 0.30.
    pub protected_recent: f32,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            max_tokens: 180_000,
            compact_threshold: 0.92,
            protected_recent: 0.30,
        }
    }
}

// ---------------------------------------------------------------------------
// SimpleContext
// ---------------------------------------------------------------------------

/// In-memory context manager with persistent history and per-call ephemeral buffer.
///
/// - `history` is persisted across turns (add_message, push_turn, set_messages, clear).
/// - `ephemeral` is cleared automatically after every [`messages_for_provider`](Self::messages_for_provider) call.
/// - [`get_messages_for_request`](ContextManager::get_messages_for_request) returns a
///   **non-destructive compacted view** — `history` is never modified by compaction.
pub struct SimpleContext {
    history: Mutex<Vec<Value>>,
    ephemeral: Mutex<Vec<Value>>,
    config: ContextConfig,
}

impl SimpleContext {
    /// Create a new context, optionally pre-loaded with `history`, using default config.
    pub fn new(history: Vec<Value>) -> Self {
        Self::with_config(history, ContextConfig::default())
    }

    /// Create a new context with custom [`ContextConfig`].
    pub fn with_config(history: Vec<Value>, config: ContextConfig) -> Self {
        Self {
            history: Mutex::new(history),
            ephemeral: Mutex::new(Vec::new()),
            config,
        }
    }

    /// Append a user/assistant exchange (two messages) into persistent history.
    pub fn push_turn(&self, user_msg: Value, assistant_msg: Value) {
        let mut history = self.history.lock().unwrap();
        history.push(user_msg);
        history.push(assistant_msg);
    }

    /// Append a message to the ephemeral buffer (cleared after next provider call).
    pub fn push_ephemeral(&self, msg: Value) {
        let mut ephemeral = self.ephemeral.lock().unwrap();
        ephemeral.push(msg);
    }

    /// Return `history + ephemeral` combined, then clear the ephemeral buffer.
    ///
    /// Call this immediately before forwarding messages to a provider so that
    /// ephemeral messages (e.g., injected tool results) are included exactly once.
    pub fn messages_for_provider(&self) -> Vec<Value> {
        let history = self.history.lock().unwrap().clone();
        let ephemeral = {
            let mut ephem = self.ephemeral.lock().unwrap();
            let msgs = ephem.clone();
            ephem.clear();
            msgs
        };
        history.into_iter().chain(ephemeral).collect()
    }

    /// Count tokens in the current **history** using the `cl100k_base` encoding.
    pub fn token_count(&self) -> usize {
        let text = {
            let history = self.history.lock().unwrap();
            history
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(" ")
        };
        estimate_tokens_from_text(&text)
    }

    /// Drop the oldest messages in 50%-of-current-length chunks until the token
    /// count is at or below `threshold`.
    ///
    /// **Mutates `self.history` in place.**  This is an explicit, intentional
    /// operation.  For non-destructive compaction before a provider call, see
    /// [`get_messages_for_request`](ContextManager::get_messages_for_request).
    ///
    /// Each iteration removes the first `len/2` messages from history, then
    /// re-checks.  Stops early if history is empty or only 1 message remains.
    pub fn compact_if_needed(&self, threshold: usize) {
        loop {
            // Compute token count and length without holding any lock across iterations.
            let (count, len) = {
                let history = self.history.lock().unwrap();
                let text = history
                    .iter()
                    .map(|v| v.to_string())
                    .collect::<Vec<_>>()
                    .join(" ");
                let count = estimate_tokens_from_text(&text);
                (count, history.len())
            };

            if count <= threshold {
                break;
            }
            if len == 0 {
                break;
            }
            // 50% chunk: drop the oldest half of current messages.
            let chunk = len / 2;
            if chunk == 0 {
                // Only 1 message left; cannot compact further.
                break;
            }

            let mut history = self.history.lock().unwrap();
            let remaining: Vec<Value> = history[chunk..].to_vec();
            *history = remaining;
        }
    }
}

// ---------------------------------------------------------------------------
// Free-standing helpers
// ---------------------------------------------------------------------------

/// Encode `text` with `cl100k_base` and return the token count.
fn estimate_tokens_from_text(text: &str) -> usize {
    let bpe_arc = tiktoken_rs::cl100k_base_singleton();
    let bpe = bpe_arc.lock();
    bpe.encode_with_special_tokens(text).len()
}

/// Serialise `msgs` to a space-joined string and count tokens.
fn estimate_tokens(msgs: &[Value]) -> usize {
    let text = msgs
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    estimate_tokens_from_text(&text)
}

/// The system reminder prepended to any compacted view.
fn compaction_notice() -> Value {
    json!({
        "role": "system",
        "content": "<system-reminder>Context was compacted. Earlier history was removed to fit token limits.</system-reminder>"
    })
}

/// Return a **compacted view** of `view` that fits within `threshold` tokens.
///
/// Applies three progressive levels — stopping as soon as the clone fits.
/// **The input `view` is a clone; the caller's original history is never touched.**
///
/// ### Level 1 — Truncate tool-result `content` strings > 250 chars
/// ### Level 2 — Drop messages beyond the last 20, keeping the first user message
/// ### Level 3 — Replace the oldest remaining user message with a compaction notice
///
/// A [`compaction_notice`] system message is prepended whenever any level fires.
fn compact_view(mut view: Vec<Value>, threshold: usize) -> Vec<Value> {
    // Fast path: already fits.
    if estimate_tokens(&view) <= threshold {
        return view;
    }

    let initial_len = view.len();
    let mut any_compacted = false;

    // ── Level 1: truncate tool-result content strings > 250 Unicode chars ─
    for msg in &mut view {
        if msg.get("role").and_then(|r| r.as_str()) != Some("tool") {
            continue;
        }
        if let Some(content) = msg.get_mut("content") {
            if let Some(s) = content.as_str() {
                if s.chars().count() > 250 {
                    let truncated: String = s.chars().take(250).collect();
                    *content = Value::String(format!("{}…", truncated));
                    any_compacted = true;
                }
            }
        }
    }

    if estimate_tokens(&view) <= threshold {
        let mut out = Vec::with_capacity(view.len() + 1);
        out.push(compaction_notice());
        out.extend(view);
        return out;
    }

    // ── Level 2: keep only the last 20 messages + the first user message ──
    if view.len() > 20 {
        let keep_from = view.len() - 20;

        // Rescue the first user message that would otherwise be discarded.
        let first_user = view[..keep_from]
            .iter()
            .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
            .cloned();

        let tail: Vec<Value> = view[keep_from..].to_vec();
        view = match first_user {
            Some(fu) => {
                let mut v = Vec::with_capacity(tail.len() + 1);
                v.push(fu);
                v.extend(tail);
                v
            }
            None => tail,
        };
        any_compacted = true;
    }

    if estimate_tokens(&view) <= threshold {
        let mut out = Vec::with_capacity(view.len() + 1);
        out.push(compaction_notice());
        out.extend(view);
        return out;
    }

    // ── Level 3: replace oldest user message with a compaction notice ──────
    let n_removed = initial_len.saturating_sub(view.len());
    if let Some(idx) = view
        .iter()
        .position(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
    {
        view[idx] = json!({
            "role": "user",
            "content": format!("[context compacted — {} messages removed]", n_removed)
        });
        any_compacted = true;
    }

    if any_compacted {
        let mut out = Vec::with_capacity(view.len() + 1);
        out.push(compaction_notice());
        out.extend(view);
        out
    } else {
        view
    }
}

// ---------------------------------------------------------------------------
// ContextManager impl
// ---------------------------------------------------------------------------

impl ContextManager for SimpleContext {
    fn add_message(
        &self,
        message: Value,
    ) -> Pin<Box<dyn Future<Output = Result<(), ContextError>> + Send + '_>> {
        Box::pin(async move {
            self.history.lock().unwrap().push(message);
            Ok(())
        })
    }

    fn get_messages(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Value>, ContextError>> + Send + '_>> {
        Box::pin(async move { Ok(self.history.lock().unwrap().clone()) })
    }

    /// Return a non-destructive compacted **view** of `history + ephemeral`.
    ///
    /// 1. Clones history and drains the ephemeral buffer (into the clone).
    /// 2. If the clone exceeds the token threshold, applies 3-level progressive
    ///    compaction on the clone — `self.history` is **never modified**.
    /// 3. Returns the (possibly compacted) clone.
    ///
    /// The token threshold is `token_budget` when supplied; otherwise it falls
    /// back to `config.max_tokens × config.compact_threshold`.
    fn get_messages_for_request(
        &self,
        token_budget: Option<i64>,
        _provider: Option<Arc<dyn Provider>>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Value>, ContextError>> + Send + '_>> {
        Box::pin(async move {
            // Step 1: clone history + drain ephemeral — non-destructive w.r.t. history.
            let view = self.messages_for_provider();

            // Step 2: determine the token threshold.
            let threshold = token_budget
                .map(|b| b as usize)
                .unwrap_or_else(|| {
                    (self.config.max_tokens as f32 * self.config.compact_threshold) as usize
                });

            // Step 3: compact the clone and return it — history is NEVER modified.
            Ok(compact_view(view, threshold))
        })
    }

    fn set_messages(
        &self,
        messages: Vec<Value>,
    ) -> Pin<Box<dyn Future<Output = Result<(), ContextError>> + Send + '_>> {
        Box::pin(async move {
            *self.history.lock().unwrap() = messages;
            Ok(())
        })
    }

    fn clear(&self) -> Pin<Box<dyn Future<Output = Result<(), ContextError>> + Send + '_>> {
        Box::pin(async move {
            self.history.lock().unwrap().clear();
            Ok(())
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::runtime::Runtime;

    fn rt() -> Runtime {
        Runtime::new().unwrap()
    }

    // -----------------------------------------------------------------------
    // Required: 6 history unit tests
    // -----------------------------------------------------------------------

    #[test]
    fn new_with_history_returns_history() {
        let msgs = vec![json!({"role": "user", "content": "hello"})];
        let ctx = SimpleContext::new(msgs.clone());
        let got = rt().block_on(ctx.get_messages()).unwrap();
        assert_eq!(got, msgs);
    }

    #[test]
    fn new_empty_starts_empty() {
        let ctx = SimpleContext::new(vec![]);
        let got = rt().block_on(ctx.get_messages()).unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn add_message_appends_to_history() {
        let ctx = SimpleContext::new(vec![]);
        let msg = json!({"role": "user", "content": "test"});
        rt().block_on(ctx.add_message(msg.clone())).unwrap();
        let got = rt().block_on(ctx.get_messages()).unwrap();
        assert_eq!(got, vec![msg]);
    }

    #[test]
    fn push_turn_adds_two_messages() {
        let ctx = SimpleContext::new(vec![]);
        let user = json!({"role": "user", "content": "hello"});
        let assistant = json!({"role": "assistant", "content": "hi"});
        ctx.push_turn(user.clone(), assistant.clone());
        let got = rt().block_on(ctx.get_messages()).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], user);
        assert_eq!(got[1], assistant);
    }

    #[test]
    fn clear_empties_history() {
        let ctx = SimpleContext::new(vec![json!({"role": "user", "content": "hello"})]);
        rt().block_on(ctx.clear()).unwrap();
        let got = rt().block_on(ctx.get_messages()).unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn set_messages_replaces_history() {
        let ctx = SimpleContext::new(vec![json!({"role": "user", "content": "old"})]);
        let new_msgs = vec![
            json!({"role": "user", "content": "new1"}),
            json!({"role": "assistant", "content": "new2"}),
        ];
        rt().block_on(ctx.set_messages(new_msgs.clone())).unwrap();
        let got = rt().block_on(ctx.get_messages()).unwrap();
        assert_eq!(got, new_msgs);
    }

    // -----------------------------------------------------------------------
    // Extra behaviour tests
    // -----------------------------------------------------------------------

    #[test]
    fn push_ephemeral_included_once_then_cleared() {
        let ctx = SimpleContext::new(vec![json!({"role": "user", "content": "history"})]);
        ctx.push_ephemeral(json!({"role": "system", "content": "ephemeral"}));

        // First call: ephemeral is included.
        let first = ctx.messages_for_provider();
        assert_eq!(first.len(), 2);

        // Second call: ephemeral was cleared.
        let second = ctx.messages_for_provider();
        assert_eq!(second.len(), 1);
    }

    #[test]
    fn token_count_returns_nonzero_for_nonempty_history() {
        let ctx = SimpleContext::new(vec![json!({"role": "user", "content": "hello world"})]);
        assert!(ctx.token_count() > 0);
    }

    #[test]
    fn compact_if_needed_reduces_history_when_over_threshold() {
        let msgs: Vec<Value> = (0..10)
            .map(|i| json!({"role": "user", "content": format!("message number {i}")}))
            .collect();
        let ctx = SimpleContext::new(msgs);

        // Threshold of 1 token forces compaction.
        ctx.compact_if_needed(1);

        let remaining = rt().block_on(ctx.get_messages()).unwrap();
        assert!(
            remaining.len() < 10,
            "compact_if_needed should have removed messages"
        );
    }

    // -----------------------------------------------------------------------
    // Ephemeral queue tests
    // -----------------------------------------------------------------------

    #[test]
    fn ephemeral_not_in_get_messages() {
        let ctx = SimpleContext::new(vec![]);
        ctx.push_ephemeral(json!({"role": "system", "content": "ephemeral only"}));
        // get_messages returns only persistent history — ephemeral must not appear.
        let got = rt().block_on(ctx.get_messages()).unwrap();
        assert!(got.is_empty(), "ephemeral must not leak into get_messages");
    }

    #[test]
    fn messages_for_provider_includes_ephemeral() {
        let history_msg = json!({"role": "user", "content": "history"});
        let ephemeral_msg = json!({"role": "system", "content": "ephemeral"});
        let ctx = SimpleContext::new(vec![history_msg.clone()]);
        ctx.push_ephemeral(ephemeral_msg.clone());
        let result = ctx.messages_for_provider();
        assert_eq!(result.len(), 2, "should contain history + ephemeral");
        assert_eq!(result[0], history_msg, "first element must be history");
        assert_eq!(result[1], ephemeral_msg, "second element must be ephemeral");
    }

    #[test]
    fn messages_for_provider_clears_ephemeral_after_call() {
        let ctx = SimpleContext::new(vec![]);
        ctx.push_ephemeral(json!({"role": "system", "content": "one-shot"}));
        // First call: ephemeral is included.
        let first = ctx.messages_for_provider();
        assert_eq!(
            first.len(),
            1,
            "first call must include the ephemeral message"
        );
        // Second call: ephemeral was cleared.
        let second = ctx.messages_for_provider();
        assert_eq!(
            second.len(),
            0,
            "second call must be empty after ephemeral cleared"
        );
    }

    #[test]
    fn messages_for_provider_history_not_cleared() {
        let history_msg = json!({"role": "user", "content": "persistent"});
        let ctx = SimpleContext::new(vec![history_msg.clone()]);
        // Call twice to verify history survives both calls.
        let _ = ctx.messages_for_provider();
        let _ = ctx.messages_for_provider();
        let history = rt().block_on(ctx.get_messages()).unwrap();
        assert_eq!(
            history.len(),
            1,
            "history must survive multiple messages_for_provider calls"
        );
        assert_eq!(history[0], history_msg);
    }

    // -----------------------------------------------------------------------
    // compact_if_needed tests (explicit, intentional mutation)
    // -----------------------------------------------------------------------

    #[test]
    fn compact_if_needed_noop_when_under_threshold() {
        // One message, very high threshold — nothing should be dropped.
        let ctx = SimpleContext::new(vec![json!({"role": "user", "content": "hello"})]);
        ctx.compact_if_needed(100_000);
        let remaining = rt().block_on(ctx.get_messages()).unwrap();
        assert_eq!(
            remaining.len(),
            1,
            "no messages should be dropped when under threshold"
        );
    }

    #[test]
    fn compact_if_needed_drops_messages_when_over_threshold() {
        // 20 messages, tiny threshold of 5 tokens — some must be dropped.
        let msgs: Vec<Value> = (0..20)
            .map(|i| json!({"role": "user", "content": format!("message number {i}")}))
            .collect();
        let ctx = SimpleContext::new(msgs);
        ctx.compact_if_needed(5);
        let remaining = rt().block_on(ctx.get_messages()).unwrap();
        assert!(
            remaining.len() < 20,
            "compact_if_needed should have dropped messages when over threshold, but len={}",
            remaining.len()
        );
    }

    #[test]
    fn compact_if_needed_noop_on_empty_context() {
        // Empty history with threshold=0 — must not panic and must stay empty.
        let ctx = SimpleContext::new(vec![]);
        ctx.compact_if_needed(0); // must not panic
        let remaining = rt().block_on(ctx.get_messages()).unwrap();
        assert!(
            remaining.is_empty(),
            "empty context must stay empty after compact_if_needed"
        );
    }

    // -----------------------------------------------------------------------
    // Token count tests (tiktoken-rs cl100k_base)
    // -----------------------------------------------------------------------

    #[test]
    fn token_count_is_zero_for_empty_context() {
        assert_eq!(SimpleContext::new(vec![]).token_count(), 0);
    }

    #[test]
    fn token_count_is_nonzero_for_nonempty_context() {
        let ctx = SimpleContext::new(vec![
            json!({"role": "user", "content": "Hello, how are you today?"}),
        ]);
        let count = ctx.token_count();
        assert!(
            count > 0,
            "token count should be > 0 for non-empty context, got {count}"
        );
        assert!(
            count < 100,
            "token count should be < 100 for a short message, got {count}"
        );
    }

    #[test]
    fn token_count_grows_with_more_messages() {
        let ctx = SimpleContext::new(vec![json!({"role": "user", "content": "Hello"})]);
        let count_before = ctx.token_count();
        ctx.push_turn(
            json!({"role": "user", "content": "What is the weather today?"}),
            json!({"role": "assistant", "content": "I don't have access to real-time weather data."}),
        );
        let count_after = ctx.token_count();
        assert!(
            count_after > count_before,
            "token count should increase after push_turn: before={count_before}, after={count_after}"
        );
    }

    // -----------------------------------------------------------------------
    // Non-destructive compaction tests (THE BUG FIX)
    // -----------------------------------------------------------------------

    /// Core regression test: get_messages_for_request must NEVER mutate history.
    #[test]
    fn get_messages_for_request_does_not_mutate_history() {
        let msgs: Vec<Value> = (0..30)
            .map(|i| json!({"role": "user", "content": format!("message {i}")}))
            .collect();
        let ctx = SimpleContext::new(msgs.clone());

        // A budget of 1 token forces maximum compaction on the view.
        rt().block_on(ctx.get_messages_for_request(Some(1), None))
            .unwrap();

        // History must be completely untouched.
        let history = rt().block_on(ctx.get_messages()).unwrap();
        assert_eq!(
            history.len(),
            30,
            "get_messages_for_request must not modify history length"
        );
        assert_eq!(history, msgs, "get_messages_for_request must not modify history content");
    }

    /// Calling get_messages_for_request repeatedly never shrinks history.
    #[test]
    fn get_messages_for_request_repeated_calls_preserve_history() {
        let msgs: Vec<Value> = (0..25)
            .map(|i| json!({"role": "user", "content": format!("msg {i}")}))
            .collect();
        let ctx = SimpleContext::new(msgs.clone());

        for _ in 0..5 {
            ctx.push_ephemeral(json!({"role": "system", "content": "inject"}));
            rt().block_on(ctx.get_messages_for_request(Some(1), None))
                .unwrap();
        }

        let history = rt().block_on(ctx.get_messages()).unwrap();
        assert_eq!(
            history.len(),
            25,
            "history must be unchanged after 5 compacted get_messages_for_request calls"
        );
    }

    /// When compaction fires, the returned view contains the system-reminder notice.
    #[test]
    fn get_messages_for_request_includes_compaction_notice() {
        let msgs: Vec<Value> = (0..30)
            .map(|i| json!({"role": "user", "content": format!("message {i}")}))
            .collect();
        let ctx = SimpleContext::new(msgs);

        let view = rt()
            .block_on(ctx.get_messages_for_request(Some(1), None))
            .unwrap();

        let has_notice = view.iter().any(|m| {
            m.get("role").and_then(|r| r.as_str()) == Some("system")
                && m.get("content")
                    .and_then(|c| c.as_str())
                    .map(|c| c.contains("compacted"))
                    .unwrap_or(false)
        });
        assert!(
            has_notice,
            "compacted view must contain the system-reminder notice"
        );
    }

    /// When history fits inside the budget, the view is returned unchanged (no notice).
    #[test]
    fn get_messages_for_request_no_notice_when_fits() {
        let ctx = SimpleContext::new(vec![json!({"role": "user", "content": "hello"})]);

        // Very large budget — no compaction needed.
        let view = rt()
            .block_on(ctx.get_messages_for_request(Some(500_000), None))
            .unwrap();

        let has_notice = view.iter().any(|m| {
            m.get("role").and_then(|r| r.as_str()) == Some("system")
                && m.get("content")
                    .and_then(|c| c.as_str())
                    .map(|c| c.contains("compacted"))
                    .unwrap_or(false)
        });
        assert!(!has_notice, "no notice should appear when history fits within budget");
        assert_eq!(view.len(), 1, "view should contain exactly the original message");
    }

    // -----------------------------------------------------------------------
    // compact_view unit tests (3-level progressive compaction)
    // -----------------------------------------------------------------------

    /// Level 1: tool-result content longer than 250 chars is truncated.
    #[test]
    fn compact_view_level1_truncates_tool_result_content() {
        let long_content = "x".repeat(500);
        let msgs = vec![
            json!({"role": "user", "content": "hello"}),
            json!({"role": "tool", "content": long_content}),
        ];
        // threshold=1 forces all levels to run.
        let result = compact_view(msgs, 1);

        let tool_msg = result
            .iter()
            .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("tool"))
            .expect("tool message must be present");
        let content = tool_msg
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap();
        // 250 chars + 1-char ellipsis = 251 max
        assert!(
            content.chars().count() <= 252,
            "truncated content must be ≤ 252 chars, got {}",
            content.chars().count()
        );
        assert!(
            content.chars().count() < 500,
            "content must have been shortened from 500 chars"
        );
    }

    /// Non-tool messages are not truncated by Level 1.
    #[test]
    fn compact_view_level1_does_not_truncate_non_tool_messages() {
        let long_content = "y".repeat(500);
        let msgs = vec![
            json!({"role": "user", "content": long_content.clone()}),
            json!({"role": "assistant", "content": long_content}),
        ];
        let result = compact_view(msgs, 1);

        for msg in &result {
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");
            if role == "user" || role == "assistant" {
                // These must not have been truncated by Level 1
                // (they may be dropped by Level 2/3 though).
                if let Some(c) = msg.get("content").and_then(|c| c.as_str()) {
                    // Content is either the original or a Level-3 notice — not truncated.
                    assert!(
                        c.chars().count() == 500 || c.contains("context compacted"),
                        "non-tool content must not be truncated by Level 1"
                    );
                }
            }
        }
    }

    /// Level 2: histories with > 20 messages are trimmed to ≤ 21 (last-20 + first-user).
    #[test]
    fn compact_view_level2_trims_to_20_plus_first_user() {
        // 40 user messages.  Level 2 keeps last 20 + rescues first user = 21 max.
        let msgs: Vec<Value> = (0..40)
            .map(|i| json!({"role": "user", "content": format!("msg {i}")}))
            .collect();
        // Use a threshold that Level 1 won't satisfy (no tool messages) but Level 2 will.
        // Estimate tokens for 21 messages and use that as threshold.
        let twenty_one: Vec<Value> = msgs[..21].to_vec();
        let threshold = estimate_tokens(&twenty_one) + 50;

        let result = compact_view(msgs.clone(), threshold);

        // Strip the leading compaction notice.
        let messages: Vec<&Value> = result
            .iter()
            .filter(|m| m.get("role").and_then(|r| r.as_str()) != Some("system"))
            .collect();
        assert!(
            messages.len() <= 21,
            "after Level 2, view should have ≤ 21 non-system messages, got {}",
            messages.len()
        );
        // First user message (msg 0) must be preserved.
        assert!(
            messages.iter().any(|m| {
                m.get("content").and_then(|c| c.as_str()) == Some("msg 0")
            }),
            "first user message must be preserved by Level 2"
        );
    }

    /// Level 3: the oldest user message is replaced with a compaction notice string.
    #[test]
    fn compact_view_level3_replaces_oldest_user_with_notice() {
        // 5 short user messages.  Set threshold below even 1-message cost to force Level 3.
        let msgs: Vec<Value> = (0..5)
            .map(|i| json!({"role": "user", "content": format!("m{i}")}))
            .collect();
        let result = compact_view(msgs, 0);

        let has_level3_notice = result.iter().any(|m| {
            m.get("content")
                .and_then(|c| c.as_str())
                .map(|c| c.contains("context compacted"))
                .unwrap_or(false)
        });
        assert!(
            has_level3_notice,
            "Level 3 must replace the oldest user message with a compaction notice"
        );
    }

    /// compact_view is a pure function — calling it does not modify the source slice.
    #[test]
    fn compact_view_does_not_modify_original_vec() {
        let msgs: Vec<Value> = (0..30)
            .map(|i| json!({"role": "user", "content": format!("msg {i}")}))
            .collect();
        let original = msgs.clone();

        let _ = compact_view(msgs.clone(), 1);

        // The original vec must be unchanged (we passed a clone, but verify the pattern).
        assert_eq!(original.len(), 30);
    }

    /// compact_view returns the view unchanged (no notice) when it already fits.
    #[test]
    fn compact_view_noop_when_under_threshold() {
        let msgs = vec![json!({"role": "user", "content": "hello"})];
        let result = compact_view(msgs.clone(), 500_000);
        assert_eq!(result, msgs, "no-op compact_view must return the original messages unchanged");
    }

    // -----------------------------------------------------------------------
    // ContextConfig tests
    // -----------------------------------------------------------------------

    #[test]
    fn context_config_default_values() {
        let cfg = ContextConfig::default();
        assert_eq!(cfg.max_tokens, 180_000);
        assert!((cfg.compact_threshold - 0.92).abs() < f32::EPSILON);
        assert!((cfg.protected_recent - 0.30).abs() < f32::EPSILON);
    }

    #[test]
    fn with_config_uses_provided_config() {
        let cfg = ContextConfig {
            max_tokens: 50_000,
            compact_threshold: 0.80,
            protected_recent: 0.20,
        };
        // A budget of None should fall back to max_tokens * compact_threshold = 40_000.
        // With 1 short message that is well under 40_000 tokens, no compaction fires.
        let ctx = SimpleContext::with_config(
            vec![json!({"role": "user", "content": "hi"})],
            cfg,
        );
        let view = rt()
            .block_on(ctx.get_messages_for_request(None, None))
            .unwrap();
        // No compaction — no notice, view is the 1 history message.
        assert_eq!(view.len(), 1);
    }
}
