//! Which store answers a question best, without a model: names that only code has (an
//! identifier, a path, a quoted name) go to the code index too; everything else is for the
//! knowledge graph alone.

/// Where a question goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// The code index for these names (at most [`MAX_NAMES`]), and the Brain for the whole
    /// question.
    Code { names: Vec<String> },
    /// The Brain alone.
    Brain,
}

/// Names looked up in the code index per question.
pub const MAX_NAMES: usize = 3;
/// A question of at most this many words, each a plain word or a name, may be a bare lookup.
const SHORT: usize = 3;

/// Routes `text`: quoted or backticked spans, and words shaped like identifiers or file
/// paths, are names to look up.
pub fn route(text: &str) -> Route {
    let mut names: Vec<String> = Vec::new();
    let push = |names: &mut Vec<String>, name: &str| {
        if !names.iter().any(|known| known == name) {
            names.push(name.to_owned());
        }
    };
    for span in quoted(text) {
        let span = span.trim();
        // A quoted sentence is prose; a quoted word or path is a name.
        if !span.is_empty() && !span.contains(char::is_whitespace) && !is_url(span) {
            push(&mut names, bare_name(trim(span)));
        }
    }
    let words: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '`' | '"'))
        .map(trim)
        .filter(|word| !word.is_empty())
        .collect();
    for word in &words {
        if is_name(word) {
            push(&mut names, bare_name(word));
        }
    }
    // `Brain` alone, `QueryBrain`: a bare word asked on its own is a lookup too.
    if names.is_empty()
        && words.len() <= SHORT
        && let [word] = words.as_slice()
        && word.chars().next().is_some_and(char::is_uppercase)
        && word.chars().skip(1).any(char::is_uppercase)
        && word.chars().all(|c| c.is_alphanumeric() || c == '_')
    {
        push(&mut names, word);
    }
    names.retain(|name| !name.is_empty());
    names.truncate(MAX_NAMES);
    if names.is_empty() {
        Route::Brain
    } else {
        Route::Code { names }
    }
}

/// The text between matching backticks or double quotes.
fn quoted(text: &str) -> Vec<&str> {
    let mut spans = Vec::new();
    for mark in ['`', '"'] {
        let mut parts = text.split(mark);
        parts.next();
        while let (Some(inside), Some(_)) = (parts.next(), parts.clone().next()) {
            spans.push(inside);
            parts.next();
        }
    }
    spans
}

fn is_url(word: &str) -> bool {
    word.contains("://") || word.starts_with("www.")
}

/// `word` without the punctuation around it; brackets it opens and closes itself stay
/// (`(tabs)/_layout.tsx`).
fn trim(word: &str) -> &str {
    let mut word = word
        .trim_matches(|c: char| matches!(c, '.' | ',' | ':' | ';' | '?' | '!' | '\'' | '`' | '"'));
    for (open, close) in [('(', ')'), ('[', ']'), ('{', '}'), ('<', '>')] {
        if word.starts_with(open) && word.matches(open).count() > word.matches(close).count() {
            word = &word[1..];
        }
        if word.ends_with(close) && word.matches(close).count() > word.matches(open).count() {
            word = &word[..word.len() - 1];
        }
    }
    word.trim_matches(|c: char| matches!(c, '.' | ',' | ':' | ';' | '?' | '!'))
}

/// Strip call syntax only after recognizing it, so plain lowercase function names count.
fn bare_name(word: &str) -> &str {
    word.strip_suffix("()").unwrap_or(word)
}

/// Shaped like something only code has: `a::b`, `snake_case`, `SCREAMING_CASE`,
/// `camelCase`, `PascalCase` with two humps or more, a call `f()`, or a path to a file
/// (`src/lib.rs`, `_layout.tsx`). URLs and plain words are not.
fn is_name(word: &str) -> bool {
    if is_url(word) || word.len() < 3 || !word.chars().any(char::is_alphabetic) {
        return false;
    }
    if let Some(name) = word.strip_suffix("()")
        && name.chars().all(|c| c.is_alphanumeric() || c == '_')
    {
        return true;
    }
    if word.contains("::") {
        return true;
    }
    if is_path(word) {
        return true;
    }
    let ident = word.chars().all(|c| c.is_alphanumeric() || c == '_');
    if !ident {
        return false;
    }
    let chars: Vec<char> = word.chars().collect();
    // An underscore between letters or digits.
    if chars
        .windows(3)
        .any(|w| w[1] == '_' && w[0].is_alphanumeric() && w[2].is_alphanumeric())
        || (word.starts_with('_') && word.len() > 1)
    {
        return true;
    }
    // A hump after two lowercase letters (`useState`, not `iPhone` or `iBeep`).
    let camel = chars
        .windows(3)
        .any(|w| w[0].is_lowercase() && w[1].is_lowercase() && w[2].is_uppercase());
    let humps = chars.iter().filter(|c| c.is_uppercase()).count();
    // Two humps with lowercase between them (`QueryBrain`, not `README` or `ID`).
    let pascal = chars[0].is_uppercase()
        && humps >= 2
        && chars
            .windows(2)
            .any(|w| w[0].is_lowercase() && w[1].is_uppercase());
    camel || pascal
}

/// A file name with an extension of letters, or a path with one: `lib.rs`,
/// `apps/native/(tabs)/_layout.tsx`. A path without an extension (`apps/native`) is a
/// folder, and a sentence's `and/or` is no path.
fn is_path(word: &str) -> bool {
    let name = word.rsplit('/').next().unwrap_or(word);
    let Some((stem, extension)) = name.rsplit_once('.') else {
        return false;
    };
    !stem.is_empty()
        && (1..=5).contains(&extension.len())
        && extension.chars().all(|c| c.is_ascii_alphanumeric())
        && extension.chars().any(|c| c.is_ascii_lowercase())
        // `e.g` and `i.e`, a version `v1.2`, an abbreviation `etc.`.
        && stem.len() > 1
        && !extension.chars().all(|c| c.is_ascii_digit())
        && !stem.chars().last().is_some_and(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(text: &str) -> Vec<String> {
        match route(text) {
            Route::Code { names } => names,
            Route::Brain => Vec::new(),
        }
    }

    #[test]
    fn plain_questions_go_to_the_brain() {
        for text in [
            "What is this project about? Overview, purpose, modules, stack.",
            "Who owns and builds iBeep?",
            "why is the icon strip bar showing the usage as a warning?",
            "Hi, where are we with this project?",
            "and/or e.g. v1.2 README ID",
            "\"what we decided about the router\"",
            "see https://github.com/foo/bar.rs and www.example.com",
        ] {
            assert_eq!(route(text), Route::Brain, "{text}");
        }
    }

    #[test]
    fn names_go_to_the_code_index_too() {
        assert_eq!(names("`PROTOCOL_VERSION`"), ["PROTOCOL_VERSION"]);
        assert_eq!(
            names("where is query_brain_tool called?"),
            ["query_brain_tool"]
        );
        assert_eq!(names("how does Brain::query rank?"), ["Brain::query"]);
        assert_eq!(names("what calls useState() here"), ["useState"]);
        assert_eq!(names("where is lookup() called?"), ["lookup"]);
        assert_eq!(names("where is lookup(), defined?"), ["lookup"]);
        assert_eq!(names("what calls f()?"), ["f"]);
        assert_eq!(names("what calls `lookup()`?"), ["lookup"]);
        assert_eq!(names("QueryBrain"), ["QueryBrain"]);
        assert_eq!(names("the \"SaversSection\" component"), ["SaversSection"]);
        assert_eq!(
            names("apps/native tabs: (tabs)/_layout.tsx, auth gating, src/lib.rs, Cargo.toml"),
            ["(tabs)/_layout.tsx", "src/lib.rs", "Cargo.toml"]
        );
        // A quoted URL isn't a name.
        assert_eq!(
            names("implement `https://github.com/example/dictation` in our Brigadier?"),
            Vec::<String>::new()
        );
    }
}
