//! Web tools for the Amplifier agent framework.
//!
//! Provides [`WebToolSuite`] which exposes `web_fetch` and `search_web` tools.

/// HTTP URL fetch tool (`web_fetch`).
pub mod fetch;
/// Web search tool (`search_web`).
pub mod search;

use std::sync::Arc;

use amplifier_core::traits::Tool;

use crate::fetch::FetchUrlTool;
use crate::search::SearchWebTool;

/// Aggregator that exposes all web tools as a named collection.
pub struct WebToolSuite;

impl WebToolSuite {
    /// Return all web tools as `(name, Arc<dyn Tool>)` pairs.
    pub fn tools() -> Vec<(String, Arc<dyn Tool>)> {
        vec![
            (
                "web_fetch".to_string(),
                Arc::new(FetchUrlTool::new()) as Arc<dyn Tool>,
            ),
            (
                "search_web".to_string(),
                Arc::new(SearchWebTool::new()) as Arc<dyn Tool>,
            ),
        ]
    }
}
