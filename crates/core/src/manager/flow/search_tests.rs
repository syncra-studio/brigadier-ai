//! The right sidebar's Search across a checkout: what it reads and leaves out, how the
//! options match, its limits and its stop; and Replace all, which rewrites only what is
//! still as the search found it, and never outside the checkout.

use std::path::Path;

use super::{Flow, Options, Reply, git};
use crate::Error;
use crate::manager::search::{MAX_FILES, SearchRun};
use crate::work::{ReplaceFile, SearchFile, SearchQuery, SearchUpdate, WorkspaceTarget};

async fn flow(name: &str) -> (Flow, WorkspaceTarget) {
    let flow = Flow::start(
        name,
        Options::default(),
        std::sync::Arc::new(|_| Box::pin(async { Reply::text("Hello.") }) as _),
    )
    .await;
    let project_id = flow
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .project_id
        .expect("a session has a project");
    (flow, WorkspaceTarget::Project { project_id })
}

fn query(text: &str) -> SearchQuery {
    SearchQuery {
        text: text.into(),
        match_case: false,
        whole_word: false,
        regex: false,
        include: String::new(),
        exclude: String::new(),
    }
}

/// Every update of a search until it ends: the files found, and the closing counts.
async fn collect(mut run: SearchRun) -> (Vec<SearchFile>, (u32, u32, bool)) {
    let mut files = Vec::new();
    while let Some(update) = run.next().await {
        match update {
            SearchUpdate::Files { files: found } => files.extend(found),
            SearchUpdate::Done {
                files: count,
                matches,
                capped,
            } => return (files, (count, matches, capped)),
        }
    }
    panic!("the search ended without Done");
}

async fn found(flow: &Flow, target: &WorkspaceTarget, query: SearchQuery) -> Vec<(String, u32)> {
    let run = flow.manager.start_search(target, query).await.unwrap();
    let (files, _) = collect(run).await;
    files
        .into_iter()
        .map(|file| (file.path, file.matches))
        .collect()
}

fn write(root: &Path, path: &str, text: &str) {
    let full = root.join(path);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(full, text).unwrap();
}

/// What git ignores and `.git` stay out; dotfiles, untracked files and nested folders are in;
/// binary and very large files are left out; results come in path order.
#[tokio::test]
async fn search_reads_what_git_would_in_path_order() {
    let (flow, target) = flow("search-reads").await;
    let repo = flow.repo.clone();
    write(&repo, ".gitignore", "target/\n*.log\n");
    write(&repo, "target/out.txt", "needle\n");
    write(&repo, "build.log", "needle\n");
    write(&repo, ".env", "NEEDLE=1\n");
    write(&repo, "src/deep/a.rs", "needle needle\nno\nneedle\n");
    write(&repo, "b.txt", "a needle\n");
    std::fs::write(repo.join("binary.bin"), b"needle\0rest").unwrap();
    write(
        &repo,
        "big.txt",
        &format!("needle\n{}", "x".repeat(3 * 1024 * 1024)),
    );
    git(&repo, &["config", "needle.key", "1"]);

    let run = flow
        .manager
        .start_search(&target, query("needle"))
        .await
        .unwrap();
    let (files, (count, matches, capped)) = collect(run).await;
    let paths: Vec<_> = files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, [".env", "b.txt", "src/deep/a.rs"]);
    assert_eq!((count, matches, capped), (3, 5, false));
    let deep = &files[2];
    assert_eq!(
        deep.lines
            .iter()
            .map(|line| (line.line, line.ranges.len()))
            .collect::<Vec<_>>(),
        [(1, 2), (3, 1)],
        "matches by line, two on the first"
    );
    assert_eq!(deep.lines[0].preview, "needle needle");
    assert_eq!(
        deep.hash,
        blake3::hash(b"needle needle\nno\nneedle\n")
            .to_hex()
            .to_string()
    );
}

/// Match case, whole word and regular expressions; an invalid one fails at the start.
#[tokio::test]
async fn search_options_match_as_they_say() {
    let (flow, target) = flow("search-options").await;
    let repo = flow.repo.clone();
    write(&repo, "a.txt", "Foo foo food foo_bar\n");
    let case = SearchQuery {
        match_case: true,
        ..query("foo")
    };
    assert_eq!(found(&flow, &target, case).await, [("a.txt".to_owned(), 3)]);
    assert_eq!(
        found(&flow, &target, query("foo")).await,
        [("a.txt".to_owned(), 4)]
    );
    let word = SearchQuery {
        whole_word: true,
        ..query("foo")
    };
    assert_eq!(found(&flow, &target, word).await, [("a.txt".to_owned(), 2)]);
    let regex = SearchQuery {
        regex: true,
        ..query(r"fo+d|bar$")
    };
    assert_eq!(
        found(&flow, &target, regex).await,
        [("a.txt".to_owned(), 2)]
    );
    // Literal text is escaped.
    write(&repo, "b.txt", "a.b axb\n");
    assert_eq!(
        found(&flow, &target, query("a.b")).await,
        [("b.txt".to_owned(), 1)]
    );
    let invalid = SearchQuery {
        regex: true,
        ..query("(unclosed")
    };
    match flow.manager.start_search(&target, invalid).await {
        Err(Error::Invalid(text)) => assert!(text.contains("regular expression"), "{text}"),
        other => panic!("an invalid pattern is refused, not {:?}", other.map(|_| ())),
    }
    assert!(flow.manager.start_search(&target, query("")).await.is_err());
}

/// Files to include and to exclude: globs at any depth, anchored ones, folders.
#[tokio::test]
async fn search_keeps_to_files_to_include_and_exclude() {
    let (flow, target) = flow("search-globs").await;
    let repo = flow.repo.clone();
    for path in [
        "src/a.ts",
        "src/a.tsx",
        "src/gen/b.ts",
        "docs/c.ts",
        "web/docs/d.ts",
    ] {
        write(&repo, path, "hit\n");
    }
    let paths =
        |found: Vec<(String, u32)>| found.into_iter().map(|(path, _)| path).collect::<Vec<_>>();
    let include = SearchQuery {
        include: "*.ts".into(),
        ..query("hit")
    };
    assert_eq!(
        paths(found(&flow, &target, include).await),
        ["docs/c.ts", "src/a.ts", "src/gen/b.ts", "web/docs/d.ts"]
    );
    let both = SearchQuery {
        include: "src/**, /docs".into(),
        exclude: "gen, *.tsx".into(),
        ..query("hit")
    };
    assert_eq!(
        paths(found(&flow, &target, both).await),
        ["docs/c.ts", "src/a.ts"]
    );
    let bad = SearchQuery {
        include: "src/[".into(),
        ..query("hit")
    };
    assert!(flow.manager.start_search(&target, bad).await.is_err());
}

/// A search stops at its limits and says so; dropping it stops it early.
#[tokio::test]
async fn search_stops_at_its_limits_and_when_dropped() {
    let (flow, target) = flow("search-caps").await;
    let repo = flow.repo.clone();
    for index in 0..MAX_FILES + 5 {
        write(&repo, &format!("many/f{index:05}.txt"), "hit\n");
    }
    let run = flow
        .manager
        .start_search(&target, query("hit"))
        .await
        .unwrap();
    let (files, (count, _, capped)) = collect(run).await;
    assert!(capped);
    assert_eq!((files.len() as u32, count), (MAX_FILES, MAX_FILES));

    let mut run = flow
        .manager
        .start_search(&target, query("hit"))
        .await
        .unwrap();
    assert!(matches!(run.next().await, Some(SearchUpdate::Files { .. })));
    drop(run);
}

/// Replace all rewrites the matches of files still as searched, with `$1` groups in a
/// pattern (whole word keeps their numbers) and `$` as itself in literal text; a file changed
/// since is skipped and said so.
#[tokio::test]
async fn replace_rewrites_only_files_unchanged_since_the_search() {
    let (flow, target) = flow("search-replace").await;
    let repo = flow.repo.clone();
    write(&repo, "a.txt", "let foo_1 = foo_2;\nfoo_3x\n");
    let outside = repo.parent().unwrap().join("atomic-replace-outside.txt");
    std::fs::write(&outside, "keep\n").unwrap();
    let old_temp = repo.join(format!(".a.txt.replace-{}", std::process::id()));
    std::os::unix::fs::symlink(&outside, &old_temp).unwrap();
    write(&repo, "b.txt", "foo_9\n");
    write(&repo, "c.txt", "cost foo_1\n");
    let pattern = SearchQuery {
        regex: true,
        whole_word: true,
        match_case: true,
        ..query(r"foo_(\d)")
    };
    let run = flow
        .manager
        .start_search(&target, pattern.clone())
        .await
        .unwrap();
    let (files, _) = collect(run).await;
    let mut listed: Vec<ReplaceFile> = files
        .iter()
        .map(|file| ReplaceFile {
            path: file.path.clone(),
            hash: file.hash.clone(),
            matches: file.matches,
        })
        .collect();
    assert_eq!(
        listed.iter().map(|file| file.matches).collect::<Vec<_>>(),
        [2, 1, 1]
    );
    // b.txt changes after the search; c.txt keeps its bytes but is listed with a wrong count.
    write(&repo, "b.txt", "foo_9 foo_8\n");
    listed[2].matches = 5;
    let outcome = flow
        .manager
        .replace_in_files(&target, pattern, "bar_$1".into(), listed)
        .await
        .unwrap();
    assert_eq!((outcome.files, outcome.matches), (1, 2));
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "keep\n");
    assert!(
        std::fs::symlink_metadata(&old_temp)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    std::fs::remove_file(&old_temp).unwrap();
    std::fs::remove_file(&outside).unwrap();
    assert_eq!(
        outcome
            .skipped
            .iter()
            .map(|skip| skip.path.as_str())
            .collect::<Vec<_>>(),
        ["b.txt", "c.txt"]
    );
    assert!(outcome.skipped[0].reason.contains("changed"));
    assert_eq!(
        std::fs::read_to_string(repo.join("a.txt")).unwrap(),
        "let bar_1 = bar_2;\nfoo_3x\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("b.txt")).unwrap(),
        "foo_9 foo_8\n"
    );

    let run = flow
        .manager
        .start_search(&target, query("FOO_3"))
        .await
        .unwrap();
    let (files, _) = collect(run).await;
    let listed = vec![ReplaceFile {
        path: files[0].path.clone(),
        hash: files[0].hash.clone(),
        matches: 1,
    }];
    flow.manager
        .replace_in_files(&target, query("FOO_3"), "$1".into(), listed)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(repo.join("a.txt")).unwrap(),
        "let bar_1 = bar_2;\n$1x\n",
        "literal text replaces as written"
    );
}

/// Replace refuses paths outside the checkout, into `.git`, or through a link leading out;
/// and a session whose own copy doesn't exist yet isn't written at all.
#[tokio::test]
async fn replace_keeps_inside_the_checkout() {
    let (flow, target) = flow("search-replace-safety").await;
    let repo = flow.repo.clone();
    let outside = repo.parent().unwrap().join("search-replace-outside.txt");
    std::fs::write(&outside, "hit\n").unwrap();
    let hash = blake3::hash(b"hit\n").to_hex().to_string();
    std::os::unix::fs::symlink(&outside, repo.join("link.txt")).unwrap();
    write(&repo, ".git/hit.txt", "hit\n");
    write(&repo, ".gitignore", "ignored/\n");
    write(&repo, "ignored/private.txt", "hit\n");
    write(&repo, "excluded.txt", "hit\n");
    std::os::unix::fs::symlink(repo.join(".git"), repo.join("git-alias")).unwrap();
    let paths = [
        "../search-replace-outside.txt".to_owned(),
        outside.to_string_lossy().into_owned(),
        ".git/hit.txt".to_owned(),
        "link.txt".to_owned(),
        "missing.txt".to_owned(),
        "ignored/private.txt".to_owned(),
        "excluded.txt".to_owned(),
        "git-alias/hit.txt".to_owned(),
    ];
    let listed = paths
        .iter()
        .map(|path| ReplaceFile {
            path: path.clone(),
            hash: hash.clone(),
            matches: 1,
        })
        .collect();
    let outcome = flow
        .manager
        .replace_in_files(
            &target,
            SearchQuery {
                exclude: "excluded.txt".into(),
                ..query("hit")
            },
            "x".into(),
            listed,
        )
        .await
        .unwrap();
    assert_eq!(outcome.files, 0);
    assert_eq!(outcome.skipped.len(), paths.len());
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "hit\n");
    assert_eq!(
        std::fs::read_to_string(repo.join("ignored/private.txt")).unwrap(),
        "hit\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("excluded.txt")).unwrap(),
        "hit\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo.join(".git/hit.txt")).unwrap(),
        "hit\n"
    );
    std::fs::remove_file(&outside).unwrap();

    // The session makes its own copy on its first message: until then its Search reads the
    // repository, and Replace writes nothing.
    write(&repo, "a.txt", "hit\n");
    let session = WorkspaceTarget::Conversation {
        conversation_id: flow.conversation.clone(),
    };
    let run = flow
        .manager
        .start_search(&session, query("hit"))
        .await
        .unwrap();
    let (files, _) = collect(run).await;
    let listed = files
        .iter()
        .map(|file| ReplaceFile {
            path: file.path.clone(),
            hash: file.hash.clone(),
            matches: file.matches,
        })
        .collect();
    assert!(
        flow.manager
            .replace_in_files(&session, query("hit"), "x".into(), listed)
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("a.txt")).unwrap(),
        "hit\n"
    );
}
