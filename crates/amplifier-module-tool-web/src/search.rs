//! search.rs — SearchWebTool with DuckDuckGo HTML scraping.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;
use serde_json::{json, Value};

use amplifier_core::errors::ToolError;
use amplifier_core::messages::ToolSpec;
use amplifier_core::models::ToolResult;
use amplifier_core::traits::Tool;

// ---------------------------------------------------------------------------
// Regex cache
// ---------------------------------------------------------------------------

struct DdgRegexes {
    /// Match complete <a class="...result__a..."...>...</a> elements.
    result_a_full: Regex,
    /// Extract href="VALUE" from an anchor's attributes.
    href: Regex,
    /// Capture text content after a class="...result__snippet..." opening tag.
    snippet_content: Regex,
    /// Strip all HTML tags.
    strip_tags: Regex,
}

static DDG_REGEXES: OnceLock<DdgRegexes> = OnceLock::new();

fn get_ddg_regexes() -> &'static DdgRegexes {
    DDG_REGEXES.get_or_init(|| DdgRegexes {
        // (?si) = DOTALL + case-insensitive
        result_a_full: Regex::new(
            r#"(?si)<a[^>]*class="[^"]*result__a[^"]*"[^>]*>.*?</a>"#,
        )
        .unwrap(),
        href: Regex::new(r#"href="([^"]*)""#).unwrap(),
        snippet_content: Regex::new(
            r#"(?si)class="[^"]*result__snippet[^"]*"[^>]*>(.*?)</[a-z]"#,
        )
        .unwrap(),
        strip_tags: Regex::new(r#"<[^>]+>"#).unwrap(),
    })
}

// ---------------------------------------------------------------------------
// URL extraction
// ---------------------------------------------------------------------------

/// If `href` contains `uddg=`, URL-decode that parameter value.
/// Otherwise return `href` as-is.
fn extract_uddg_url(href: &str) -> String {
    if let Some(pos) = href.find("uddg=") {
        let encoded = &href[pos + 5..];
        // Stop at the next query-string delimiter if present
        let encoded = encoded.split('&').next().unwrap_or(encoded);
        urlencoding::decode(encoded)
            .map(|s| s.into_owned())
            .unwrap_or_else(|_| encoded.to_string())
    } else {
        href.to_string()
    }
}

// ---------------------------------------------------------------------------
// parse_ddg_results
// ---------------------------------------------------------------------------

/// Parse DuckDuckGo HTML search results into a Vec of JSON objects.
///
/// Each object has `title`, `url`, and `snippet` fields.
/// Returns at most `num_results` items; skips entries with empty title or URL.
pub fn parse_ddg_results(html: &str, num_results: usize) -> Vec<Value> {
    if html.is_empty() {
        return Vec::new();
    }

    let re = get_ddg_regexes();

    // Collect (href, title_html) from all result__a anchors in document order.
    let anchors: Vec<(String, String)> = re
        .result_a_full
        .find_iter(html)
        .map(|m| {
            let tag_str = m.as_str();
            let href = re
                .href
                .captures(tag_str)
                .map(|c| c[1].to_string())
                .unwrap_or_default();
            let title_html = tag_str.to_string();
            (href, title_html)
        })
        .collect();

    // Collect snippet text from all result__snippet elements in document order.
    let snippets: Vec<String> = re
        .snippet_content
        .captures_iter(html)
        .map(|cap| {
            let raw = cap[1].to_string();
            // Strip any nested tags inside the snippet, then trim whitespace.
            re.strip_tags
                .replace_all(&raw, "")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();

    // Pair anchors with snippets by position, respect the limit.
    anchors
        .iter()
        .zip(snippets.iter())
        .take(num_results)
        .filter_map(|((href, title_html), snippet)| {
            // Strip tags from the full anchor match to get plain title text.
            let title = re
                .strip_tags
                .replace_all(title_html, "")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");

            if title.is_empty() || href.is_empty() {
                return None;
            }

            let url = extract_uddg_url(href);

            Some(json!({
                "title": title,
                "url": url,
                "snippet": snippet,
            }))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// SearchWebTool
// ---------------------------------------------------------------------------

/// Tool that searches the web using DuckDuckGo HTML scraping.
pub struct SearchWebTool {
    /// Default HTTP client with a 15-second timeout.
    client: reqwest::Client,
}

impl SearchWebTool {
    /// Create a new [`SearchWebTool`] with a 15-second default timeout.
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_default();
        Self { client }
    }
}

impl Default for SearchWebTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for SearchWebTool {
    fn name(&self) -> &str {
        "search_web"
    }

    fn description(&self) -> &str {
        "Search the web using DuckDuckGo"
    }

    fn get_spec(&self) -> ToolSpec {
        let mut properties = HashMap::new();

        properties.insert(
            "query".to_string(),
            json!({
                "type": "string",
                "description": "The search query"
            }),
        );
        properties.insert(
            "num_results".to_string(),
            json!({
                "type": "integer",
                "description": "Number of results to return",
                "default": 5
            }),
        );

        let mut parameters = HashMap::new();
        parameters.insert("type".to_string(), json!("object"));
        parameters.insert("properties".to_string(), json!(properties));
        parameters.insert("required".to_string(), json!(["query"]));

        ToolSpec {
            name: "search_web".to_string(),
            parameters,
            description: Some("Search the web using DuckDuckGo".to_string()),
            extensions: HashMap::new(),
        }
    }

    fn execute(
        &self,
        input: Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult, ToolError>> + Send + '_>> {
        Box::pin(async move {
            // --- Extract query (required) ---
            let query = match input.get("query").and_then(|v| v.as_str()) {
                Some(s) if !s.is_empty() => s.to_string(),
                _ => {
                    return Err(ToolError::Other {
                        message: "missing required parameter: 'query'".to_string(),
                    });
                }
            };

            // --- Extract num_results (default 5) ---
            let num_results = input
                .get("num_results")
                .and_then(|v| v.as_u64())
                .unwrap_or(5) as usize;

            // --- URL-encode the query ---
            let encoded_query = urlencoding::encode(&query);
            let url = format!("https://duckduckgo.com/html/?q={}", encoded_query);

            // --- Perform GET request ---
            let response = self
                .client
                .get(&url)
                .header(
                    reqwest::header::USER_AGENT,
                    "Mozilla/5.0 (compatible; amplifier-search/0.1)",
                )
                .send()
                .await
                .map_err(|e| ToolError::Other {
                    message: format!("request failed: {}", e),
                })?;

            // --- Read body ---
            let body = response.text().await.map_err(|e| ToolError::Other {
                message: format!("failed to read response body: {}", e),
            })?;

            // --- Parse results ---
            let results = parse_ddg_results(&body, num_results);

            Ok(ToolResult {
                success: true,
                output: Some(json!(results)),
                error: None,
            })
        })
    }
}
