//! Full-page content extraction for link understanding
//!
//! Extracts readable text from HTML pages for LLM context injection

use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};

/// Extracted page content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkContent {
    /// Original URL
    pub url: String,
    /// Page title
    pub title: Option<String>,
    /// Extracted readable text
    pub text: String,
    /// Content length before truncation
    pub original_length: usize,
}

/// Extract readable text content from HTML
///
/// Strips navigation, scripts, styles, and other non-content elements,
/// then extracts visible text. Truncates to `max_length` characters.
#[must_use]
#[allow(clippy::option_if_let_else)]
pub fn extract_content(url: &str, html: &str, max_length: usize) -> LinkContent {
    let document = Html::parse_document(html);

    // Extract title
    let title = Selector::parse("title").ok().and_then(|sel| {
        document
            .select(&sel)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
    });

    // Selectors for elements to skip
    let skip_selectors = [
        "script",
        "style",
        "noscript",
        "nav",
        "header",
        "footer",
        "aside",
        "iframe",
        "svg",
        "form",
        "[role=navigation]",
        "[role=banner]",
        "[role=contentinfo]",
        ".nav",
        ".menu",
        ".sidebar",
        ".footer",
        ".header",
        ".cookie-banner",
        ".advertisement",
    ];

    let skip_set: Vec<Selector> = skip_selectors
        .iter()
        .filter_map(|s| Selector::parse(s).ok())
        .collect();

    // Try to find main content area first
    let content_selectors = [
        "article",
        "main",
        "[role=main]",
        ".content",
        ".post",
        "#content",
    ];
    let mut content_root = None;
    for sel_str in &content_selectors {
        if let Ok(sel) = Selector::parse(sel_str)
            && let Some(el) = document.select(&sel).next()
        {
            content_root = Some(el);
            break;
        }
    }

    // Extract text from the content root or the full body
    let text = if let Some(root) = content_root {
        extract_text_from_element(&root, &skip_set)
    } else if let Ok(body_sel) = Selector::parse("body") {
        document
            .select(&body_sel)
            .next()
            .map_or_else(String::new, |body| {
                extract_text_from_element(&body, &skip_set)
            })
    } else {
        document.root_element().text().collect::<String>()
    };

    // Clean up whitespace
    let cleaned = normalize_whitespace(&text);
    let original_length = cleaned.len();

    // Truncate to max length at a word boundary
    let truncated = if cleaned.len() > max_length {
        truncate_at_word_boundary(&cleaned, max_length)
    } else {
        cleaned
    };

    LinkContent {
        url: url.to_string(),
        title,
        text: truncated,
        original_length,
    }
}

/// Extract visible text from an HTML element, skipping certain child elements
fn extract_text_from_element(
    element: &scraper::ElementRef<'_>,
    skip_selectors: &[Selector],
) -> String {
    let mut parts = Vec::new();

    for node in element.children() {
        if let Some(el) = scraper::ElementRef::wrap(node) {
            // Skip non-content elements
            if skip_selectors.iter().any(|sel| sel.matches(&el)) {
                continue;
            }
            let text = el.text().collect::<String>();
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                parts.push(trimmed.to_string());
            }
        } else if let Some(text) = node.value().as_text() {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                parts.push(trimmed.to_string());
            }
        }
    }

    parts.join("\n")
}

/// Collapse multiple whitespace/newlines into single spaces/newlines
fn normalize_whitespace(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut prev_newline = false;
    let mut prev_space = false;

    for ch in text.chars() {
        if ch == '\n' {
            if !prev_newline {
                result.push('\n');
            }
            prev_newline = true;
            prev_space = false;
        } else if ch.is_whitespace() {
            if !prev_space && !prev_newline {
                result.push(' ');
            }
            prev_space = true;
        } else {
            result.push(ch);
            prev_newline = false;
            prev_space = false;
        }
    }

    result.trim().to_string()
}

/// Truncate text at the nearest word boundary before `max_len`
fn truncate_at_word_boundary(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        return text.to_string();
    }

    // Find last space before max_len
    let truncated = &text[..max_len];
    truncated.rfind(' ').map_or_else(
        || format!("{truncated}..."),
        |last_space| format!("{}...", &text[..last_space]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_basic_content() {
        let html = r#"<html><head><title>Test Page</title></head>
        <body><article><p>Hello world. This is content.</p></article></body></html>"#;
        let result = extract_content("https://example.com", html, 4000);
        assert_eq!(result.title.as_deref(), Some("Test Page"));
        assert!(result.text.contains("Hello world"));
    }

    #[test]
    fn skips_scripts_and_styles() {
        let html = r"<html><body>
        <script>alert('xss')</script>
        <style>.foo{color:red}</style>
        <p>Visible text</p>
        </body></html>";
        let result = extract_content("https://example.com", html, 4000);
        assert!(!result.text.contains("alert"));
        assert!(!result.text.contains("color:red"));
        assert!(result.text.contains("Visible text"));
    }

    #[test]
    fn truncates_long_content() {
        let long_text = "word ".repeat(1000);
        let html = format!("<html><body><p>{long_text}</p></body></html>");
        let result = extract_content("https://example.com", &html, 100);
        assert!(result.text.len() <= 110); // 100 + "..." + word boundary tolerance
        assert!(result.text.ends_with("..."));
    }

    #[test]
    fn normalize_whitespace_collapses() {
        let input = "Hello   world\n\n\n\nFoo   bar";
        let result = normalize_whitespace(input);
        assert_eq!(result, "Hello world\nFoo bar");
    }
}
