use amplifier_module_tool_web::fetch::strip_html;

/// Verifies that <script> tags and their content are completely removed,
/// while other HTML content is preserved.
#[test]
fn strip_html_removes_script_tags_and_content() {
    let html = r#"<p>Hello</p><script>alert('xss');</script><p>World</p>"#;
    let result = strip_html(html);
    assert!(!result.contains("script"), "script tag should be removed");
    assert!(!result.contains("alert"), "script content should be removed");
    assert!(result.contains("Hello"), "main content should be kept");
    assert!(result.contains("World"), "main content should be kept");
}

/// Verifies that <style> tags and their content are completely removed.
#[test]
fn strip_html_removes_style_tags_and_content() {
    let html = r#"<p>Text</p><style>body { color: red; }</style><p>More</p>"#;
    let result = strip_html(html);
    assert!(!result.contains("style"), "style tag should be removed");
    assert!(!result.contains("color: red"), "style content should be removed");
    assert!(result.contains("Text"), "main content should be kept");
    assert!(result.contains("More"), "main content should be kept");
}

/// Verifies that <nav>, <header>, and <footer> tags and their content are removed,
/// while main page content is preserved.
#[test]
fn strip_html_removes_nav_header_footer() {
    let html = r#"<header>Site Header</header><nav>Navigation Links</nav><main>Main Content</main><footer>Footer Text</footer>"#;
    let result = strip_html(html);
    assert!(!result.contains("Site Header"), "header content should be removed");
    assert!(!result.contains("Navigation Links"), "nav content should be removed");
    assert!(!result.contains("Footer Text"), "footer content should be removed");
    assert!(result.contains("Main Content"), "main content should be kept");
}

/// Verifies that multiple consecutive whitespace characters are collapsed
/// into a single space.
#[test]
fn strip_html_collapses_whitespace() {
    let html = "<p>Hello   World</p>\n\n<p>Foo\t\tBar</p>";
    let result = strip_html(html);
    assert!(!result.contains("   "), "multiple spaces should be collapsed");
    assert!(!result.contains("\t\t"), "multiple tabs should be collapsed");
    assert!(!result.contains("\n\n"), "double newlines should be collapsed");
    assert!(result.contains("Hello"), "content should be preserved");
}

/// Verifies that content longer than 8KB is truncated and a truncation notice
/// is appended. The total length must be <= 8*1024 + 100 bytes.
#[test]
fn strip_html_truncates_at_8kb() {
    // Create content larger than 8KB
    let content = "a".repeat(10 * 1024);
    let html = format!("<p>{}</p>", content);
    let result = strip_html(&html);
    assert!(
        result.ends_with("[...truncated at 8KB]"),
        "should end with truncation notice, got: {}",
        &result[result.len().saturating_sub(30)..]
    );
    assert!(
        result.len() <= 8 * 1024 + 100,
        "length {} should be <= {}",
        result.len(),
        8 * 1024 + 100
    );
}

/// Verifies that plain text (without any HTML) passes through unchanged.
#[test]
fn strip_html_plain_text_passthrough() {
    let text = "Just plain text without any HTML tags.";
    let result = strip_html(text);
    assert_eq!(result, text, "plain text should pass through unchanged");
}
