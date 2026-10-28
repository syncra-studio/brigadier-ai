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

/// The omnibox may omit the scheme and the root slash. Preserve path/query
/// case and never equate two explicitly different schemes.
pub(crate) fn url_matches(actual: &str, requested: &str, exact: bool) -> bool {
    fn parts(value: &str) -> Option<(Option<String>, String)> {
        let value = value.trim();
        if value.is_empty() || value.chars().any(char::is_whitespace) {
            return None;
        }
        let (scheme, rest) = if let Some((scheme, rest)) = value.split_once("://") {
            if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
                return None;
            }
            (Some(scheme.to_ascii_lowercase()), rest)
        } else {
            (None, value)
        };
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let host = &rest[..end];
        if host.is_empty()
            || host.contains('@')
            || !(host.contains('.') || host.starts_with("localhost") || host.starts_with('['))
        {
            return None;
        }
        let tail = &rest[end..];
        Some((
            scheme,
            format!(
                "{}{}",
                host.to_ascii_lowercase(),
                if tail == "/" { "" } else { tail }
            ),
        ))
    }
    let Some((actual_scheme, actual)) = parts(actual) else {
        return false;
    };
    let requested = requested.trim();
    if requested.is_empty() {
        return false;
    }
    if exact {
        let Some((requested_scheme, requested)) = parts(requested) else {
            return false;
        };
        return (actual_scheme.is_none()
            || requested_scheme.is_none()
            || actual_scheme == requested_scheme)
            && actual == requested;
    }
    if let Some((requested_scheme, normalized)) = parts(requested) {
        (actual_scheme.is_none() || requested_scheme.is_none() || actual_scheme == requested_scheme)
            && actual.contains(&normalized)
    } else {
        actual.contains(requested)
    }
}

pub(crate) fn is_omnibox(automation_id: &str, class_name: &str) -> bool {
    matches!(
        automation_id.to_ascii_lowercase().as_str(),
        "addresseditbox" | "omnibox" | "urlbar"
    ) || class_name.eq_ignore_ascii_case("OmniboxView")
}

/// Address-bar evidence binds only a single visible top-level document with
/// no conflicting URL of its own. Never use a title or a page's search field.
pub(crate) fn omnibox_binds_document(
    visible_documents: usize,
    document_url: Option<&str>,
    bar: &str,
    requested: &str,
    exact: bool,
) -> bool {
    visible_documents == 1
        && url_matches(bar, requested, exact)
        && document_url.is_none_or(|url| url_matches(url, requested, exact))
}

#[cfg(test)]
mod omnibox_tests {
    use super::*;
    #[test]
    fn edge_address_value_matches_displayed_scheme_and_root_slash_variants() {
        for actual in [
            "https://example.com",
            "https://example.com/",
            "example.com",
            "EXAMPLE.COM/",
        ] {
            assert!(url_matches(actual, "https://example.com/", true));
            assert!(url_matches(actual, "example.com", false));
        }
        assert!(!url_matches(
            "http://example.com",
            "https://example.com",
            true
        ));
        assert!(!url_matches(
            "https://example.com/Other",
            "https://example.com/other",
            true
        ));
        assert!(!url_matches("Example Domain", "Example", false));
        assert!(!url_matches(
            "https://example.com.evil/",
            "https://example.com",
            true
        ));
    }
    #[test]
    fn omnibox_never_selects_between_documents_or_overrides_a_conflict() {
        assert!(is_omnibox("addressEditBox", ""));
        assert!(is_omnibox("", "OmniboxView"));
        assert!(!is_omnibox("search", "Edit"));
        assert!(omnibox_binds_document(
            1,
            None,
            "example.com",
            "example.com",
            false
        ));
        assert!(!omnibox_binds_document(
            2,
            None,
            "example.com",
            "example.com",
            false
        ));
        assert!(!omnibox_binds_document(
            0,
            None,
            "example.com",
            "example.com",
            false
        ));
        assert!(!omnibox_binds_document(
            1,
            Some("https://other.com"),
            "example.com",
            "example.com",
            false
        ));
    }
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
        assert!(is_web_url("https://example.com"));
        assert!(!is_web_url("Example Domain"));
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

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PageReadRoute {
    Legacy,
    MatchedDocument,
    RefuseUnboundPort,
}

pub(crate) fn page_read_route(has_url_hint: bool, has_cdp_port: bool) -> PageReadRoute {
    if has_cdp_port {
        PageReadRoute::RefuseUnboundPort
    } else if has_url_hint {
        PageReadRoute::MatchedDocument
    } else {
        PageReadRoute::Legacy
    }
}

#[cfg(test)]
mod page_tests {
    use super::*;
    #[test]
    fn targeted_reads_never_use_legacy_window_fallback() {
        assert_eq!(page_read_route(false, false), PageReadRoute::Legacy);
        assert_eq!(page_read_route(true, false), PageReadRoute::MatchedDocument);
        assert_eq!(
            page_read_route(true, true),
            PageReadRoute::RefuseUnboundPort
        );
        assert_eq!(
            page_read_route(false, true),
            PageReadRoute::RefuseUnboundPort
        );
    }
}

/// Accessibility providers may expose each inline text run separately. Keep
/// their internal whitespace and add a boundary when the provider omitted it.
pub(crate) fn append_text_run(output: &mut String, run: &str) {
    if run.is_empty() {
        return;
    }
    if let (Some(left), Some(right)) = (output.chars().last(), run.chars().next()) {
        if !left.is_whitespace()
            && !right.is_whitespace()
            && !matches!(right, ',' | '.' | ';' | ':' | '!' | '?' | ')' | ']' | '}')
            && !matches!(left, '(' | '[' | '{')
        {
            output.push(' ');
        }
    }
    output.push_str(run);
}

#[cfg(test)]
mod text_run_tests {
    use super::*;
    #[test]
    fn inline_runs_keep_word_boundaries_and_existing_spacing() {
        let mut text = String::new();
        for run in [
            "The", "new", "rule", " taking ", "effect", "…", "Hello", ",", "world", "!",
        ] {
            append_text_run(&mut text, run);
        }
        assert_eq!(text, "The new rule taking effect … Hello, world!");
        let mut text = String::new();
        for run in ["αβ", "γδ", "\n", "next\tline", ""] {
            append_text_run(&mut text, run);
        }
        assert_eq!(text, "αβ γδ\nnext\tline");
    }
}

/// Resolve well-known desktop aliases before shell:AppsFolder, whose parsing
/// registrations cannot reliably receive file/folder arguments.
pub(crate) fn named_desktop_executable(name: &str) -> Option<&'static str> {
    match name.trim().to_ascii_lowercase().as_str() {
        "file explorer" | "windows explorer" | "explorer" | "explorer.exe" => Some("explorer.exe"),
        "notepad" | "notepad.exe" => Some("notepad.exe"),
        _ => None,
    }
}

#[cfg(test)]
mod explorer_tests {
    use super::*;
    #[test]
    fn explorer_folder_launch_uses_the_executable_and_keeps_one_argument() {
        assert_eq!(
            named_desktop_executable("File Explorer"),
            Some("explorer.exe")
        );
        assert_eq!(named_browser_executable("File Explorer"), None);
        assert_eq!(
            quote_argument(r"C:\Users\A User\Documents"),
            r#""C:\Users\A User\Documents""#
        );
        assert_eq!(named_desktop_executable("unregistered app"), None);
    }
}
