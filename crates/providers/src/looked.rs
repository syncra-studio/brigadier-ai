//! What a tool call or command read and searched, from what each CLI tells of it
//! ([`ProviderEvent::Looked`](crate::ProviderEvent::Looked)): the line range a shell read
//! printed, and the files a search's output names.

use crate::model::{FileRead, LineRange, SearchKind};

/// Files kept per search: a listing of a large folder names more than anyone reads.
pub(crate) const HITS_KEPT: usize = 200;

/// The lines a shell read prints, when its command says (`sed -n '<a>,<b>p'`, `head -n <n>`;
/// a pipeline's last such step decides). `None` for the whole file or a part it doesn't tell
/// (`cat`, `tail`).
pub(crate) fn command_lines(command: &str) -> Option<LineRange> {
    let stages = command.split('|').map(str::trim).collect::<Vec<_>>();
    stages.iter().rev().find_map(|stage| {
        let words = words(stage)?;
        match words.first().map(String::as_str) {
            Some("sed") => sed_lines(&words[1..]),
            Some("head") => head_lines(&words[1..]),
            _ => None,
        }
    })
}

/// `sed -n '<a>,<b>p'` (or `'<a>p'`, `'<a>,$p'`) → its lines.
fn sed_lines(args: &[String]) -> Option<LineRange> {
    if args.first().map(String::as_str) != Some("-n") {
        return None;
    }
    let script = args.get(1)?.strip_suffix('p')?;
    let (start, end) = match script.split_once(',') {
        Some((start, "$")) => (start.parse().ok()?, None),
        Some((start, end)) => (start.parse().ok()?, Some(end.parse().ok()?)),
        None => {
            let line = script.parse().ok()?;
            (line, Some(line))
        }
    };
    (start >= 1 && end.is_none_or(|end| end >= start)).then_some(LineRange { start, end })
}

/// `head`, `head -n <n>`, `head -<n>` → the first lines.
fn head_lines(args: &[String]) -> Option<LineRange> {
    let mut count = 10;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "-n" {
            count = args.next()?.parse().ok()?;
        } else if let Some(n) = arg.strip_prefix("-n") {
            count = n.parse().ok()?;
        } else if let Some(n) = arg.strip_prefix('-') {
            // `-c` counts bytes, not lines.
            count = n.parse().ok()?;
        }
    }
    (count >= 1).then_some(LineRange {
        start: 1,
        end: Some(count),
    })
}

/// A plain `sed -n '<range>p' <file>` Codex could not parse (it leaves `'<a>,$p'` unknown):
/// the file and its lines.
pub(crate) fn sed_read(command: &str) -> Option<FileRead> {
    let words = words(command)?;
    let [sed, _, _, file] = words.as_slice() else {
        return None;
    };
    if sed != "sed" {
        return None;
    }
    let lines = sed_lines(&words[1..3])?;
    Some(FileRead {
        path: file.clone(),
        lines: Some(lines),
    })
}

/// The words of a simple command, its quotes removed; `None` for anything a shell would do
/// more with (a pipe, a list, a redirection, a substitution or a variable).
fn words(command: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word: Option<String> = None;
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                let word = word.get_or_insert_with(String::new);
                loop {
                    match chars.next()? {
                        '\'' => break,
                        c => word.push(c),
                    }
                }
            }
            '"' => {
                let word = word.get_or_insert_with(String::new);
                loop {
                    match chars.next()? {
                        '"' => break,
                        '$' | '`' | '\\' => return None,
                        c => word.push(c),
                    }
                }
            }
            c if c.is_whitespace() => {
                if let Some(word) = word.take() {
                    words.push(word);
                }
            }
            '|' | ';' | '&' | '<' | '>' | '$' | '`' | '\\' | '(' | ')' | '*' | '?' => {
                return None;
            }
            c => word.get_or_insert_with(String::new).push(c),
        }
    }
    words.extend(word);
    (!words.is_empty()).then_some(words)
}

/// The files a search printed: a content search's `path:line:` (or `path:`) prefixes, a file
/// search's or listing's lines (the name of `ls -l`'s rows). Candidates only: what names no
/// file is dropped where the paths are resolved. At most [`HITS_KEPT`], each once.
pub(crate) fn output_hits(output: &str, kind: SearchKind) -> Vec<String> {
    let mut hits: Vec<String> = Vec::new();
    for line in output.lines() {
        let line = line.trim_end();
        if line.is_empty() || line.starts_with("total ") {
            continue;
        }
        let hit = match kind {
            SearchKind::Content => content_hit(line),
            SearchKind::Files => Some(listed_name(line)),
        };
        if let Some(hit) = hit
            && !hit.is_empty()
            && !hits.iter().any(|known| known == hit)
        {
            hits.push(hit.to_owned());
            if hits.len() == HITS_KEPT {
                break;
            }
        }
    }
    hits
}

/// `path:41:text` (or `path-41-context`, a context line) with line numbers, else `path:text`,
/// else a line that is only a path (`-l`).
fn content_hit(line: &str) -> Option<&str> {
    [':', '-']
        .into_iter()
        .find_map(|separator| {
            line.match_indices(separator).find_map(|(at, _)| {
                let rest = &line[at + 1..];
                let digits = rest.chars().take_while(char::is_ascii_digit).count();
                (at > 0 && digits > 0 && rest[digits..].starts_with(separator)).then(|| &line[..at])
            })
        })
        .or_else(|| match line.split_once(':') {
            Some((path, _)) => (!path.is_empty()).then_some(path),
            // A search that names only files (`-l`).
            None => Some(line),
        })
}

/// A listing's file name: the line, or the last column of an `ls -l` row.
fn listed_name(line: &str) -> &str {
    // Bytes, not a `str` slice: byte 10 may fall inside a multibyte file name.
    let long = line.len() > 10
        && line.as_bytes()[..10].iter().all(|c| {
            matches!(
                c,
                b'-' | b'd' | b'l' | b'r' | b'w' | b'x' | b's' | b't' | b'@' | b'+'
            )
        });
    if long {
        line.rsplit(' ').next().unwrap_or(line)
    } else {
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listed_name_may_be_multibyte() {
        assert_eq!(listed_name("中文文件.md"), "中文文件.md");
        assert_eq!(
            listed_name("-rw-r--r--  1 me  staff  12 Oct  7 21:00 中文文件.md"),
            "中文文件.md"
        );
    }

    fn range(start: u64, end: Option<u64>) -> Option<LineRange> {
        Some(LineRange { start, end })
    }

    #[test]
    fn a_shell_read_tells_its_lines_when_the_command_does() {
        assert_eq!(command_lines("sed -n '35,44p' src.rs"), range(35, Some(44)));
        assert_eq!(command_lines("sed -n 7p a.rs"), range(7, Some(7)));
        assert_eq!(command_lines("sed -n '100,$p' a.rs"), range(100, None));
        assert_eq!(
            command_lines("nl -ba src.rs | sed -n '1,5p'"),
            range(1, Some(5))
        );
        assert_eq!(command_lines("head -n 20 other.rs"), range(1, Some(20)));
        assert_eq!(command_lines("head -5 other.rs"), range(1, Some(5)));
        assert_eq!(command_lines("head other.rs"), range(1, Some(10)));
        assert_eq!(command_lines("cat notes.md"), None);
        assert_eq!(command_lines("tail -n 5 notes.md"), None);
        assert_eq!(command_lines("sed -n '9,3p' a.rs"), None);
        assert_eq!(command_lines("sed 's/a/b/' a.rs"), None);
    }

    #[test]
    fn a_plain_sed_read_is_found_where_codex_left_it_unknown() {
        assert_eq!(
            sed_read("sed -n '100,$p' src.rs"),
            Some(FileRead {
                path: "src.rs".into(),
                lines: range(100, None)
            })
        );
        assert_eq!(sed_read("sed -n '1,5p' a.rs > b.rs"), None);
        assert_eq!(sed_read("sed -i '' 's/a/b/' a.rs"), None);
    }

    #[test]
    fn a_search_names_the_files_its_output_shows() {
        let content = "src.rs:41:pub fn compute_answer()\n./other.rs:1:fn main()\n\
                       src.rs-40-// context\nlib/a-12-b.rs:3:x\nplain.rs:no numbers\nlisted.rs\n";
        assert_eq!(
            output_hits(content, SearchKind::Content),
            [
                "src.rs",
                "./other.rs",
                "lib/a-12-b.rs",
                "plain.rs",
                "listed.rs"
            ]
        );
        let listing = "total 16\n-rw-r--r--@ 1 user  staff  22 Oct  7 19:57 notes.md\nsrc.rs\n";
        assert_eq!(
            output_hits(listing, SearchKind::Files),
            ["notes.md", "src.rs"]
        );
    }
}
