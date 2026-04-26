//! fetch.rs — WebFetchTool with HTML stripping, configurable offset/limit, SSRF protection,
//! and structured JSON response metadata.

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

/// Default maximum bytes returned in a single response (200 KB).
const DEFAULT_LIMIT: usize = 200 * 1024;

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

/// Strip HTML from `html` and collapse whitespace.
///
/// Steps:
/// 1. Remove block-level noise (`<script>`, `<style>`, `<nav>`, `<header>`,
///    `<footer>` — tags **and** their content).
/// 2. Strip all remaining HTML tags.
/// 3. Collapse runs of whitespace to a single space and trim.
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
    s.trim().to_string()
}

// ---------------------------------------------------------------------------
// SSRF protection
// ---------------------------------------------------------------------------

/// Return `true` if the URL's host is a private or loopback address.
///
/// Blocked ranges:
/// - `localhost` / `*.localhost` (domain)
/// - `127.0.0.0/8` (IPv4 loopback)
/// - `::1` (IPv6 loopback)
/// - `10.0.0.0/8` (RFC-1918 private)
/// - `192.168.0.0/16` (RFC-1918 private)
/// - `172.16.0.0/12` (RFC-1918 private, 172.16–172.31)
/// - `169.254.0.0/16` (link-local / APIPA)
fn is_private_host(url_str: &str) -> Result<bool, ToolError> {
    let parsed = url::Url::parse(url_str).map_err(|e| ToolError::Other {
        message: format!("invalid URL: {}", e),
    })?;

    match parsed.host() {
        Some(url::Host::Domain(host)) => {
            let lower = host.to_lowercase();
            // "localhost" or "foo.localhost"
            Ok(lower == "localhost" || lower.ends_with(".localhost"))
        }
        Some(url::Host::Ipv4(addr)) => {
            let [a, b, _, _] = addr.octets();
            Ok(a == 127                               // 127.0.0.0/8   loopback
                || a == 10                            // 10.0.0.0/8    private
                || (a == 192 && b == 168)             // 192.168.0.0/16
                || (a == 172 && (16..=31).contains(&b)) // 172.16.0.0/12
                || (a == 169 && b == 254))            // 169.254.0.0/16 link-local
        }
        Some(url::Host::Ipv6(addr)) => Ok(addr.is_loopback()),
        None => Err(ToolError::Other {
            message: "invalid URL: no host found".to_string(),
        }),
    }
}

// ---------------------------------------------------------------------------
// slice_text
// ---------------------------------------------------------------------------

/// Return a substring of `text` starting at byte `offset`, up to `limit` bytes,
/// clamped to valid UTF-8 character boundaries.
fn slice_text(text: &str, offset: usize, limit: usize) -> &str {
    let len = text.len();
    if offset >= len {
        return "";
    }

    // Advance start to the next valid char boundary at or after `offset`.
    let start = (offset..=len)
        .find(|&i| text.is_char_boundary(i))
        .unwrap_or(len);

    let end_raw = (start + limit).min(len);

    // Retreat end to the nearest valid char boundary at or before `end_raw`.
    let end = (start..=end_raw)
        .rev()
        .find(|&i| text.is_char_boundary(i))
        .unwrap_or(start);

    &text[start..end]
}

// ---------------------------------------------------------------------------
// FetchUrlTool
// ---------------------------------------------------------------------------

/// Tool that fetches a URL and returns its content as text with structured metadata.
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
        "web_fetch"
    }

    fn description(&self) -> &str {
        "Fetch content from a web URL"
    }

    fn get_spec(&self) -> ToolSpec {
        let mut properties = HashMap::new();

        properties.insert(
            "url".to_string(),
            json!({
                "type": "string",
                "description": "URL to fetch content from"
            }),
        );
        properties.insert(
            "save_to_file".to_string(),
            json!({
                "type": "string",
                "description": "Save full content to this file path instead of returning in response. \
                                Returns metadata + preview when set."
            }),
        );
        properties.insert(
            "offset".to_string(),
            json!({
                "type": "integer",
                "description": "Start reading from byte N (default 0). Use for pagination.",
                "default": 0
            }),
        );
        properties.insert(
            "limit".to_string(),
            json!({
                "type": "integer",
                "description": "Max bytes to return (default 200KB). Use for pagination.",
                "default": DEFAULT_LIMIT
            }),
        );

        let mut parameters = HashMap::new();
        parameters.insert("type".to_string(), json!("object"));
        parameters.insert("properties".to_string(), json!(properties));
        parameters.insert("required".to_string(), json!(["url"]));

        ToolSpec {
            name: "web_fetch".to_string(),
            parameters,
            description: Some("Fetch content from a web URL".to_string()),
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

            // --- SSRF protection ---
            if is_private_host(&url)? {
                return Err(ToolError::Other {
                    message: "Blocked: private/loopback addresses not allowed".to_string(),
                });
            }

            // --- Optional parameters ---
            let save_to_file: Option<String> = input
                .get("save_to_file")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());

            let offset: usize = input
                .get("offset")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;

            let limit: usize = input
                .get("limit")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize)
                .unwrap_or(DEFAULT_LIMIT);

            // --- Perform GET request ---
            let response = self
                .client
                .get(&url)
                .header(reqwest::header::USER_AGENT, "amplifier-tool-web/0.1")
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

            // --- Extract content-type before consuming response ---
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("application/octet-stream")
                .to_string();

            // --- Read body bytes ---
            let body_bytes = response.bytes().await.map_err(|e| ToolError::Other {
                message: format!("failed to read response body: {}", e),
            })?;

            let total_bytes = body_bytes.len();

            // --- Convert to text (lossy UTF-8) ---
            let raw_text = String::from_utf8_lossy(&body_bytes).into_owned();

            // --- Strip HTML when the response is an HTML document ---
            let is_html = content_type.contains("text/html");
            let processed = if is_html {
                strip_html(&raw_text)
            } else {
                raw_text
            };

            // --- Save full content to file if requested ---
            if let Some(ref path) = save_to_file {
                tokio::fs::write(path, processed.as_bytes())
                    .await
                    .map_err(|e| ToolError::Other {
                        message: format!("failed to write to file '{}': {}", path, e),
                    })?;

                return Ok(ToolResult {
                    success: true,
                    output: Some(json!({
                        "url": url,
                        "content_type": content_type,
                        "total_bytes": total_bytes,
                        "saved_to": path,
                        "truncated": false,
                    })),
                    error: None,
                });
            }

            // --- Apply offset + limit ---
            let sliced = slice_text(&processed, offset, limit);
            let truncated = (offset + limit) < processed.len();

            Ok(ToolResult {
                success: true,
                output: Some(json!({
                    "url": url,
                    "content": sliced,
                    "content_type": content_type,
                    "truncated": truncated,
                    "total_bytes": total_bytes,
                })),
                error: None,
            })
        })
    }
}
