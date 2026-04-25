//! Context building stub — builds inherited context for sub-agent delegation.

use serde_json::Value;

use amplifier_module_tool_task::{ContextDepth, ContextScope};

/// Build inherited context from messages for a sub-agent.
///
/// # Arguments
/// * `_messages` — Source messages to slice context from.
/// * `_depth` — How much context to include.
/// * `_turns` — Maximum turns when depth is `Recent`.
/// * `_scope` — Which categories to include.
///
/// Returns `None` when context is empty or `ContextDepth::None`.
pub fn build_inherited_context(
    _messages: &[Value],
    _depth: &ContextDepth,
    _turns: usize,
    _scope: &ContextScope,
) -> Option<String> {
    None
}
