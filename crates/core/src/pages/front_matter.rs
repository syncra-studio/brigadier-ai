//! A page file's front matter: the `---` block at its top holding the title, the emoji and when
//! the page was made. Only those keys are read; every other line in the block is kept as it is,
//! so a key another program added survives the next save.

/// What a page file holds, split into its front matter and its Markdown.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct PageFile {
    pub title: String,
    pub emoji: Option<String>,
    pub created_ms: Option<i64>,
    /// The block's other lines, in order.
    pub other: Vec<String>,
    pub body: String,
}

/// Splits a page file. Text without a closed `---` block at its top is all Markdown.
pub(super) fn parse(text: &str) -> PageFile {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Some(rest) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    else {
        return body_only(text);
    };
    let mut page = PageFile::default();
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        let content = line.trim_end_matches(['\n', '\r']);
        if content == "---" || content == "..." {
            let body = &rest[offset + line.len()..];
            // The blank line `render` puts after the block.
            let body = body
                .strip_prefix("\r\n")
                .or_else(|| body.strip_prefix('\n'))
                .unwrap_or(body);
            page.body = body.to_owned();
            return page;
        }
        offset += line.len();
        match content.split_once(':') {
            Some(("title", value)) => page.title = scalar(value),
            Some(("emoji", value)) => page.emoji = Some(scalar(value)).filter(|e| !e.is_empty()),
            Some(("created", value)) => {
                page.created_ms = scalar(value)
                    .parse::<jiff::Timestamp>()
                    .ok()
                    .map(|at| at.as_millisecond());
            }
            _ => page.other.push(content.to_owned()),
        }
    }
    // Never closed: not front matter after all.
    body_only(text)
}

fn body_only(text: &str) -> PageFile {
    PageFile {
        body: text.to_owned(),
        ..PageFile::default()
    }
}

/// The file for `page`: its front matter, a blank line, then its Markdown.
pub(super) fn render(page: &PageFile) -> String {
    let mut out = String::with_capacity(page.body.len() + 128);
    out.push_str("---\n");
    out.push_str("title: ");
    out.push_str(&quote(&page.title));
    out.push('\n');
    if let Some(emoji) = page.emoji.as_deref().filter(|e| !e.is_empty()) {
        out.push_str("emoji: ");
        out.push_str(&quote(emoji));
        out.push('\n');
    }
    if let Some(at) = page
        .created_ms
        .and_then(|ms| jiff::Timestamp::from_millisecond(ms).ok())
    {
        out.push_str("created: ");
        out.push_str(&at.to_string());
        out.push('\n');
    }
    for line in &page.other {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str("---\n\n");
    out.push_str(&page.body);
    out
}

/// A double-quoted string: JSON's escaping is valid YAML, and any text stays on one line.
fn quote(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_owned())
}

/// A value as written by `quote`, by hand in single quotes, or plain.
fn scalar(raw: &str) -> String {
    let value = raw.trim();
    if value.starts_with('"')
        && let Ok(text) = serde_json::from_str::<String>(value)
    {
        return text;
    }
    if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        return value[1..value.len() - 1].replace("''", "'");
    }
    value.to_owned()
}
