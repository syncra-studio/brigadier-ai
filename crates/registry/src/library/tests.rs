use super::*;
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("brigadier-library-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_git_skill_is_fetched_by_its_name() {
    let dir = scratch("git");
    let repo = dir.join("repo");
    for (folder, name) in [("skills/one", "one"), ("skills/two", "two")] {
        std::fs::create_dir_all(repo.join(folder)).unwrap();
        std::fs::write(
            repo.join(folder).join("SKILL.md"),
            format!("---\nname: {name}\n---\n"),
        )
        .unwrap();
    }
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-C"])
            .arg(&repo)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
    };
    git(&["init", "-q"]);
    git(&["add", "."]);
    git(&["commit", "-qm", "skills"]);
    let url = format!("file://{}", repo.display());
    let whole = SkillSource::Git {
        url: url.clone(),
        rev: None,
        subdir: None,
    };
    let scratch_one = dir.join("s1");
    assert!(
        fetch_skill(&whole, &scratch_one)
            .unwrap_err()
            .contains("several skills")
    );
    let one = SkillSource::Git {
        url,
        rev: None,
        subdir: Some("skills/two".to_owned()),
    };
    let fetched = fetch_skill(&one, &dir.join("s2")).unwrap();
    assert!(fetched.ends_with("skills/two"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_item_fetched_from_git_knows_when_its_source_moved_on() {
    let dir = scratch("origin");
    let repo = dir.join("repo");
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
    };
    std::fs::create_dir_all(repo.join("tidy")).unwrap();
    std::fs::write(repo.join("tidy/SKILL.md"), "---\nname: tidy\n---\nv1\n").unwrap();
    git(&["init", "-q", "-b", "main"]);
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "v1"]);
    let source = SkillSource::Git {
        url: repo.display().to_string(),
        rev: None,
        subdir: None,
    };
    let fetched = dir.join("fetch");
    std::fs::create_dir_all(&fetched).unwrap();
    fetch_skill(&source, &fetched).unwrap();
    let commit = fetched_commit(&fetched).unwrap();
    assert_eq!(latest_commit(&source).unwrap(), commit);

    std::fs::write(repo.join("tidy/SKILL.md"), "---\nname: tidy\n---\nv2\n").unwrap();
    git(&["commit", "-q", "-am", "v2"]);
    assert_ne!(latest_commit(&source).unwrap(), commit);
    assert!(latest_commit(&SkillSource::Folder(dir.clone())).is_err());
    let _ = std::fs::remove_dir_all(dir);
}
