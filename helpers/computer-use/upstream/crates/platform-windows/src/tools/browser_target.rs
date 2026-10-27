//! Portable rules for exact browser launch and URL selection.
pub(crate) fn named_browser_executable(name: &str) -> Option<&'static str> {
    match name.trim().to_ascii_lowercase().as_str() {
        "microsoft edge" | "edge" | "msedge" | "msedge.exe" => Some("msedge.exe"),
        "google chrome" | "chrome" | "chrome.exe" => Some("chrome.exe"),
        "mozilla firefox" | "firefox" | "firefox.exe" => Some("firefox.exe"),
        "brave" | "brave browser" | "brave.exe" => Some("brave.exe"),
        _ => None,
    }
}

pub(crate) fn urls_belong_to_app(has_resolved_app: bool, has_urls: bool) -> bool {
    has_resolved_app && has_urls
}

pub(crate) fn is_web_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// Quote a single argv item using the Windows CommandLineToArgvW convention.
pub(crate) fn quote_argument(value: &str) -> String {
    let mut output = String::from("\"");
    let mut slashes = 0;
    for ch in value.chars() {
        if ch == '\\' {
            slashes += 1;
            continue;
        }
        if ch == '"' {
            output.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
        } else {
            output.extend(std::iter::repeat_n('\\', slashes));
        }
        slashes = 0;
        output.push(ch);
    }
    output.extend(std::iter::repeat_n('\\', slashes * 2));
    output.push('"');
    output
}

pub(crate) fn url_matches(actual: &str, requested: &str, exact: bool) -> bool {
    if requested.trim().is_empty() || !is_web_url(actual) {
        return false;
    }
    if !exact {
        return actual.contains(requested);
    }
    fn root_slash(url: &str) -> &str {
        if let Some((_, rest)) = url.split_once("://") {
            if rest.ends_with('/') && rest.matches('/').count() == 1 {
                return &url[..url.len() - 1];
            }
        }
        url
    }
    root_slash(actual) == root_slash(requested)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn named_browser_never_resolves_to_default_browser() {
        assert_eq!(
            named_browser_executable("Microsoft Edge"),
            Some("msedge.exe")
        );
        assert_eq!(
            named_browser_executable("Google Chrome"),
            Some("chrome.exe")
        );
        assert_eq!(named_browser_executable("other app"), None);
        assert!(urls_belong_to_app(true, true));
        assert!(!urls_belong_to_app(false, true));
        assert!(!urls_belong_to_app(true, false));
    }
    #[test]
    fn launch_urls_remain_single_arguments() {
        assert_eq!(
            quote_argument("https://example.com/a b?q=\"x\""),
            "\"https://example.com/a b?q=\\\"x\\\"\""
        );
        assert_eq!(quote_argument(r"C:\folder\"), "\"C:\\folder\\\\\"");
    }
    #[test]
    fn url_checks_do_not_accept_page_titles_or_different_pages() {
        assert!(url_matches(
            "https://example.com/",
            "https://example.com",
            true
        ));
        assert!(url_matches(
            "https://example.com/target",
            "example.com",
            false
        ));
        assert!(!url_matches(
            "https://example.com/other",
            "https://example.com/target",
            true
        ));
        assert!(!url_matches(
            "https://example.com/path/",
            "https://example.com/path",
            true
        ));
        assert!(!url_matches("Example Domain", "Example", false));
        assert!(!url_matches("https://example.com", "", false));
    }
}
