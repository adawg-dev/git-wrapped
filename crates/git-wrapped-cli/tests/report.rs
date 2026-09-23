mod common;
use common::Fixture;
use git_wrapped::analysis::analyze;
use git_wrapped::awards::select_awards;
use git_wrapped::config::{normalize, Config};
use git_wrapped::git::{discover, scan};

fn authored_commit(f: &Fixture, email: &str, date: &str, args: &[&str]) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(f.dir.path())
        .args(args)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", email)
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn analysis_counts_history_and_serializes_deterministically() {
    let f = Fixture::new();
    f.commit(
        "a.txt",
        b"one\ntwo\n",
        "alice@example.com",
        "2024-01-01T23:30:00 -0800",
    );
    assert!(f.git(&["branch", "side"]).status.success());
    f.commit(
        "a.txt",
        b"one\nthree\n",
        "alice@example.com",
        "2024-01-03T11:00:00 +0000",
    );
    authored_commit(
        &f,
        "alice@example.com",
        "2024-01-04T12:00:00 +0000",
        &["commit", "--allow-empty", "-qm", "empty"],
    );
    assert!(f.git(&["checkout", "-q", "side"]).status.success());
    f.commit(
        "dir/b.txt",
        b"x\ny\n",
        "bob@example.com",
        "2024-01-02T10:00:00 +0000",
    );
    assert!(f.git(&["mv", "dir/b.txt", "dir/c.txt"]).status.success());
    authored_commit(
        &f,
        "bob@example.com",
        "2024-01-05T13:00:00 +0000",
        &["commit", "-qm", "rename"],
    );
    f.commit(
        "photo.png",
        b"\0\x01\x02",
        "bob@example.com",
        "2024-01-06T14:00:00 +0000",
    );
    assert!(f.git(&["checkout", "-q", "master"]).status.success());
    authored_commit(
        &f,
        "alice@example.com",
        "2024-01-07T15:00:00 +0000",
        &["merge", "--no-ff", "-qm", "merge", "side"],
    );

    let repo = discover(f.dir.path()).unwrap();
    let data = analyze(&repo, &Config::default()).unwrap();
    let again = analyze(&repo, &Config::default()).unwrap();
    assert_eq!(
        serde_json::to_vec(&data).unwrap(),
        serde_json::to_vec(&again).unwrap()
    );
    assert_eq!(data.repository.total_commits, 7);
    assert_eq!(data.repository.total_contributors, 2);
    assert_eq!(
        (
            data.repository.additions,
            data.repository.deletions,
            data.repository.net_historical_lines,
            data.repository.churn
        ),
        (5, 1, 4, 6)
    );
    assert_eq!(data.repository.tracked_files, 3);
    assert_eq!(data.repository.age_days, 6);
    assert_eq!(data.repository.first_commit, "2024-01-01T23:30:00-08:00");
    assert_eq!(data.repository.latest_commit, "2024-01-07T15:00:00+00:00");
    let alice = &data.contributors[0];
    assert_eq!(alice.first_contribution, "2024-01-01T23:30:00-08:00");
    assert_eq!(alice.latest_contribution, "2024-01-07T15:00:00+00:00");
    assert_eq!(
        (alice.id.as_str(), alice.commits, alice.commit_percent),
        ("alice@example.com", 4, 400.0 / 7.0)
    );
    assert_eq!(
        (alice.additions, alice.deletions, alice.net, alice.churn),
        (3, 1, 2, 4)
    );
    assert_eq!(
        (
            alice.files_touched,
            alice.directories_touched,
            alice.active_days
        ),
        (1, 1, 4)
    );
    assert_eq!(
        (
            alice.average_commit_size,
            alice.median_commit_size,
            alice.largest_commit,
            alice.largest_deletion
        ),
        (1.0, 1.0, 2, 1)
    );
    assert_eq!(
        (
            alice.commits_by_hour[23],
            alice.commits_by_weekday[0],
            alice.commits_by_month[0]
        ),
        (1, 1, 4)
    );
    let bob = &data.contributors[1];
    assert_eq!(
        (
            bob.commits,
            bob.additions,
            bob.deletions,
            bob.files_touched,
            bob.directories_touched,
            bob.active_days
        ),
        (3, 2, 0, 3, 2, 3)
    );
    assert_eq!(
        (
            bob.average_commit_size,
            bob.median_commit_size,
            bob.largest_commit
        ),
        (2.0 / 3.0, 0.0, 2)
    );
    assert_eq!(
        (
            bob.commits_by_hour[10],
            bob.commits_by_weekday[1],
            bob.commits_by_month[0]
        ),
        (1, 1, 3)
    );
    assert_eq!(
        (
            data.activity[0].month.as_str(),
            data.activity[0].commits,
            data.activity[0].additions,
            data.activity[0].deletions
        ),
        ("2024-01", 7, 5, 1)
    );
    assert_eq!(data.activity_heatmap.len(), 7);
    assert_eq!(data.activity_heatmap[0].date, "2024-01-01");
    assert_eq!(
        serde_json::to_value(&data.awards).unwrap(),
        serde_json::to_value(select_awards(&data)).unwrap()
    );
    assert_eq!(
        data.commits.iter().filter(|c| c.files_changed == 0).count(),
        2
    );
}

#[test]
fn award_ties_use_stable_identity() {
    let f = Fixture::new();
    f.commit(
        "a.txt",
        b"a\n",
        "z@example.com",
        "2024-01-01T12:00:00 +0000",
    );
    f.commit(
        "b.txt",
        b"b\n",
        "a@example.com",
        "2024-01-02T12:00:00 +0000",
    );
    let repo = discover(f.dir.path()).unwrap();
    let data = analyze(&repo, &Config::default()).unwrap();
    let awards = select_awards(&data);
    assert_eq!(
        awards
            .iter()
            .find(|a| a.slug == "commit-machine")
            .unwrap()
            .winner_id,
        "a@example.com"
    );
    assert!(!awards.iter().any(|a| a.slug == "night-owl"));
}

#[test]
fn awards_cover_all_ten_positive_metrics() {
    let f = Fixture::new();
    for (index, hour) in [1, 6, 1, 6, 1, 6].into_iter().enumerate() {
        f.commit(
            "file.txt",
            format!("line {index}\n").as_bytes(),
            "a@example.com",
            &format!("2024-01-0{}T{hour:02}:00:00 +0000", index + 6),
        );
    }
    let data = analyze(&discover(f.dir.path()).unwrap(), &Config::default()).unwrap();
    let slugs: Vec<_> = data.awards.iter().map(|a| a.slug.as_str()).collect();
    assert_eq!(
        slugs,
        [
            "commit-machine",
            "code-creator",
            "code-destroyer",
            "net-positive",
            "night-owl",
            "early-bird",
            "weekend-warrior",
            "biggest-bang",
            "biggest-cleanup",
            "repo-explorer"
        ]
    );
    for award in &data.awards {
        assert!(!award.winner_id.is_empty());
        assert!(!award.winner.is_empty());
        assert!(!award.metric.is_empty());
        assert!(!award.value.is_empty());
        assert!(!award.explanation.is_empty());
    }
}

#[test]
fn analysis_rejects_empty_history() {
    let f = Fixture::new();
    let repo = git_wrapped::git::Repository {
        root: f.dir.path().to_owned(),
        name: "empty".into(),
        shallow: false,
        tracked_files: 0,
    };
    assert!(analyze(&repo, &Config::default()).is_err());
}

#[test]
fn author_offset_controls_hour_and_day() {
    let f = Fixture::new();
    f.commit(
        "a.txt",
        b"one\n",
        "alice@example.com",
        "2024-01-01T23:30:00 -0800",
    );
    let repo = discover(f.dir.path()).unwrap();
    let data = analyze(&repo, &Config::default()).unwrap();
    assert_eq!(data.contributors[0].commits_by_hour[23], 1);
    assert_eq!(data.activity_heatmap[0].date, "2024-01-01");
}

#[test]
fn mailmap_then_aliases_preserve_raw_author() {
    let f = Fixture::new();
    f.commit(
        "nested/old",
        b"old\n",
        "old@example.com",
        "2024-01-01T10:00:00 +0000",
    );
    f.commit(
        "other",
        b"other\n",
        "other@example.com",
        "2024-01-02T10:00:00 +0000",
    );
    std::fs::write(
        f.dir.path().join(".mailmap"),
        "Correct Name <correct@example.com> <old@example.com>\n",
    )
    .unwrap();
    std::fs::write(
        f.dir.path().join(".git-wrapped.json"),
        r#"{"contributors":{"Team Member":["correct@example.com","OTHER@EXAMPLE.COM"]}}"#,
    )
    .unwrap();
    let repo = discover(&f.dir.path().join("nested")).unwrap();
    let config = Config::load(&repo.root).unwrap();
    let mut commits = Vec::new();
    scan(&repo, |commit| {
        commits.push(commit);
        Ok(())
    })
    .unwrap();
    let old = commits
        .iter()
        .find(|c| c.raw_author.email == "old@example.com")
        .unwrap();
    assert_eq!(old.mapped_author.email, "correct@example.com");
    assert_eq!(old.raw_author.email, "old@example.com");
    assert_eq!(
        normalize(&old.mapped_author, &config),
        ("correct@example.com".into(), "Team Member".into())
    );
    assert_eq!(
        normalize(&commits[0].mapped_author, &config),
        normalize(&commits[1].mapped_author, &config)
    );
}

#[test]
fn malformed_config_error_names_discovered_root_file() {
    let f = Fixture::new();
    f.commit("a", b"a\n", "a@example.com", "2024-01-01T10:00:00 +0000");
    std::fs::write(f.dir.path().join(".git-wrapped.json"), "{").unwrap();
    let repo = discover(f.dir.path()).unwrap();
    let error = Config::load(&repo.root).err().unwrap();
    assert!(error.contains(&repo.root.join(".git-wrapped.json").display().to_string()));
}

#[test]
fn discovers_nested_repository_and_rejects_empty_history() {
    let f = Fixture::new();
    assert!(discover(f.dir.path()).unwrap_err().contains("no commits"));
    f.commit(
        "nested/file.txt",
        b"hello\n",
        "test@example.com",
        "2024-01-01T10:00:00 +0000",
    );
    let repo = discover(&f.dir.path().join("nested")).unwrap();
    assert_eq!(repo.root, f.dir.path().canonicalize().unwrap());
    assert_eq!(repo.tracked_files, 1);
}

#[test]
fn scans_root_empty_rename_binary_and_unusual_paths() {
    let f = Fixture::new();
    f.commit(
        "old\nname.txt",
        b"hello\n",
        "test@example.com",
        "2024-01-01T10:00:00 +0000",
    );
    assert!(f
        .git(&["mv", "old\nname.txt", "new\nname.txt"])
        .status
        .success());
    assert!(f.git(&["commit", "-qm", "rename"]).status.success());
    f.commit(
        "photo.png",
        b"\0\x01\x02",
        "test@example.com",
        "2024-01-02T10:00:00 +0000",
    );
    assert!(f
        .git(&["commit", "--allow-empty", "-qm", "empty"])
        .status
        .success());
    let repo = discover(f.dir.path()).unwrap();
    let mut commits = Vec::new();
    scan(&repo, |c| {
        commits.push(c);
        Ok(())
    })
    .unwrap();
    assert_eq!(commits.len(), 4);
    assert!(commits[0].changes.is_empty());
    assert!(commits[1].changes[0].binary);
    assert_eq!(
        commits[2].changes[0].old_path.as_deref(),
        Some(b"old\nname.txt".as_slice())
    );
    assert_eq!(commits[2].changes[0].path, b"new\nname.txt");
    assert_eq!(commits[3].changes[0].additions, 1);
    assert!(commits[3].parents.is_empty());
}

#[test]
fn linked_worktree_is_discovered() {
    let f = Fixture::new();
    f.commit("a", b"a\n", "a@x", "2024-01-01T10:00:00 +0000");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("linked");
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(f.dir.path())
        .args(["worktree", "add", "--detach"])
        .arg(&path)
        .arg("HEAD")
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    assert_eq!(discover(&path).unwrap().root, path.canonicalize().unwrap());
}

#[test]
fn merge_has_no_numstat_and_scan_continues() {
    let f = Fixture::new();
    f.commit("base", b"base\n", "a@x", "2024-01-01T10:00:00 +0000");
    assert!(f.git(&["branch", "side"]).status.success());
    f.commit("main", b"main\n", "a@x", "2024-01-02T10:00:00 +0000");
    assert!(f.git(&["checkout", "-q", "side"]).status.success());
    f.commit("side", b"side\n", "a@x", "2024-01-03T10:00:00 +0000");
    assert!(f.git(&["checkout", "-q", "master"]).status.success());
    assert!(f
        .git(&["merge", "--no-ff", "-qm", "merge", "side"])
        .status
        .success());
    let mut commits = Vec::new();
    scan(&discover(f.dir.path()).unwrap(), |c| {
        commits.push(c);
        Ok(())
    })
    .unwrap();
    assert_eq!(commits.len(), 4);
    assert_eq!(commits[0].parents.len(), 2);
    assert!(commits[0].changes.is_empty());
    assert_eq!(commits[3].changes[0].additions, 1);
}

#[test]
fn shallow_clone_reports_available_history() {
    let f = Fixture::new();
    f.commit("one", b"one\n", "a@x", "2024-01-01T10:00:00 +0000");
    f.commit("two", b"two\n", "a@x", "2024-01-02T10:00:00 +0000");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("clone");
    let status = std::process::Command::new("git")
        .args(["clone", "-q", "--depth=1"])
        .arg(format!("file://{}", f.dir.path().display()))
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let repo = discover(&path).unwrap();
    assert!(repo.shallow);
    let mut commits = Vec::new();
    scan(&repo, |c| {
        commits.push(c);
        Ok(())
    })
    .unwrap();
    assert_eq!(commits.len(), 1);
}

#[test]
fn scan_preserves_control_byte_path() {
    let f = Fixture::new();
    f.commit(
        "prefix\u{1e}suffix",
        b"text\n",
        "a@x",
        "2024-01-01T10:00:00 +0000",
    );
    let mut paths = Vec::new();
    scan(&discover(f.dir.path()).unwrap(), |c| {
        paths.extend(c.changes.into_iter().map(|change| change.path));
        Ok(())
    })
    .unwrap();
    assert_eq!(paths, vec![b"prefix\x1esuffix".to_vec()]);
}

#[test]
fn render_report_is_safe_complete_and_deterministic() {
    use git_wrapped::render::{render_report, Theme};
    let f = Fixture::new();
    f.commit(
        "a\t\u{1e}.txt",
        b"a\n",
        "a@example.com",
        "2024-01-01T12:00:00 +0000",
    );
    let mut data = analyze(&discover(f.dir.path()).unwrap(), &Config::default()).unwrap();
    data.repository.name = "<script>&\"".into();
    data.contributors[0].name = "Line\nBreak\u{85}".into();
    data.commits[0].subject = "subject\t\u{1e}".into();
    let out = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    render_report(&data, out.path(), Theme::Dark).unwrap();
    render_report(&data, other.path(), Theme::Dark).unwrap();
    for file in [
        "summary.svg",
        "contributors.svg",
        "activity.svg",
        "activity-heatmap.svg",
        "awards/commit-machine.svg",
        "awards/code-creator.svg",
        "awards/code-destroyer.svg",
        "awards/night-owl.svg",
        "data.json",
    ] {
        let value = std::fs::read_to_string(out.path().join(file)).unwrap();
        assert_eq!(
            value,
            std::fs::read_to_string(other.path().join(file)).unwrap()
        );
        if file.ends_with("svg") {
            assert!(value.starts_with("<svg"));
            assert!(!value.contains("<script>"));
            assert!(!value.chars().any(|c| c.is_control() && !c.is_whitespace()));
        }
    }
    let summary = std::fs::read_to_string(out.path().join("summary.svg")).unwrap();
    assert!(summary.contains("&lt;script&gt;&amp;&quot;"));
    assert!(summary.contains("#10131f"));
    assert!(
        std::fs::read_to_string(out.path().join("awards/night-owl.svg"))
            .unwrap()
            .contains("No eligible winner")
    );
    render_report(&data, other.path(), Theme::Light).unwrap();
    assert!(std::fs::read_to_string(other.path().join("summary.svg"))
        .unwrap()
        .contains("#f7f7fc"));
    // Sparse analysis months must retain their calendar spacing.
    data.activity.push(git_wrapped::model::Activity {
        month: "2024-03".into(),
        commits: 2,
        additions: 0,
        deletions: 0,
    });
    render_report(&data, other.path(), Theme::Dark).unwrap();
    let activity = std::fs::read_to_string(other.path().join("activity.svg")).unwrap();
    assert!(activity.contains("2024-02: 0 commits"));
    #[cfg(unix)]
    {
        let protected = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(protected.path(), "untouched").unwrap();
        let linked = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(protected.path(), linked.path().join("summary.svg")).unwrap();
        assert!(render_report(&data, linked.path(), Theme::Dark).is_err());
        assert_eq!(
            std::fs::read_to_string(protected.path()).unwrap(),
            "untouched"
        );
    }
    data.contributors.clear();
    data.awards.clear();
    data.activity.clear();
    data.activity_heatmap.clear();
    render_report(&data, other.path(), Theme::Dark).unwrap();
    assert!(std::fs::read_to_string(other.path().join("activity.svg"))
        .unwrap()
        .contains("No activity"));
}
