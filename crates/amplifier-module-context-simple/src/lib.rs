//! SimpleContext — in-memory context manager.
//!
//! Implements [`amplifier_core::traits::ContextManager`] with two storage areas:
//! - `history`: persisted across turns
//! - `ephemeral`: cleared after each provider call (via [`SimpleContext::messages_for_provider`])

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use amplifier_core::errors::ContextError;
use amplifier_core::traits::{ContextManager, Provider};
use serde_json::Value;

// ---------------------------------------------------------------------------
// SimpleContext
// ---------------------------------------------------------------------------

/// In-memory context manager with persistent history and per-call ephemeral buffer.
///
/// - `history` is persisted across turns (add_message, push_turn, set_messages, clear).
/// - `ephemeral` is cleared automatically after every [`messages_for_provider`](Self::messages_for_provider) call.
pub struct SimpleContext {
    history: Mutex<Vec<Value>>,
    ephemeral: Mutex<Vec<Value>>,
}

impl SimpleContext {
    /// Create a new context, optionally pre-loaded with `history`.
    pub fn new(history: Vec<Value>) -> Self {
        Self {
            history: Mutex::new(history),
            ephemeral: Mutex::new(Vec::new()),
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
        let bpe_arc = tiktoken_rs::cl100k_base_singleton();
        let bpe = bpe_arc.lock();
        bpe.encode_with_special_tokens(&text).len()
    }

    /// Drop the oldest messages in 50%-of-current-length chunks until the token
    /// count is at or below `threshold`.
    ///
    /// Each iteration removes the first `len/2` messages from history, then
    /// re-checks. Stops early if history is empty or only 1 message remains.
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
                let bpe_arc = tiktoken_rs::cl100k_base_singleton();
                let bpe = bpe_arc.lock();
                let count = bpe.encode_with_special_tokens(&text).len();
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

    fn get_messages_for_request(
        &self,
        token_budget: Option<i64>,
        _provider: Option<Arc<dyn Provider>>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Value>, ContextError>> + Send + '_>> {
        Box::pin(async move {
            if let Some(budget) = token_budget {
                self.compact_if_needed(budget as usize);
            }
            Ok(self.messages_for_provider())
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
        assert_eq!(first.len(), 1, "first call must include the ephemeral message");
        // Second call: ephemeral was cleared.
        let second = ctx.messages_for_provider();
        assert_eq!(second.len(), 0, "second call must be empty after ephemeral cleared");
    }

    #[test]
    fn messages_for_provider_history_not_cleared() {
        let history_msg = json!({"role": "user", "content": "persistent"});
        let ctx = SimpleContext::new(vec![history_msg.clone()]);
        // Call twice to verify history survives both calls.
        let _ = ctx.messages_for_provider();
        let _ = ctx.messages_for_provider();
        let history = rt().block_on(ctx.get_messages()).unwrap();
        assert_eq!(history.len(), 1, "history must survive multiple messages_for_provider calls");
        assert_eq!(history[0], history_msg);
    }

    // -----------------------------------------------------------------------
    // compact_if_needed tests
    // -----------------------------------------------------------------------

    #[test]
    fn compact_if_needed_noop_when_under_threshold() {
        // One message, very high threshold — nothing should be dropped.
        let ctx = SimpleContext::new(vec![json!({"role": "user", "content": "hello"})]);
        ctx.compact_if_needed(100_000);
        let remaining = rt().block_on(ctx.get_messages()).unwrap();
        assert_eq!(remaining.len(), 1, "no messages should be dropped when under threshold");
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
        assert!(remaining.is_empty(), "empty context must stay empty after compact_if_needed");
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
        assert!(count > 0, "token count should be > 0 for non-empty context, got {count}");
        assert!(count < 100, "token count should be < 100 for a short message, got {count}");
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
}
