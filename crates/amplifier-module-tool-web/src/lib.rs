pub mod fetch;
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
                "fetch_url".to_string(),
                Arc::new(FetchUrlTool::new()) as Arc<dyn Tool>,
            ),
            (
                "search_web".to_string(),
                Arc::new(SearchWebTool::new()) as Arc<dyn Tool>,
            ),
        ]
    }
}
