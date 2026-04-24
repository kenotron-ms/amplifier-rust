//! Todo tool module — provides the `todo` tool for managing an in-memory
//! session-scoped todo list.
//!
//! This module defines [`TodoTool`], which implements the
//! `amplifier_core::traits::Tool` interface.  The todo list is stored in an
//! in-memory `Mutex<Vec<TodoItem>>` with no disk persistence.  IDs are
//! generated via UUID v4 on every `create` or `update` call.
//!
//! # Actions
//!
//! - `create` — Replace all todos with the provided list (IDs regenerated).
//! - `update` — Identical to `create`; replaces all todos.
//! - `list`   — Return the current todo list without modification.

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use amplifier_core::errors::ToolError;
use amplifier_core::messages::ToolSpec;
use amplifier_core::models::ToolResult;
use amplifier_core::traits::Tool;

// ---------------------------------------------------------------------------
// TodoStatus
// ---------------------------------------------------------------------------

/// Canonical status values for todo items.
///
/// Used for display/documentation purposes; the [`TodoItem`] struct stores
/// status as a plain `String` so callers may pass arbitrary values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

impl fmt::Display for TodoStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TodoStatus::Pending => write!(f, "pending"),
            TodoStatus::InProgress => write!(f, "in_progress"),
            TodoStatus::Completed => write!(f, "completed"),
        }
    }
}

// ---------------------------------------------------------------------------
// TodoItem
// ---------------------------------------------------------------------------

/// A single todo item stored in the session-scoped todo list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    /// Unique identifier (UUID v4) generated at creation time.
    pub id: String,
    /// Imperative description of the task.
    pub content: String,
    /// Present-continuous form (e.g., "Running tests").
    pub active_form: String,
    /// Current status string (e.g., "pending", "in_progress", "completed").
    pub status: String,
}

// ---------------------------------------------------------------------------
// TodoTool
// ---------------------------------------------------------------------------

/// Tool that manages a session-scoped in-memory todo list.
///
/// The list is stored in a [`Mutex`]-protected `Vec<TodoItem>`.  All
/// mutations happen under the lock within synchronous code, so the lock is
/// never held across an `.await` point.
pub struct TodoTool {
    items: Mutex<Vec<TodoItem>>,
}

impl Default for TodoTool {
    fn default() -> Self {
        Self {
            items: Mutex::new(Vec::new()),
        }
    }
}

impl TodoTool {
    /// Serialise the current item list into the standard tool output shape.
    ///
    /// ```json
    /// { "count": 2, "todos": [{ "id": "...", "content": "...", ... }] }
    /// ```
    fn to_output(items: &[TodoItem]) -> Value {
        json!({
            "count": items.len(),
            "todos": items.iter().map(|item| json!({
                "id":          item.id,
                "content":     item.content,
                "active_form": item.active_form,
                "status":      item.status,
            })).collect::<Vec<_>>()
        })
    }

    /// Synchronous execution core — called from the async `execute` wrapper.
    ///
    /// Matches `action`:
    /// - `"create"` / `"update"` — replace all items with newly generated UUIDs.
    /// - `"list"` — return current items unchanged.
    /// - anything else — return [`ToolError::Other`].
    fn do_execute(&self, input: &Value) -> Result<ToolResult, ToolError> {
        let action = input
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::Other {
                message: "missing required parameter: 'action'".to_string(),
            })?;

        match action {
            "create" | "update" => {
                let todos_val = input.get("todos").ok_or_else(|| ToolError::Other {
                    message: "missing required parameter: 'todos' for create/update".to_string(),
                })?;

                let todos_arr = todos_val.as_array().ok_or_else(|| ToolError::Other {
                    message: "'todos' must be an array".to_string(),
                })?;

                let new_items: Vec<TodoItem> = todos_arr
                    .iter()
                    .map(|t| TodoItem {
                        id: Uuid::new_v4().to_string(),
                        content: t
                            .get("content")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        active_form: t
                            .get("active_form")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        status: t
                            .get("status")
                            .and_then(|v| v.as_str())
                            .unwrap_or("pending")
                            .to_string(),
                    })
                    .collect();

                let output = {
                    let mut items = self.items.lock().unwrap();
                    *items = new_items;
                    Self::to_output(&items)
                };

                Ok(ToolResult {
                    success: true,
                    output: Some(output),
                    error: None,
                })
            }

            "list" => {
                let items = self.items.lock().unwrap();
                let output = Self::to_output(&items);
                Ok(ToolResult {
                    success: true,
                    output: Some(output),
                    error: None,
                })
            }

            other => Err(ToolError::Other {
                message: format!(
                    "unknown action: '{}'; expected one of: create, update, list",
                    other
                ),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Tool impl
// ---------------------------------------------------------------------------

impl Tool for TodoTool {
    fn name(&self) -> &str {
        "todo"
    }

    fn description(&self) -> &str {
        "Manage an in-memory session-scoped todo list. \
         Actions: create (replace all todos), update (same as create), \
         list (return current todos). IDs are generated via UUID v4."
    }

    fn get_spec(&self) -> ToolSpec {
        let mut properties = HashMap::new();

        properties.insert(
            "action".to_string(),
            json!({
                "type": "string",
                "enum": ["create", "update", "list"],
                "description": "Action to perform: create (replace all), update (same as create), list (read current)"
            }),
        );

        properties.insert(
            "todos".to_string(),
            json!({
                "type": "array",
                "description": "List of todo items (required for create/update, ignored for list)",
                "items": {
                    "type": "object",
                    "properties": {
                        "content":     { "type": "string", "description": "Imperative task description" },
                        "active_form": { "type": "string", "description": "Present-continuous form" },
                        "status":      { "type": "string", "description": "Task status string" }
                    },
                    "required": ["content", "active_form", "status"]
                }
            }),
        );

        let mut parameters = HashMap::new();
        parameters.insert("type".to_string(), json!("object"));
        parameters.insert("properties".to_string(), json!(properties));
        parameters.insert("required".to_string(), json!(["action"]));

        ToolSpec {
            name: "todo".to_string(),
            parameters,
            description: Some(
                "Manage an in-memory session-scoped todo list. \
                 Actions: create (replace all todos), update (same as create), \
                 list (return current todos)."
                    .to_string(),
            ),
            extensions: HashMap::new(),
        }
    }

    fn execute(
        &self,
        input: Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult, ToolError>> + Send + '_>> {
        Box::pin(async move { self.do_execute(&input) })
    }
}
