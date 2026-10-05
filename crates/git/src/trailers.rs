//! Commit message trailers: the Co-authored-by lines that name an AI, which the user's
//! setting leaves out of commits.

/// `message` without its Co-authored-by trailers that name an AI (people's are kept), and
/// without the blank lines that leaves at its end. Returned unchanged when it has none, or
/// when nothing else would be left.
pub fn strip_ai_coauthors(message: &str) -> String {
    let mut removed = false;
    let mut lines: Vec<&str> = Vec::new();
    for line in message.lines() {
        if ai_coauthor(line) {
            removed = true;
        } else {
            lines.push(line);
        }
    }
    if !removed {
        return message.to_owned();
    }
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    if lines.is_empty() {
        return message.to_owned();
    }
    let mut cleaned = lines.join("\n");
    if message.ends_with('\n') {
        cleaned.push('\n');
    }
    cleaned
}

/// Whether `line` is a Co-authored-by trailer whose name or email names an AI.
fn ai_coauthor(line: &str) -> bool {
    let Some((key, value)) = line.split_once(':') else {
        return false;
    };
    if !key.trim().eq_ignore_ascii_case("co-authored-by") {
        return false;
    }
    let value = value.to_ascii_lowercase();
    let words: Vec<&str> = value
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    words.iter().enumerate().any(|(index, word)| {
        matches!(
            *word,
            "claude"
                | "anthropic"
                | "codex"
                | "openai"
                | "copilot"
                | "gemini"
                | "cursoragent"
                | "devin"
                | "aider"
        ) || word.starts_with("gpt")
            || word.ends_with("gpt")
            || (*word == "cursor" && words.get(index + 1) == Some(&"agent"))
    })
}
