//! fetch.rs — FetchUrlTool with HTML stripping and 8KB truncation.

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
// Constants
// ---------------------------------------------------------------------------

const MAX_BYTES: usize = 8 * 1024;
const TRUNCATION_NOTICE: &str = "[...truncated at 8KB]";

// ---------------------------------------------------------------------------
// Regex cache
// ---------------------------------------------------------------------------

/// Compiled HTML-stripping regexes, initialized once.
struct HtmlRegexes {
    script: Regex,
    style: Regex,
    nav: Regex,
    header: Regex,
    footer: Regex,
    tags: Regex,
    whitespace: Regex,
}

static HTML_REGEXES: OnceLock<HtmlRegexes> = OnceLock::new();

fn get_regexes() -> &'static HtmlRegexes {
    HTML_REGEXES.get_or_init(|| HtmlRegexes {
        // (?s) = DOTALL (. matches \n), (?i) = case-insensitive
        script: Regex::new(r"(?si)<script[^>]*>.*?</script\s*>").unwrap(),
        style: Regex::new(r"(?si)<style[^>]*>.*?</style\s*>").unwrap(),
        nav: Regex::new(r"(?si)<nav[^>]*>.*?</nav\s*>").unwrap(),
        header: Regex::new(r"(?si)<header[^>]*>.*?</header\s*>").unwrap(),
        footer: Regex::new(r"(?si)<footer[^>]*>.*?</footer\s*>").unwrap(),
        tags: Regex::new(r"<[^>]+>").unwrap(),
        whitespace: Regex::new(r"\s+").unwrap(),
    })
}

// ---------------------------------------------------------------------------
// strip_html
// ---------------------------------------------------------------------------

/// Strip HTML from `html`, collapse whitespace, and truncate at 8KB.
///
/// Steps:
/// 1. Remove block-level noise (`<script>`, `<style>`, `<nav>`, `<header>`,
///    `<footer>` — tags **and** their content).
/// 2. Strip all remaining HTML tags.
/// 3. Collapse runs of whitespace to a single space and trim.
/// 4. If the result exceeds `MAX_BYTES`, truncate and append
///    `TRUNCATION_NOTICE`.
pub fn strip_html(html: &str) -> String {
    let re = get_regexes();

    // Step 1 — remove block-level noise
    let s = re.script.replace_all(html, "");
    let s = re.style.replace_all(&s, "");
    let s = re.nav.replace_all(&s, "");
    let s = re.header.replace_all(&s, "");
    let s = re.footer.replace_all(&s, "");

    // Step 2 — strip all remaining tags
    let s = re.tags.replace_all(&s, "");

    // Step 3 — collapse whitespace and trim
    let s = re.whitespace.replace_all(&s, " ");
    let mut result = s.trim().to_string();

    // Step 4 — truncate at MAX_BYTES
    if result.len() > MAX_BYTES {
        // Truncate at a valid UTF-8 char boundary at or before MAX_BYTES
        let boundary = result
            .char_indices()
            .take_while(|&(i, _)| i < MAX_BYTES)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(MAX_BYTES);
        result.truncate(boundary);
        result.push_str(TRUNCATION_NOTICE);
    }

    result
}

// ---------------------------------------------------------------------------
// FetchUrlTool
// ---------------------------------------------------------------------------

/// Tool that fetches a URL and returns its content as stripped text.
pub struct FetchUrlTool {
    /// Default client with a 30-second timeout.
    client: reqwest::Client,
}

impl FetchUrlTool {
    /// Create a new [`FetchUrlTool`] with a 30-second default timeout.
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();
        Self { client }
    }
}

impl Default for FetchUrlTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for FetchUrlTool {
    fn name(&self) -> &str {
        "fetch_url"
    }

    fn description(&self) -> &str {
        "Fetch content from a URL and return as text"
    }

    fn get_spec(&self) -> ToolSpec {
        let mut properties = HashMap::new();

        properties.insert(
            "url".to_string(),
            json!({
                "type": "string",
                "description": "The URL to fetch"
            }),
        );
        properties.insert(
            "timeout_secs".to_string(),
            json!({
                "type": "integer",
                "description": "Request timeout in seconds",
                "default": 10
            }),
        );

        let mut parameters = HashMap::new();
        parameters.insert("type".to_string(), json!("object"));
        parameters.insert("properties".to_string(), json!(properties));
        parameters.insert("required".to_string(), json!(["url"]));

        ToolSpec {
            name: "fetch_url".to_string(),
            parameters,
            description: Some("Fetch content from a URL and return as text".to_string()),
            extensions: HashMap::new(),
        }
    }

    fn execute(
        &self,
        input: Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult, ToolError>> + Send + '_>> {
        Box::pin(async move {
            // --- Extract url (required) ---
            let url = match input.get("url").and_then(|v| v.as_str()) {
                Some(s) if !s.is_empty() => s.to_string(),
                _ => {
                    return Err(ToolError::Other {
                        message: "missing required parameter: 'url'".to_string(),
                    });
                }
            };

            // --- Extract timeout_secs (default 10) ---
            let timeout_secs = input
                .get("timeout_secs")
                .and_then(|v| v.as_u64())
                .unwrap_or(10);

            // --- Build per-request client with specified timeout ---
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(timeout_secs))
                .build()
                .unwrap_or_else(|_| self.client.clone());

            // --- Perform GET request ---
            let response = client
                .get(&url)
                .header(
                    reqwest::header::USER_AGENT,
                    "amplifier-tool-web/0.1",
                )
                .send()
                .await
                .map_err(|e| ToolError::Other {
                    message: format!("request failed: {}", e),
                })?;

            // --- Check HTTP status ---
            let status = response.status();
            if !status.is_success() {
                return Err(ToolError::Other {
                    message: format!("HTTP {}: {}", status.as_u16(), url),
                });
            }

            // --- Read body text ---
            let body = response.text().await.map_err(|e| ToolError::Other {
                message: format!("failed to read response body: {}", e),
            })?;

            // --- Strip HTML and truncate ---
            let text = strip_html(&body);

            Ok(ToolResult {
                success: true,
                output: Some(json!(text)),
                error: None,
            })
        })
    }
}
