//! Integration tests for amplifier-module-tool-todo.

use amplifier_core::traits::Tool;
use amplifier_module_tool_todo::TodoTool;
use serde_json::json;

/// Test that create action replaces all todos and returns correct output.
#[tokio::test]
async fn create_replaces_all_todos() {
    let tool = TodoTool::default();
    let input = json!({
        "action": "create",
        "todos": [
            {
                "content": "Task 1",
                "active_form": "Working on task 1",
                "status": "pending"
            },
            {
                "content": "Task 2",
                "active_form": "Working on task 2",
                "status": "in_progress"
            }
        ]
    });

    let result = tool.execute(input).await.unwrap();
    assert!(result.success);

    let output = result.output.unwrap();
    assert_eq!(output["count"].as_u64().unwrap(), 2);

    let todos = output["todos"].as_array().unwrap();
    assert_eq!(todos.len(), 2);

    // IDs should be non-empty strings
    assert!(!todos[0]["id"].as_str().unwrap().is_empty());
    assert!(!todos[1]["id"].as_str().unwrap().is_empty());

    // Content and status preserved
    assert_eq!(todos[0]["content"], "Task 1");
    assert_eq!(todos[1]["content"], "Task 2");
    assert_eq!(todos[0]["status"], "pending");
    assert_eq!(todos[1]["status"], "in_progress");
}

/// Test that list returns the current todos after a create.
#[tokio::test]
async fn list_returns_current_todos() {
    let tool = TodoTool::default();

    // Create one todo
    let create_input = json!({
        "action": "create",
        "todos": [
            {
                "content": "My task",
                "active_form": "Working on my task",
                "status": "pending"
            }
        ]
    });
    tool.execute(create_input).await.unwrap();

    // List todos
    let list_input = json!({ "action": "list" });
    let result = tool.execute(list_input).await.unwrap();
    assert!(result.success);

    let output = result.output.unwrap();
    assert_eq!(output["count"].as_u64().unwrap(), 1);

    let todos = output["todos"].as_array().unwrap();
    assert_eq!(todos.len(), 1);
    assert_eq!(todos[0]["content"], "My task");
}

/// Test that update replaces all existing todos (old ones are gone).
#[tokio::test]
async fn update_replaces_all_todos() {
    let tool = TodoTool::default();

    // Create an initial todo
    let create_input = json!({
        "action": "create",
        "todos": [
            {
                "content": "Old task",
                "active_form": "Working on old task",
                "status": "pending"
            }
        ]
    });
    let create_result = tool.execute(create_input).await.unwrap();
    let old_id = create_result.output.unwrap()["todos"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Update with 2 new todos
    let update_input = json!({
        "action": "update",
        "todos": [
            {
                "content": "New task 1",
                "active_form": "Working on new task 1",
                "status": "pending"
            },
            {
                "content": "New task 2",
                "active_form": "Working on new task 2",
                "status": "completed"
            }
        ]
    });
    let result = tool.execute(update_input).await.unwrap();
    assert!(result.success);

    let output = result.output.unwrap();
    assert_eq!(output["count"].as_u64().unwrap(), 2);

    let todos = output["todos"].as_array().unwrap();
    assert_eq!(todos.len(), 2);

    // Old todo's ID should not appear
    for todo in todos {
        assert_ne!(todo["id"].as_str().unwrap(), old_id.as_str());
    }

    // New todos should have correct content
    assert_eq!(todos[0]["content"], "New task 1");
    assert_eq!(todos[1]["content"], "New task 2");
}

/// Test that listing a fresh tool returns an empty array.
#[tokio::test]
async fn list_empty_returns_empty_array() {
    let tool = TodoTool::default();

    let input = json!({ "action": "list" });
    let result = tool.execute(input).await.unwrap();
    assert!(result.success);

    let output = result.output.unwrap();
    assert_eq!(output["count"].as_u64().unwrap(), 0);

    let todos = output["todos"].as_array().unwrap();
    assert!(todos.is_empty());
}

/// Test that an unknown action returns an Err.
#[tokio::test]
async fn unknown_action_returns_error() {
    let tool = TodoTool::default();

    let input = json!({ "action": "delete" });
    let result = tool.execute(input).await;
    assert!(result.is_err(), "expected Err for unknown action 'delete'");
}

/// Test that IDs are unique across separate create calls.
#[tokio::test]
async fn ids_are_unique_across_creates() {
    let tool = TodoTool::default();

    let input1 = json!({
        "action": "create",
        "todos": [
            {
                "content": "Task A",
                "active_form": "Working on A",
                "status": "pending"
            }
        ]
    });
    let result1 = tool.execute(input1).await.unwrap();
    let id1 = result1.output.unwrap()["todos"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let input2 = json!({
        "action": "create",
        "todos": [
            {
                "content": "Task B",
                "active_form": "Working on B",
                "status": "pending"
            }
        ]
    });
    let result2 = tool.execute(input2).await.unwrap();
    let id2 = result2.output.unwrap()["todos"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    assert_ne!(id1, id2, "IDs from separate create calls should be unique");
}
