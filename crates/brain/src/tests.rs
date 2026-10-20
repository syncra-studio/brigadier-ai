//! The Brain's write path: history instead of overwrites, same-content folding, ordered
//! revival, and stale nodes not leading answers.

use super::*;

/// A Brain in its own temporary folder, removed on drop. No embedding model: retrieval is
/// full-text only.
struct TestBrain {
    brain: Brain,
    /// Last, so it is removed once the Brain is closed.
    _dir: Removed,
}

impl TestBrain {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("brain-test-{}", uuid::Uuid::now_v7()));
        let embedder = Embedder::new(dir.join("models"));
        let brain = Brain::open(&dir.join("brain.sqlite"), Scope::Project, embedder).unwrap();
        Self {
            brain,
            _dir: Removed(dir),
        }
    }
}

/// A folder removed when dropped. The Brain's thread closes its database just after the Brain
/// is dropped, and Windows removes no file still open: there it takes a few tries.
struct Removed(PathBuf);

impl Drop for Removed {
    fn drop(&mut self) {
        let tries = if cfg!(windows) { 100 } else { 1 };
        for _ in 1..tries {
            if std::fs::remove_dir_all(&self.0).is_ok() || !self.0.exists() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl std::ops::Deref for TestBrain {
    type Target = Brain;
    fn deref(&self) -> &Brain {
        &self.brain
    }
}

fn provenance(session: Option<&str>) -> Provenance {
    Provenance {
        origin: Origin::Enrichment,
        session_id: session.map(str::to_owned),
        task_id: None,
        job_id: None,
        worker: None,
        commit: None,
        recorded_at_ms: 1,
        claim: None,
    }
}

fn node(kind: NodeKind, key: Option<&str>, title: &str, body: &str) -> NewNode {
    NewNode {
        kind,
        key: key.map(str::to_owned),
        title: title.into(),
        body: body.into(),
        provenance: provenance(None),
        files: Vec::new(),
        expires_at_ms: None,
    }
}

fn decision(session: &str, key: &str, title: &str) -> NewNode {
    NewNode {
        provenance: provenance(Some(session)),
        ..node(NodeKind::Decision, Some(key), title, "")
    }
}

fn all(brain: &Brain) -> Vec<Node> {
    brain.nodes(&NodeFilter::default()).unwrap()
}

fn superseded_by(node: &Node) -> Option<&str> {
    match &node.state {
        NodeState::Superseded { by, .. } => Some(by),
        _ => None,
    }
}

fn query(brain: &Brain, text: &str, history: bool) -> Vec<BrainHit> {
    brain
        .query(&BrainQuery {
            text: text.into(),
            history,
            ..BrainQuery::default()
        })
        .unwrap()
        .hits
}

#[test]
fn matching_history_does_not_use_up_the_current_result_limit() {
    let brain = TestBrain::new();
    let mut rule = String::new();
    for revision in 0..16 {
        rule = brain
            .record(node(
                NodeKind::Convention,
                Some("convention:history"),
                "Needle",
                &format!("revision {revision}"),
            ))
            .unwrap();
    }
    let module = brain
        .record(node(
            NodeKind::Module,
            Some("module:needle"),
            "Module",
            "Needle",
        ))
        .unwrap();
    let answer = brain
        .query(&BrainQuery {
            text: "Needle".into(),
            limit: Some(12),
            ..BrainQuery::default()
        })
        .unwrap();
    assert_eq!(answer.hits.len(), 2);
    assert!(answer.hits.iter().any(|hit| hit.node.id == rule));
    assert!(answer.hits.iter().any(|hit| hit.node.id == module));
}

#[test]
fn a_rewritten_rule_keeps_its_earlier_text_as_history() {
    let brain = TestBrain::new();
    let first = node(
        NodeKind::Convention,
        Some("convention:x"),
        "Stack: Tauri 1",
        "old",
    );
    let id = brain.record(first.clone()).unwrap();
    // The same text again changes nothing.
    assert_eq!(brain.record(first).unwrap(), id);
    assert_eq!(all(&brain).len(), 1);

    let second = node(
        NodeKind::Convention,
        Some("convention:x"),
        "Stack: Tauri 2",
        "new",
    );
    assert_eq!(
        brain.record(second).unwrap(),
        id,
        "the current node keeps its id"
    );
    let nodes = all(&brain);
    assert_eq!(nodes.len(), 2);
    let current = nodes.iter().find(|node| node.id == id).unwrap();
    assert_eq!(current.title, "Stack: Tauri 2");
    assert_eq!(current.state, NodeState::Fresh);
    let old = nodes.iter().find(|node| node.id != id).unwrap();
    assert_eq!(old.title, "Stack: Tauri 1");
    assert_eq!(old.key.as_deref(), Some("convention:x"));
    match &old.state {
        NodeState::Superseded {
            by,
            reason,
            since_ms,
        } => {
            assert_eq!(by, &id);
            assert_eq!(reason.as_deref(), Some("rewritten by enrichment"));
            assert!(since_ms.is_some());
        }
        other => panic!("not superseded: {other:?}"),
    }
    assert_eq!(
        brain
            .node_by_key(NodeKind::Convention, "convention:x")
            .unwrap()
            .unwrap()
            .id,
        id
    );
    assert!(
        brain
            .edges(std::slice::from_ref(&id))
            .unwrap()
            .contains(&Edge {
                from: id.clone(),
                to: old.id.clone(),
                kind: EdgeKind::Supersedes,
            })
    );

    // What holds now answers; the earlier text only when history is asked for.
    let hits = query(&brain, "Tauri", false);
    assert_eq!(
        hits.iter()
            .map(|hit| hit.node.id.as_str())
            .collect::<Vec<_>>(),
        [id.as_str()]
    );
    let hits = query(&brain, "Tauri", true);
    assert_eq!(hits.len(), 2);
    let text = brain
        .query(&BrainQuery {
            text: "Tauri".into(),
            history: true,
            ..BrainQuery::default()
        })
        .unwrap()
        .text;
    assert!(
        text.contains(&format!("SUPERSEDED by {id}: rewritten by enrichment")),
        "{text}"
    );
}

#[test]
fn summaries_are_rewritten_without_history() {
    let brain = TestBrain::new();
    let key = Some("file:README.md");
    brain
        .record(node(NodeKind::FileSummary, key, "README", "old"))
        .unwrap();
    brain
        .record(node(NodeKind::FileSummary, key, "README", "new"))
        .unwrap();
    assert_eq!(all(&brain).len(), 1);
}

#[test]
fn history_stays_one_line() {
    let brain = TestBrain::new();
    let key = Some("convention:x");
    let id = brain
        .record(node(NodeKind::Convention, key, "v1", ""))
        .unwrap();
    brain
        .record(node(NodeKind::Convention, key, "v2", ""))
        .unwrap();
    brain
        .record(node(NodeKind::Convention, key, "v3", ""))
        .unwrap();
    let nodes = all(&brain);
    let by_title = |title: &str| nodes.iter().find(|node| node.title == title).unwrap();
    assert_eq!(by_title("v3").id, id);
    assert_eq!(superseded_by(by_title("v2")), Some(id.as_str()));
    assert_eq!(
        superseded_by(by_title("v1")),
        Some(by_title("v2").id.as_str())
    );
}

#[test]
fn rules_with_the_same_content_fold_but_not_across_symbols_or_sessions() {
    let brain = TestBrain::new();
    let id = brain
        .record(decision("s1", "d:1", "Use C++ for the engine"))
        .unwrap();
    assert_eq!(
        brain
            .record(NewNode {
                key: None,
                ..decision("s1", "d:2", "use c++ for the engine!")
            })
            .unwrap(),
        id,
        "the same words fold"
    );
    assert_ne!(
        brain
            .record(decision("s1", "d:3", "Use C# for the engine"))
            .unwrap(),
        id
    );
    assert_ne!(
        brain
            .record(decision("s2", "d:4", "Use C++ for the engine"))
            .unwrap(),
        id
    );
    assert_eq!(all(&brain).len(), 3);
    // Conventions fold project-wide, whatever the kind of rule.
    let rule = brain
        .record(node(
            NodeKind::Convention,
            Some("convention:a"),
            "Tabs, not spaces",
            "Always.",
        ))
        .unwrap();
    assert_eq!(
        brain
            .record(node(NodeKind::Contract, None, "tabs not spaces", "always"))
            .unwrap(),
        rule
    );
}

#[test]
fn normalizing_keeps_what_changes_a_meaning() {
    assert_eq!(write::fold_key(NodeKind::Module, None, "a", "b"), None);
    let key = |title: &str| write::fold_key(NodeKind::Convention, None, title, "").unwrap();
    assert_eq!(key("Node >= 20, v1.2"), key("node >= 20 v1.2."));
    assert_ne!(key("Node >= 20"), key("Node 20"));
    assert_ne!(key("v1.2"), key("v12"));
    assert_ne!(key("a-b"), key("a b"));
    assert_eq!(key("ends-"), key("ends"));
}

#[test]
fn equivalent_keyed_rules_keep_their_own_identity_and_history() {
    let brain = TestBrain::new();
    let first = brain
        .record(node(
            NodeKind::Convention,
            Some("rule:first"),
            "Tabs",
            "Always",
        ))
        .unwrap();
    let second = brain
        .record(node(
            NodeKind::Convention,
            Some("rule:second"),
            "Tabs",
            "Always",
        ))
        .unwrap();
    assert_ne!(
        first, second,
        "distinct keys cannot be folded without aliases"
    );
    assert_eq!(
        brain
            .node_by_key(NodeKind::Convention, "rule:second")
            .unwrap()
            .unwrap()
            .id,
        second
    );
    brain
        .link(vec![NewEdge {
            from: "key:rule:first".into(),
            to: "key:rule:second".into(),
            kind: EdgeKind::Relates,
        }])
        .unwrap();
    assert!(
        brain
            .edges(std::slice::from_ref(&second))
            .unwrap()
            .contains(&Edge {
                from: first.clone(),
                to: second.clone(),
                kind: EdgeKind::Relates
            })
    );
    assert_eq!(
        brain
            .record(node(
                NodeKind::Convention,
                Some("rule:second"),
                "Spaces",
                "Always"
            ))
            .unwrap(),
        second
    );
    let nodes = all(&brain);
    assert_eq!(
        nodes
            .iter()
            .filter(|node| !matches!(node.state, NodeState::Superseded { .. }))
            .count(),
        2
    );
    assert!(
        nodes
            .iter()
            .any(|node| node.title == "Tabs" && superseded_by(node) == Some(second.as_str()))
    );
    assert_eq!(brain.node(&first).unwrap().unwrap().title, "Tabs");
    // The open-time backfill must preserve keyed identities too.
    brain
        .write(|tx, changes| {
            tx.execute(
                "UPDATE nodes SET title = 'Tabs', fold_key = NULL WHERE state != 'superseded'",
                [],
            )?;
            assert_eq!(write::settle_rules(tx, 5, changes)?, 0);
            Ok(())
        })
        .unwrap();
    assert_eq!(
        brain
            .node_by_key(NodeKind::Convention, "rule:first")
            .unwrap()
            .unwrap()
            .id,
        first
    );
    assert_eq!(
        brain
            .node_by_key(NodeKind::Convention, "rule:second")
            .unwrap()
            .unwrap()
            .id,
        second
    );
}

#[test]
fn a_replacement_under_another_title_supersedes_the_old_node() {
    let brain = TestBrain::new();
    let module = brain
        .record(node(
            NodeKind::Module,
            Some("module:core"),
            "core",
            "the core",
        ))
        .unwrap();
    let old = brain
        .record(node(
            NodeKind::Convention,
            Some("convention:old"),
            "Old rule",
            "x",
        ))
        .unwrap();
    brain
        .link(vec![Edge {
            from: old.clone(),
            to: module.clone(),
            kind: EdgeKind::About,
        }])
        .unwrap();
    let new = brain
        .record_replacing(
            node(
                NodeKind::Convention,
                Some("convention:new"),
                "New rule",
                "y",
            ),
            "key:convention:old",
            "replaced by the enrichment job j1",
        )
        .unwrap();
    let old_node = brain.node(&old).unwrap().unwrap();
    assert_eq!(
        old_node.state,
        NodeState::Superseded {
            by: new.clone(),
            reason: Some("replaced by the enrichment job j1".into()),
            since_ms: match old_node.state {
                NodeState::Superseded { since_ms, .. } => since_ms,
                _ => None,
            },
        }
    );
    let edges = brain.edges(std::slice::from_ref(&new)).unwrap();
    assert!(edges.contains(&Edge {
        from: new.clone(),
        to: module,
        kind: EdgeKind::About
    }));
    assert!(edges.contains(&Edge {
        from: new.clone(),
        to: old.clone(),
        kind: EdgeKind::Supersedes
    }));
    // An unknown node to replace fails the whole write.
    assert!(
        brain
            .record_replacing(
                node(NodeKind::Convention, None, "Other", "z"),
                "key:nope",
                "r"
            )
            .is_err()
    );
    assert_eq!(all(&brain).len(), 3);
    // Replacing itself (the same key) is a rewrite, not an error.
    let same = brain
        .record_replacing(
            node(
                NodeKind::Convention,
                Some("convention:new"),
                "New rule",
                "y2",
            ),
            "key:convention:new",
            "r",
        )
        .unwrap();
    assert_eq!(same, new);
}

#[test]
fn deleting_the_current_version_revives_only_the_latest_with_its_freshness() {
    let brain = TestBrain::new();
    let key = Some("convention:x");
    let file = |hash: &str| FileRef {
        path: "a.rs".into(),
        hash: Some(hash.into()),
    };
    brain
        .record(node(NodeKind::Convention, key, "v1", ""))
        .unwrap();
    let mut v2 = node(NodeKind::Convention, key, "v2", "");
    v2.files = vec![file("h1")];
    let id = brain.record(v2).unwrap();
    // v2 goes stale, then v3 rewrites it.
    assert_eq!(
        brain.files_changed(&[file("h2")]).unwrap(),
        vec![id.clone()]
    );
    brain
        .record(node(NodeKind::Convention, key, "v3", ""))
        .unwrap();

    brain.delete(&id).unwrap();
    let nodes = all(&brain);
    assert_eq!(nodes.len(), 2);
    let v2 = nodes.iter().find(|node| node.title == "v2").unwrap();
    assert!(
        matches!(&v2.state, NodeState::Stale { reason, .. } if reason == "a.rs changed"),
        "{:?}",
        v2.state
    );
    let v1 = nodes.iter().find(|node| node.title == "v1").unwrap();
    assert_eq!(superseded_by(v1), Some(v2.id.as_str()));
    assert_eq!(
        brain
            .node_by_key(NodeKind::Convention, "convention:x")
            .unwrap()
            .unwrap()
            .id,
        v2.id
    );
}

#[test]
fn a_changed_file_marks_history_quietly() {
    let brain = TestBrain::new();
    let key = Some("convention:x");
    let mut v1 = node(NodeKind::Convention, key, "v1", "");
    v1.files = vec![FileRef {
        path: "a.rs".into(),
        hash: Some("h1".into()),
    }];
    let id = brain.record(v1).unwrap();
    brain
        .record(node(NodeKind::Convention, key, "v2", ""))
        .unwrap();
    let changed = FileRef {
        path: "a.rs".into(),
        hash: Some("h2".into()),
    };
    assert!(brain.files_changed(&[changed]).unwrap().is_empty());
    // Current again, it is as stale as the file made it.
    brain.delete(&id).unwrap();
    let v1 = all(&brain).pop().unwrap();
    assert!(
        matches!(v1.state, NodeState::Stale { .. }),
        "{:?}",
        v1.state
    );
}

#[test]
fn a_superseded_node_held_by_another_key_stays_history() {
    let brain = TestBrain::new();
    let a = brain
        .record(node(NodeKind::Decision, Some("d:a"), "A", ""))
        .unwrap();
    let b = brain
        .record(node(NodeKind::Decision, Some("d:b"), "B", ""))
        .unwrap();
    brain.supersede(&a, &b, "changed").unwrap();
    // A newer node holds a's key by now.
    let c = brain
        .record(node(NodeKind::Decision, Some("d:a"), "C", ""))
        .unwrap();
    brain.delete(&b).unwrap();
    let a_node = brain.node(&a).unwrap().unwrap();
    assert_eq!(superseded_by(&a_node), Some(c.as_str()));
}

#[test]
fn opening_folds_rules_recorded_twice_before() {
    let brain = TestBrain::new();
    let one = brain
        .record(node(NodeKind::Convention, None, "Rule", "x"))
        .unwrap();
    let two = brain
        .record(node(NodeKind::Convention, None, "Other", "y"))
        .unwrap();
    // As an older Brain held them: the same rule twice, without same-content keys.
    let stale = one.clone();
    brain
        .write(move |tx, _| {
            tx.execute(
                "UPDATE nodes SET title = 'Rule', body = 'x', fold_key = NULL",
                [],
            )?;
            tx.execute(
                "UPDATE nodes SET state = 'stale', stale_reason = 'r', stale_since_ms = 1 \
                 WHERE id = ?1",
                [&stale],
            )?;
            Ok(())
        })
        .unwrap();
    let folded = brain
        .write(|tx, changes| write::settle_rules(tx, 5, changes))
        .unwrap();
    assert_eq!(folded, 1);
    // The fresh one stays.
    assert_eq!(
        superseded_by(&brain.node(&one).unwrap().unwrap()),
        Some(two.as_str())
    );
    assert_eq!(brain.node(&two).unwrap().unwrap().state, NodeState::Fresh);
}

#[test]
fn a_stale_node_does_not_lead_a_fresh_match() {
    let brain = TestBrain::new();
    let mut stale = node(
        NodeKind::FileSummary,
        Some("file:README.md"),
        "Router overview",
        "router",
    );
    stale.files = vec![FileRef {
        path: "README.md".into(),
        hash: Some("h1".into()),
    }];
    let stale = brain.record(stale).unwrap();
    let fresh = brain
        .record(node(
            NodeKind::Module,
            Some("module:router"),
            "routing",
            "the query router",
        ))
        .unwrap();
    let before: Vec<String> = query(&brain, "router overview", false)
        .into_iter()
        .map(|hit| hit.node.id)
        .collect();
    assert_eq!(before.first(), Some(&stale));
    brain
        .files_changed(&[FileRef {
            path: "README.md".into(),
            hash: Some("h2".into()),
        }])
        .unwrap();
    let after: Vec<String> = query(&brain, "router overview", false)
        .into_iter()
        .map(|hit| hit.node.id)
        .collect();
    assert_eq!(after, [fresh, stale]);
}

#[test]
fn an_answer_says_what_did_not_fit() {
    let brain = TestBrain::new();
    for index in 0..6 {
        brain
            .record(node(
                NodeKind::Module,
                Some(&format!("module:m{index}")),
                &format!("module {index}"),
                &"words about the module ".repeat(20),
            ))
            .unwrap();
    }
    let answer = brain
        .query(&BrainQuery {
            text: "module".into(),
            max_tokens: Some(200),
            ..BrainQuery::default()
        })
        .unwrap();
    assert!(
        answer.text.contains("more hit(s) didn't fit"),
        "{}",
        answer.text
    );
}

fn from(origin: Origin, session: Option<&str>, task: Option<&str>, mut node: NewNode) -> NewNode {
    node.provenance.origin = origin;
    node.provenance.session_id = session.map(str::to_owned);
    node.provenance.task_id = task.map(str::to_owned);
    node
}

#[test]
fn forgetting_a_session_keeps_what_another_one_supports() {
    let brain = TestBrain::new();
    let rule = || node(NodeKind::Convention, Some("convention:x"), "Tabs", "always");
    let shared = brain
        .record(from(Origin::Orchestrator, Some("s1"), None, rule()))
        .unwrap();
    assert_eq!(
        brain
            .record(from(Origin::Orchestrator, Some("s2"), None, rule()))
            .unwrap(),
        shared
    );
    let sole = brain
        .record(from(
            Origin::Orchestrator,
            Some("s1"),
            None,
            node(NodeKind::Decision, None, "Only s1", ""),
        ))
        .unwrap();
    // Both conversations see the shared rule as theirs.
    let of = |session: &str| -> Vec<String> {
        brain
            .nodes(&NodeFilter {
                session_id: Some(session.into()),
                ..NodeFilter::default()
            })
            .unwrap()
            .into_iter()
            .map(|node| node.id)
            .collect()
    };
    assert_eq!(of("s2"), std::slice::from_ref(&shared));
    assert_eq!(of("s1").len(), 2);

    assert_eq!(brain.forget_session("s2").unwrap(), 0);
    assert!(brain.node(&shared).unwrap().is_some());
    // s2 recorded it last; s1 is what supports it now.
    assert_eq!(brain.forget_session("s1").unwrap(), 2);
    assert!(brain.node(&sole).unwrap().is_none());
    assert!(brain.node(&shared).unwrap().is_none());
}

#[test]
fn a_node_shows_its_latest_remaining_source() {
    let brain = TestBrain::new();
    let rule = || node(NodeKind::Convention, Some("convention:x"), "Tabs", "always");
    let mut first = from(Origin::Orchestrator, Some("s1"), None, rule());
    first.provenance.commit = Some("abc".into());
    let id = brain.record(first.clone()).unwrap();
    brain
        .record(from(Origin::Orchestrator, Some("s2"), None, rule()))
        .unwrap();
    assert_eq!(
        brain
            .node(&id)
            .unwrap()
            .unwrap()
            .provenance
            .session_id
            .as_deref(),
        Some("s2")
    );
    brain.forget_session("s2").unwrap();
    assert_eq!(
        brain.node(&id).unwrap().unwrap().provenance,
        first.provenance
    );
}

#[test]
fn forgetting_a_confirmation_removes_only_its_file_evidence() {
    // Exercise both keyed confirmation and same-content folding.
    for key in [Some("convention:files"), None] {
        let brain = TestBrain::new();
        let recording = |session: &str, path: &str| {
            let mut rule = node(NodeKind::Convention, key, "File evidence", "Keep it");
            rule.provenance = provenance(Some(session));
            rule.files = vec![FileRef {
                path: path.into(),
                hash: Some("h1".into()),
            }];
            rule
        };
        let id = brain.record(recording("s1", "a.rs")).unwrap();
        assert_eq!(brain.record(recording("s2", "b.rs")).unwrap(), id);
        assert_eq!(brain.node(&id).unwrap().unwrap().files.len(), 2);
        assert_eq!(brain.forget_session("s2").unwrap(), 0);
        let files = brain.node(&id).unwrap().unwrap().files;
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "a.rs");
        assert!(
            brain
                .files_changed(&[FileRef {
                    path: "b.rs".into(),
                    hash: None
                }])
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            brain
                .files_changed(&[FileRef {
                    path: "a.rs".into(),
                    hash: Some("h2".into())
                }])
                .unwrap(),
            [id]
        );
    }
}

#[test]
fn renewing_a_source_replaces_only_its_own_files() {
    let brain = TestBrain::new();
    let recording = |session: &str, path: &str| {
        let mut rule = node(
            NodeKind::Convention,
            Some("convention:files"),
            "Evidence",
            "",
        );
        rule.provenance = provenance(Some(session));
        rule.files = vec![FileRef {
            path: path.into(),
            hash: Some("h1".into()),
        }];
        rule
    };
    let id = brain.record(recording("s1", "old.rs")).unwrap();
    brain.record(recording("s2", "retained.rs")).unwrap();
    brain.record(recording("s1", "new.rs")).unwrap();
    let paths: Vec<String> = brain
        .node(&id)
        .unwrap()
        .unwrap()
        .files
        .into_iter()
        .map(|file| file.path)
        .collect();
    assert_eq!(paths, ["new.rs", "retained.rs"]);
    brain.forget_session("s1").unwrap();
    assert_eq!(
        brain.node(&id).unwrap().unwrap().files[0].path,
        "retained.rs"
    );
}

#[test]
fn forgetting_an_origin_keeps_what_another_confirmed() {
    let brain = TestBrain::new();
    let module = || node(NodeKind::Module, Some("module:core"), "core", "the core");
    let id = brain
        .record(from(Origin::Index, None, None, module()))
        .unwrap();
    brain
        .record(from(Origin::Enrichment, None, None, module()))
        .unwrap();
    // Rewritten by enrichment: only enrichment supports the new text.
    let other = brain
        .record(from(
            Origin::Index,
            None,
            None,
            node(NodeKind::Module, Some("module:ui"), "ui", "a"),
        ))
        .unwrap();
    brain
        .record(from(
            Origin::Enrichment,
            None,
            None,
            node(NodeKind::Module, Some("module:ui"), "ui", "b"),
        ))
        .unwrap();
    assert!(brain.holds_origin(Origin::Index).unwrap());
    assert_eq!(brain.forget_origins(&[Origin::Index]).unwrap(), 0);
    assert!(!brain.holds_origin(Origin::Index).unwrap());
    assert_eq!(brain.forget_origins(&[Origin::Enrichment]).unwrap(), 2);
    assert!(brain.node(&id).unwrap().is_none());
    assert!(brain.node(&other).unwrap().is_none());
}

#[test]
fn forgetting_the_source_of_a_rewrite_brings_back_the_version_before() {
    let brain = TestBrain::new();
    let key = Some("convention:x");
    let id = brain
        .record(from(
            Origin::Orchestrator,
            Some("s1"),
            None,
            node(NodeKind::Convention, key, "v1", ""),
        ))
        .unwrap();
    brain
        .record(from(
            Origin::Orchestrator,
            Some("s2"),
            None,
            node(NodeKind::Convention, key, "v2", ""),
        ))
        .unwrap();
    assert_eq!(brain.forget_session("s2").unwrap(), 1);
    let current = brain
        .node_by_key(NodeKind::Convention, "convention:x")
        .unwrap()
        .unwrap();
    assert_eq!(current.title, "v1");
    assert_ne!(current.id, id);
    assert_eq!(current.state, NodeState::Fresh);
    assert_eq!(all(&brain).len(), 1);
}

#[test]
fn refreshing_a_task_drops_only_what_it_alone_no_longer_says() {
    let brain = TestBrain::new();
    let report = |body: &str| {
        from(
            Origin::Report,
            Some("s1"),
            Some("t1"),
            node(NodeKind::Report, Some("task:t1"), "task-1 report", body),
        )
    };
    let part = |index: u32| {
        from(
            Origin::Report,
            Some("s1"),
            Some("t1"),
            node(
                NodeKind::Report,
                Some(&format!("task:t1:part:{index}")),
                "part",
                &format!("part {index}"),
            ),
        )
    };
    let decision = |task: &str, text: &str| {
        from(
            Origin::Report,
            Some("s1"),
            Some(task),
            node(
                NodeKind::Decision,
                Some(&format!("task:{task}:{text}")),
                text,
                "",
            ),
        )
    };
    let edges = |ids: &[String]| {
        ids[1..]
            .iter()
            .map(|id| Edge {
                from: ids[0].clone(),
                to: id.clone(),
                kind: EdgeKind::Contains,
            })
            .collect::<Vec<_>>()
    };
    let first = brain
        .refresh_task(
            "t1",
            vec![
                report("v1"),
                part(1),
                part(2),
                decision("t1", "Use A"),
                decision("t1", "Use B"),
            ],
            edges,
        )
        .unwrap();
    // Another task of the same conversation made the same decision.
    let shared = brain
        .record(NewNode {
            key: None,
            ..decision("t2", "Use B")
        })
        .unwrap();
    assert_eq!(shared, first[4]);
    assert_eq!(all(&brain).len(), 5);

    let second = brain
        .refresh_task(
            "t1",
            vec![report("v2"), part(1), decision("t1", "Use A")],
            edges,
        )
        .unwrap();
    assert_eq!(
        second,
        [first[0].clone(), first[1].clone(), first[3].clone()]
    );
    let ids: Vec<String> = all(&brain).into_iter().map(|node| node.id).collect();
    assert!(!ids.contains(&first[2]), "the surplus part goes");
    assert!(ids.contains(&first[4]), "t2 still supports Use B");
    assert_eq!(
        brain
            .node(&first[4])
            .unwrap()
            .unwrap()
            .provenance
            .task_id
            .as_deref(),
        Some("t2")
    );
    assert_eq!(ids.len(), 4);
}

#[test]
fn capped_answers_page_each_kind_and_say_what_is_left() {
    let brain = TestBrain::new();
    for index in 0..5 {
        brain
            .record(node(
                NodeKind::Convention,
                Some(&format!("convention:{index}")),
                &format!("router rule {index}"),
                "about the router",
            ))
            .unwrap();
        brain
            .record(node(
                NodeKind::Module,
                Some(&format!("module:{index}")),
                &format!("router module {index}"),
                "the router",
            ))
            .unwrap();
    }
    let caps = BrainCaps {
        facts: 2,
        entities: 1,
        passages: 1,
        body: 200,
    };
    let ask = |page| {
        brain
            .query(&BrainQuery {
                text: "router".into(),
                caps: Some(caps),
                page,
                ..BrainQuery::default()
            })
            .unwrap()
    };
    let first = ask(None);
    let count = |answer: &BrainAnswer, kind| {
        answer
            .hits
            .iter()
            .filter(|hit| hit.node.kind == kind)
            .count()
    };
    assert_eq!(count(&first, NodeKind::Convention), 2);
    assert_eq!(count(&first, NodeKind::Module), 1);
    assert!(
        first.text.ends_with(
            "3 more facts, 4 more modules and files; ask with page 2 or a narrower question."
        ),
        "{}",
        first.text
    );
    let second = ask(Some(2));
    assert_eq!(count(&second, NodeKind::Convention), 2);
    let seen: HashSet<&str> = first.hits.iter().map(|hit| hit.node.id.as_str()).collect();
    assert!(
        second
            .hits
            .iter()
            .all(|hit| !seen.contains(hit.node.id.as_str()))
    );
    let last = ask(Some(5));
    assert_eq!(count(&last, NodeKind::Module), 1);
    assert!(!last.text.contains("more"), "{}", last.text);
}

/// BUILD-PLAN A11: a claim that says what the user decided folds into the user's node as one
/// more source; the node stays the user's decision. Answers list settled facts before claims
/// and label each.
#[test]
fn a_claim_confirming_a_user_decision_keeps_it_the_users_and_claims_come_last() {
    let brain = TestBrain::new();
    let said = |origin: Origin, claim: Option<Claim>| Provenance {
        origin,
        claim,
        ..provenance(Some("s1"))
    };
    let user = brain
        .record(NewNode {
            provenance: said(Origin::User, None),
            ..node(NodeKind::Decision, None, "Ship tabs in the Makefile", "")
        })
        .unwrap();
    let again = brain
        .record(NewNode {
            provenance: said(Origin::Orchestrator, Some(Claim::unverified())),
            ..node(NodeKind::Decision, None, "Ship tabs in the Makefile", "")
        })
        .unwrap();
    assert_eq!(again, user);
    let kept = brain.node(&user).unwrap().unwrap();
    assert_eq!(source_label(kept.kind, &kept.provenance), "user decision");
    let claim = brain
        .record(NewNode {
            provenance: said(Origin::Report, Some(Claim::unverified())),
            ..node(NodeKind::Decision, None, "Ship spaces in the Makefile", "")
        })
        .unwrap();
    let answer = brain
        .query(&BrainQuery {
            text: "Makefile".into(),
            ..BrainQuery::default()
        })
        .unwrap();
    let order: Vec<&str> = answer.hits.iter().map(|hit| hit.node.id.as_str()).collect();
    assert_eq!(order, [user.as_str(), claim.as_str()]);
    assert!(
        answer
            .text
            .contains("· fresh · worker claim, unverified] Ship spaces in the Makefile"),
        "{}",
        answer.text
    );
    assert!(
        answer
            .text
            .contains("· fresh · user decision] Ship tabs in the Makefile"),
        "{}",
        answer.text
    );
}

fn transcript_entry(stream: TranscriptStream, seq: i64, role: &str, text: &str) -> TranscriptEntry {
    TranscriptEntry {
        conversation_id: "c1".into(),
        stream,
        seq,
        role: role.into(),
        request_id: None,
        at_ms: seq,
        text: text.into(),
    }
}

/// The conversation and its thread number their events on their own: a message and a thread
/// command with the same seq are both kept and found, each stream keeps its own watermark,
/// and a hit carries its entry's first line.
#[test]
fn a_message_and_a_thread_command_with_the_same_seq_are_both_found() {
    let brain = TestBrain::new();
    brain
        .index_transcript(vec![
            transcript_entry(
                TranscriptStream::Conversation,
                12,
                "user",
                "Check the quokka build.",
            ),
            transcript_entry(
                TranscriptStream::Thread,
                12,
                "command",
                "Ran `make quokka` (exit 2)\nquokka: missing target",
            ),
            transcript_entry(
                TranscriptStream::Thread,
                30,
                "read",
                "Read src/a.rs\nfn a() {}",
            ),
        ])
        .unwrap();
    // Indexing again changes nothing.
    brain
        .index_transcript(vec![transcript_entry(
            TranscriptStream::Thread,
            12,
            "command",
            "Ran `make quokka` (exit 2)\nquokka: missing target",
        )])
        .unwrap();
    let mut hits = brain.search_transcript("c1", "quokka", 10).unwrap();
    hits.sort_by_key(|hit| hit.stream.as_str());
    assert_eq!(hits.len(), 2, "{hits:?}");
    assert_eq!(
        (hits[0].stream, hits[0].seq, hits[0].role.as_str()),
        (TranscriptStream::Conversation, 12, "user")
    );
    assert_eq!(
        (hits[1].stream, hits[1].seq, hits[1].role.as_str()),
        (TranscriptStream::Thread, 12, "command")
    );
    assert_eq!(hits[1].lead, "Ran `make quokka` (exit 2)");
    assert_eq!(
        brain
            .transcript_watermark("c1", TranscriptStream::Conversation)
            .unwrap(),
        Some(12)
    );
    assert_eq!(
        brain
            .transcript_watermark("c1", TranscriptStream::Thread)
            .unwrap(),
        Some(30)
    );
    assert_eq!(brain.stats().unwrap().transcript_entries, 3);
    // A page read through seq 50 that added nothing still moves its stream's watermark, and
    // only that stream's.
    brain
        .index_transcript_through("c1", TranscriptStream::Thread, 50, Vec::new())
        .unwrap();
    assert_eq!(
        brain
            .transcript_watermark("c1", TranscriptStream::Thread)
            .unwrap(),
        Some(50)
    );
    assert_eq!(
        brain
            .transcript_watermark("c1", TranscriptStream::Conversation)
            .unwrap(),
        Some(12)
    );
    brain.forget_transcript("c1").unwrap();
    assert!(
        brain
            .search_transcript("c1", "quokka", 10)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        brain
            .transcript_watermark("c1", TranscriptStream::Thread)
            .unwrap(),
        None
    );
}

/// The user's edit of a memory changes it in place, keeps no earlier text, and deleting it
/// afterwards brings nothing back.
#[test]
fn an_edit_changes_the_text_in_place_with_no_history() {
    let brain = TestBrain::new();
    let id = brain
        .record(node(
            NodeKind::Preference,
            Some("preference:a"),
            "Lives in Iasi",
            "",
        ))
        .unwrap();
    brain
        .edit(&id, "Lives in Chisinau".into(), String::new())
        .unwrap();
    let nodes = all(&brain);
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].id, id);
    assert_eq!(nodes[0].title, "Lives in Chisinau");
    assert_eq!(nodes[0].key.as_deref(), Some("preference:a"));
    brain.delete(&id).unwrap();
    assert!(all(&brain).is_empty());
    assert!(matches!(
        brain.edit(&id, "x".into(), String::new()),
        Err(Error::NotFound(_))
    ));
}
