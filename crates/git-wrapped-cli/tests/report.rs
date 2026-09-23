mod common;
use common::{tempdir, Fixture};
use git_wrapped::analysis::{
    activity_by_day, activity_by_month, analyze, analyze_with_options, sample_trees,
    AnalysisOptions, TimezoneChoice,
};
use git_wrapped::awards::select_awards;
use git_wrapped::config::{normalize, Config};
use git_wrapped::git::{discover, reachable_tag_dates, scan, scan_with_cancel};
use git_wrapped::progress::CancelFlag;
use std::{fs, process::Command};

#[test]
fn current_ownership_counts_head_lines_and_normalizes_mailmap_aliases() {
    let f = Fixture::new();
    f.commit("a.txt", b"one\ntwo\n", "old@x", "2024-01-01T10:00:00 +0000");
    f.commit("a.txt", b"one\nthree\n", "b@x", "2024-01-02T10:00:00 +0000");
    fs::write(
        f.dir.path().join(".mailmap"),
        "Canonical <canon@x> <old@x>\n",
    )
    .unwrap();
    f.commit("empty.txt", b"", "b@x", "2024-01-03T10:00:00 +0000");
    f.commit("binary.bin", b"x\0y", "b@x", "2024-01-04T10:00:00 +0000");
    let repo = discover(f.dir.path()).unwrap();
    fs::write(
        f.dir.path().join(".git-wrapped.json"),
        r#"{"exclude":[".mailmap"],"contributors":{"Canonical":["canon@x","alias@x"]}}"#,
    )
    .unwrap();
    let config = Config::load(&repo.root).unwrap();
    let mut data = analyze(&repo, &config).unwrap();
    git_wrapped::deep::analyze_deep(&repo, &config, &mut data, Default::default()).unwrap();
    let deep = data.deep.unwrap();
    assert_eq!(deep.surviving_loc, 2);
    assert_eq!(deep.coverage.attributed_lines, 2);
    assert_eq!(deep.coverage.skipped_binary, 1);
    assert_eq!(deep.coverage.unknown_lines, 0);
    assert_eq!(deep.ownership.iter().map(|x| x.lines).sum::<u64>(), 2);
    assert!(deep
        .ownership
        .iter()
        .any(|x| x.author_id == "alias@x" && x.lines == 1));
    assert!(deep
        .ownership
        .iter()
        .any(|x| x.author_id == "b@x" && x.lines == 1));
}

#[test]
fn deep_limits_report_partial_coverage_and_tree_selection() {
    let f = Fixture::new();
    f.commit("keep/a", b"a\n", "a@x", "2024-01-01T23:30:00 -0800");
    f.commit("skip/b", b"b\n", "a@x", "2024-01-02T10:00:00 +0000");
    f.commit("keep/c", b"c\n", "a@x", "2024-01-03T10:00:00 +0000");
    let repo = discover(f.dir.path()).unwrap();
    let options = AnalysisOptions {
        timezone: TimezoneChoice::Utc,
        exclusions: vec!["skip/**".into()],
        ..Default::default()
    };
    let mut data = analyze_with_options(&repo, &Config::default(), &options).unwrap();
    assert!(data.tree_samples.is_empty());
    git_wrapped::deep::analyze_deep_with_cancel(
        &repo,
        &Config::default(),
        &mut data,
        git_wrapped::deep::DeepLimits {
            max_files: 1,
            ..Default::default()
        },
        &CancelFlag::default(),
    )
    .unwrap();
    assert_eq!(data.tree_samples.len(), 3);
    assert_eq!(data.tree_samples[0].author_date, "2024-01-02");
    assert_eq!(data.tree_samples[0].tracked_files, 1);
    assert_eq!(data.tree_samples[1].tracked_files, 1);
    assert_eq!(data.tree_samples[2].tracked_files, 2);
    let deep = data.deep.unwrap();
    assert_eq!(deep.coverage.eligible_files, 2);
    assert_eq!(deep.coverage.analyzed_files, 1);
    assert!(deep.coverage.truncated);
}

#[test]
fn deep_head_ownership_ignores_history_date_and_author_selectors() {
    let f = Fixture::new();
    f.commit("first", b"one\n", "a@x", "2024-01-01T10:00:00 +0000");
    f.commit("second", b"two\n", "b@x", "2024-01-02T10:00:00 +0000");
    let repo = discover(f.dir.path()).unwrap();
    let options = AnalysisOptions {
        since: Some("2024-01-02".parse().unwrap()),
        author_ids: vec!["b@x".into()],
        ..Default::default()
    };
    let mut data = analyze_with_options(&repo, &Config::default(), &options).unwrap();
    assert_eq!(data.commits.len(), 1);
    git_wrapped::deep::analyze_deep(&repo, &Config::default(), &mut data, Default::default())
        .unwrap();
    assert_eq!(data.tree_samples.len(), 1);
    assert_eq!(data.tree_samples[0].tracked_files, 2);
    let ownership = &data.deep.unwrap().ownership;
    assert!(ownership
        .iter()
        .any(|x| x.author_id == "a@x" && x.lines == 1));
    assert!(ownership
        .iter()
        .any(|x| x.author_id == "b@x" && x.lines == 1));
}

#[test]
fn cancelled_deep_pass_publishes_no_partial_data() {
    let f = Fixture::new();
    f.commit("first", b"one\n", "a@x", "2024-01-01T10:00:00 +0000");
    let repo = discover(f.dir.path()).unwrap();
    let mut data = analyze(&repo, &Config::default()).unwrap();
    let cancel = CancelFlag::default();
    cancel.cancel();
    assert_eq!(
        git_wrapped::deep::analyze_deep_with_cancel(
            &repo,
            &Config::default(),
            &mut data,
            Default::default(),
            &cancel
        )
        .unwrap_err(),
        "cancelled"
    );
    assert!(data.deep.is_none());
    assert!(data.tree_samples.is_empty());
}

#[test]
fn deep_coverage_skips_gitlinks_and_stops_at_line_budget() {
    let f = Fixture::new();
    f.commit("text", b"one\ntwo\n", "a@x", "2024-01-01T10:00:00 +0000");
    let sha = String::from_utf8(f.git(&["rev-parse", "HEAD"]).stdout).unwrap();
    assert!(f
        .git(&[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{},module", sha.trim())
        ])
        .status
        .success());
    assert!(f.git(&["commit", "-qm", "gitlink"]).status.success());
    let repo = discover(f.dir.path()).unwrap();
    let mut data = analyze(&repo, &Config::default()).unwrap();
    git_wrapped::deep::analyze_deep(
        &repo,
        &Config::default(),
        &mut data,
        git_wrapped::deep::DeepLimits {
            max_lines: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let coverage = &data.deep.unwrap().coverage;
    assert_eq!(coverage.skipped_submodules, 1);
    assert_eq!(coverage.attributed_lines, 1);
    assert!(coverage.truncated);
}

#[test]
fn deep_blame_stream_stops_at_line_budget_on_many_line_file() {
    let f = Fixture::new();
    let contents = (0..5000).map(|n| format!("line {n}\n")).collect::<String>();
    f.commit(
        "many.txt",
        contents.as_bytes(),
        "a@x",
        "2024-01-01T10:00:00 +0000",
    );
    let repo = discover(f.dir.path()).unwrap();
    let mut data = analyze(&repo, &Config::default()).unwrap();
    git_wrapped::deep::analyze_deep(
        &repo,
        &Config::default(),
        &mut data,
        git_wrapped::deep::DeepLimits {
            max_lines: 17,
            ..Default::default()
        },
    )
    .unwrap();
    let deep = data.deep.unwrap();
    assert_eq!(deep.surviving_loc, 17);
    assert_eq!(deep.coverage.attributed_lines, 17);
    assert!(deep.coverage.truncated);
}

#[test]
fn deep_blame_accepts_sha256_repository_if_git_supports_it() {
    let dir = tempdir();
    let init = Command::new("git")
        .args(["init", "-q", "--object-format=sha256"])
        .arg(dir.path())
        .output()
        .unwrap();
    if !init.status.success() {
        return;
    }
    let git = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(args)
            .output()
            .unwrap()
    };
    assert!(git(&["config", "user.name", "Test"]).status.success());
    assert!(git(&["config", "user.email", "a@x"]).status.success());
    fs::write(dir.path().join("a.txt"), b"one\n").unwrap();
    assert!(git(&["add", "a.txt"]).status.success());
    assert!(git(&["commit", "-qm", "first"]).status.success());
    let repo = discover(dir.path()).unwrap();
    let mut data = analyze(&repo, &Config::default()).unwrap();
    assert_eq!(data.commits[0].sha.len(), 64);
    git_wrapped::deep::analyze_deep(&repo, &Config::default(), &mut data, Default::default())
        .unwrap();
    assert_eq!(data.deep.unwrap().coverage.attributed_lines, 1);
}

#[test]
fn deep_cli_uses_distinct_cache_and_ownership_view() {
    let f = Fixture::new();
    f.commit("a", b"one\n", "a@x", "2024-01-01T10:00:00 +0000");
    let out = tempdir();
    let output = out.path().join("nested");
    let args: &[&std::ffi::OsStr] = &[
        "--output".as_ref(),
        output.as_os_str(),
        "ownership".as_ref(),
        "--deep".as_ref(),
    ];
    let first = cli(args, f.dir.path());
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(String::from_utf8_lossy(&first.stdout).contains("a@x\t1"));
    assert!(output.join(".git-wrapped-cache.json").exists());
    let second = cli(args, f.dir.path());
    assert!(String::from_utf8_lossy(&second.stderr).contains("Using cached analysis"));
    let shallow = cli(
        &[
            "--output".as_ref(),
            output.as_os_str(),
            "--no-png".as_ref(),
            "report".as_ref(),
        ],
        f.dir.path(),
    );
    assert!(
        shallow.status.success(),
        "{}",
        String::from_utf8_lossy(&shallow.stderr)
    );
    assert!(!String::from_utf8_lossy(&shallow.stderr).contains("Using cached analysis"));
    let shallow_json: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("data.json")).unwrap()).unwrap();
    assert!(shallow_json["deep"].is_null());
    assert!(shallow_json["tree_samples"].as_array().unwrap().is_empty());
    let deep = cli(
        &[
            "--output".as_ref(),
            output.as_os_str(),
            "--no-png".as_ref(),
            "report".as_ref(),
            "--deep".as_ref(),
        ],
        f.dir.path(),
    );
    assert!(
        deep.status.success(),
        "{}",
        String::from_utf8_lossy(&deep.stderr)
    );
    assert!(!String::from_utf8_lossy(&deep.stderr).contains("Using cached analysis"));
    let json: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("data.json")).unwrap()).unwrap();
    assert_eq!(json["deep"]["coverage"]["attributed_lines"], 1);
}

#[test]
fn exclusions_keep_commits_and_filter_history_and_head_paths() {
    let f = Fixture::new();
    f.commit("Cargo.lock", b"lock\n", "a@x", "2024-01-01T10:00:00 +0000");
    f.commit(
        "vendor/dependency.rs",
        b"vendor\n",
        "a@x",
        "2024-01-02T10:00:00 +0000",
    );
    f.commit(
        "src/main.rs",
        b"source\n",
        "a@x",
        "2024-01-03T10:00:00 +0000",
    );
    fs::write(
        f.dir.path().join(".git-wrapped.json"),
        r#"{"exclude":["**/*.lock"],"contributors":{}}"#,
    )
    .unwrap();
    let repo = discover(f.dir.path()).unwrap();
    let config = Config::load(&repo.root).unwrap();
    let from_config = analyze_with_options(&repo, &config, &Default::default()).unwrap();
    assert_eq!(from_config.repository.total_commits, 3);
    assert_eq!(from_config.repository.additions, 2);
    assert_eq!(from_config.repository.excluded_changes, 1);
    let exported = cli(
        &[
            "--exclude".as_ref(),
            "vendor/**".as_ref(),
            "export".as_ref(),
        ],
        f.dir.path(),
    );
    assert!(
        exported.status.success(),
        "{}",
        String::from_utf8_lossy(&exported.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&exported.stdout).unwrap();
    assert_eq!(json["repository"]["total_commits"], 3);
    assert_eq!(json["repository"]["additions"], 1);
    assert_eq!(json["repository"]["tracked_files"], 1);
    assert_eq!(json["repository"]["excluded_changes"], 2);
    assert_eq!(
        json["repository"]["excluded_patterns"],
        serde_json::json!(["**/*.lock", "vendor/**"])
    );
    assert_eq!(json["files"].as_array().unwrap().len(), 1);
    assert_eq!(json["files"][0]["display_path"], "src/main.rs");
}

#[cfg(unix)]
#[test]
fn non_utf8_paths_keep_distinct_ids_while_matching_lossy_display() {
    use std::io::Write;
    use std::process::Stdio;

    let f = Fixture::new();
    let git_input = |args: &[&str], input: &[u8]| {
        let mut child = Command::new("git")
            .arg("-C")
            .arg(f.dir.path())
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    let blob = git_input(&["hash-object", "-w", "--stdin"], b"line\n");
    let mut tree_input = Vec::new();
    for suffix in [0xfe, 0xff] {
        tree_input.extend_from_slice(format!("100644 blob {blob}\t").as_bytes());
        tree_input.extend_from_slice(&[b'a', suffix, b'.', b'r', b's', 0]);
    }
    let tree = git_input(&["mktree", "-z"], &tree_input);
    let commit = git_input(&["commit-tree", &tree], b"non utf8\n");
    assert!(f
        .git(&["update-ref", "refs/heads/master", &commit])
        .status
        .success());
    let repo = discover(f.dir.path()).unwrap();
    let plain = analyze(&repo, &Config::default()).unwrap();
    assert_eq!(plain.files.len(), 2);
    assert_eq!(plain.files[0].display_path, "a�.rs");
    assert_eq!(plain.files[1].display_path, "a�.rs");
    assert_ne!(plain.files[0].path_id, plain.files[1].path_id);
    let filtered = analyze_with_options(
        &repo,
        &Config::default(),
        &AnalysisOptions {
            exclusions: vec!["a�.rs".into()],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(filtered.repository.total_commits, 1);
    assert_eq!(filtered.repository.excluded_changes, 2);
    assert!(filtered.files.is_empty());
}

#[test]
fn invalid_config_exclusion_names_file_and_pattern() {
    let f = Fixture::new();
    f.commit("a", b"one\n", "a@x", "2024-01-01T10:00:00 +0000");
    fs::write(
        f.dir.path().join(".git-wrapped.json"),
        r#"{"exclude":["["]}"#,
    )
    .unwrap();
    let repo = discover(f.dir.path()).unwrap();
    let config = Config::load(&repo.root).unwrap();
    let error = analyze_with_options(&repo, &config, &Default::default()).unwrap_err();
    assert!(
        error.contains(".git-wrapped.json") && error.contains("["),
        "{error}"
    );

    fs::remove_file(f.dir.path().join(".git-wrapped.json")).unwrap();
    let cli_error = cli(
        &["--exclude".as_ref(), "[".as_ref(), "export".as_ref()],
        f.dir.path(),
    );
    assert!(!cli_error.status.success());
    let stderr = String::from_utf8_lossy(&cli_error.stderr);
    assert!(
        stderr.contains("--exclude") && stderr.contains("["),
        "{stderr}"
    );
}

#[test]
fn config_timezone_is_used_unless_cli_overrides_it() {
    let f = Fixture::new();
    f.commit("a", b"one\n", "a@x", "2024-01-01T23:30:00 -0800");
    fs::write(
        f.dir.path().join(".git-wrapped.json"),
        r#"{"timezone":"utc"}"#,
    )
    .unwrap();
    let from_config = cli(&["export".as_ref()], f.dir.path());
    assert!(from_config.status.success());
    let json: serde_json::Value = serde_json::from_slice(&from_config.stdout).unwrap();
    assert_eq!(json["repository"]["timezone"], "utc");
    let override_zone = cli(
        &["--timezone".as_ref(), "commit".as_ref(), "export".as_ref()],
        f.dir.path(),
    );
    assert!(override_zone.status.success());
    let json: serde_json::Value = serde_json::from_slice(&override_zone.stdout).unwrap();
    assert_eq!(json["repository"]["timezone"], "commit");
}

fn cli(args: &[&std::ffi::OsStr], cwd: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_git-wrapped"))
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn report_cache_hits_only_for_the_same_analysis_key() {
    let f = Fixture::new();
    f.commit("a", b"one\n", "a@x", "2024-01-01T10:00:00 +0000");
    let out = tempdir();
    let args: &[&std::ffi::OsStr] = &[
        "--no-png".as_ref(),
        "--output".as_ref(),
        out.path().as_os_str(),
        "report".as_ref(),
    ];
    let run = || cli(args, f.dir.path());
    let first = run();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(!String::from_utf8_lossy(&first.stderr).contains("Using cached analysis"));
    assert!(out.path().join(".git-wrapped-cache.json").is_file());
    let second = run();
    assert!(second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("Using cached analysis"));
    assert!(!String::from_utf8_lossy(&second.stdout).contains("Using cached analysis"));

    let no_cache = cli(
        &[
            "--no-cache".as_ref(),
            "--no-png".as_ref(),
            "--output".as_ref(),
            out.path().as_os_str(),
            "report".as_ref(),
        ],
        f.dir.path(),
    );
    assert!(no_cache.status.success());
    assert!(!String::from_utf8_lossy(&no_cache.stderr).contains("Using cached analysis"));

    let zone = cli(
        &[
            "--timezone".as_ref(),
            "utc".as_ref(),
            "--no-png".as_ref(),
            "--output".as_ref(),
            out.path().as_os_str(),
            "report".as_ref(),
        ],
        f.dir.path(),
    );
    assert!(zone.status.success());
    assert!(!String::from_utf8_lossy(&zone.stderr).contains("Using cached analysis"));
    let author = cli(
        &[
            "--author".as_ref(),
            "a@x".as_ref(),
            "--no-png".as_ref(),
            "--output".as_ref(),
            out.path().as_os_str(),
            "report".as_ref(),
        ],
        f.dir.path(),
    );
    assert!(author.status.success());
    assert!(!String::from_utf8_lossy(&author.stderr).contains("Using cached analysis"));
    fs::write(
        f.dir.path().join(".git-wrapped.json"),
        r#"{"timezone":"utc"}"#,
    )
    .unwrap();
    let changed_config = run();
    assert!(changed_config.status.success());
    assert!(!String::from_utf8_lossy(&changed_config.stderr).contains("Using cached analysis"));
    f.commit("b", b"two\n", "a@x", "2024-01-02T10:00:00 +0000");
    let changed_head = run();
    assert!(changed_head.status.success());
    assert!(!String::from_utf8_lossy(&changed_head.stderr).contains("Using cached analysis"));
}

#[test]
fn corrupt_report_cache_is_rebuilt() {
    let f = Fixture::new();
    f.commit("a", b"one\n", "a@x", "2024-01-01T10:00:00 +0000");
    let out = tempdir();
    let args: &[&std::ffi::OsStr] = &[
        "--no-png".as_ref(),
        "--output".as_ref(),
        out.path().as_os_str(),
        "report".as_ref(),
    ];
    assert!(cli(args, f.dir.path()).status.success());
    fs::write(out.path().join(".git-wrapped-cache.json"), b"broken").unwrap();
    let repaired = cli(args, f.dir.path());
    assert!(repaired.status.success());
    assert!(!String::from_utf8_lossy(&repaired.stderr).contains("Using cached analysis"));
    let hit = cli(args, f.dir.path());
    assert!(String::from_utf8_lossy(&hit.stderr).contains("Using cached analysis"));
}

#[test]
fn mailmap_and_tag_changes_invalidate_report_cache_without_new_head() {
    let f = Fixture::new();
    f.commit("a", b"one\n", "old@x", "2024-01-01T10:00:00 +0000");
    let out = tempdir();
    let args: &[&std::ffi::OsStr] = &[
        "--no-png".as_ref(),
        "--output".as_ref(),
        out.path().as_os_str(),
        "report".as_ref(),
    ];
    assert!(cli(args, f.dir.path()).status.success());
    assert!(
        String::from_utf8_lossy(&cli(args, f.dir.path()).stderr).contains("Using cached analysis")
    );

    fs::write(f.dir.path().join(".mailmap"), "Mapped <new@x> <old@x>\n").unwrap();
    let changed_mailmap = cli(args, f.dir.path());
    assert!(changed_mailmap.status.success());
    assert!(!String::from_utf8_lossy(&changed_mailmap.stderr).contains("Using cached analysis"));
    assert!(
        String::from_utf8_lossy(&cli(args, f.dir.path()).stderr).contains("Using cached analysis")
    );

    assert!(f.git(&["tag", "-a", "-m", "first", "v1"]).status.success());
    let added_tag = cli(args, f.dir.path());
    assert!(added_tag.status.success());
    assert!(!String::from_utf8_lossy(&added_tag.stderr).contains("Using cached analysis"));
    assert!(f
        .git(&["tag", "-f", "-a", "-m", "changed", "v1"])
        .status
        .success());
    let changed_tag = cli(args, f.dir.path());
    assert!(changed_tag.status.success());
    assert!(!String::from_utf8_lossy(&changed_tag.stderr).contains("Using cached analysis"));

    let external = tempdir();
    let map = external.path().join("extra.mailmap");
    fs::write(&map, "Other <other@x> <old@x>\n").unwrap();
    assert!(f
        .git(&["config", "mailmap.file", map.to_str().unwrap()])
        .status
        .success());
    assert!(
        !String::from_utf8_lossy(&cli(args, f.dir.path()).stderr).contains("Using cached analysis")
    );
    fs::write(&map, "Another <another@x> <old@x>\n").unwrap();
    assert!(
        !String::from_utf8_lossy(&cli(args, f.dir.path()).stderr).contains("Using cached analysis")
    );
}

#[cfg(unix)]
#[test]
fn symlink_cache_target_is_not_replaced_or_followed() {
    let f = Fixture::new();
    f.commit("a", b"one\n", "a@x", "2024-01-01T10:00:00 +0000");
    let out = tempdir();
    let external = tempdir();
    let protected = external.path().join("keep.json");
    fs::write(&protected, b"unchanged").unwrap();
    std::os::unix::fs::symlink(&protected, out.path().join(".git-wrapped-cache.json")).unwrap();
    let args: &[&std::ffi::OsStr] = &[
        "--no-png".as_ref(),
        "--output".as_ref(),
        out.path().as_os_str(),
        "report".as_ref(),
    ];
    let result = cli(args, f.dir.path());
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stderr).contains("symlink cache"));
    assert_eq!(fs::read(&protected).unwrap(), b"unchanged");
    assert!(
        fs::symlink_metadata(out.path().join(".git-wrapped-cache.json"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn export_does_not_create_or_use_report_cache() {
    let f = Fixture::new();
    f.commit("a", b"one\n", "a@x", "2024-01-01T10:00:00 +0000");
    let out = tempdir();
    let path = out.path().join("new-report");
    let result = cli(
        &["--output".as_ref(), path.as_os_str(), "export".as_ref()],
        f.dir.path(),
    );
    assert!(result.status.success());
    assert!(result.stdout.starts_with(b"{"));
    assert!(!path.exists());
    assert!(!String::from_utf8_lossy(&result.stderr).contains("Using cached analysis"));
}

#[test]
fn utc_date_filter_differs_from_commit_offset_and_is_inclusive() {
    let f = Fixture::new();
    f.commit("early", b"early\n", "a@x", "2024-01-01T10:00:00 +0000");
    f.commit("late", b"late\n", "a@x", "2024-01-01T23:30:00 -0800");
    let repo = discover(f.dir.path()).unwrap();
    let date = chrono::NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
    let options = AnalysisOptions {
        since: Some(date),
        until: Some(date),
        timezone: TimezoneChoice::Utc,
        ..Default::default()
    };
    let selected = analyze_with_options(&repo, &Config::default(), &options).unwrap();
    assert_eq!(selected.repository.total_commits, 1);
    assert_eq!(selected.activity_heatmap[0].date, "2024-01-02");
    assert_eq!(
        selected.repository.selected_since.as_deref(),
        Some("2024-01-02")
    );
    assert_eq!(selected.repository.timezone, "utc");
    assert_eq!(selected.repository.tracked_files_scope, "HEAD");
    let error = analyze_with_options(
        &repo,
        &Config::default(),
        &AnalysisOptions {
            timezone: TimezoneChoice::Commit,
            ..options
        },
    )
    .unwrap_err();
    assert!(error.contains("no selected commits"), "{error}");
}

#[test]
fn named_timezone_uses_instant_and_rejects_invalid_range() {
    let f = Fixture::new();
    f.commit("winter", b"one\n", "a@x", "2024-01-02T07:30:00 +0000");
    f.commit("summer", b"two\n", "a@x", "2024-07-02T06:30:00 +0000");
    let repo = discover(f.dir.path()).unwrap();
    let options = AnalysisOptions {
        timezone: TimezoneChoice::Named("America/Los_Angeles".parse().unwrap()),
        ..Default::default()
    };
    let selected = analyze_with_options(&repo, &Config::default(), &options).unwrap();
    assert_eq!(
        selected
            .activity_heatmap
            .iter()
            .map(|cell| cell.date.as_str())
            .collect::<Vec<_>>(),
        ["2024-01-01", "2024-07-01"]
    );
    let error = analyze_with_options(
        &repo,
        &Config::default(),
        &AnalysisOptions {
            since: Some(chrono::NaiveDate::from_ymd_opt(2024, 7, 2).unwrap()),
            until: Some(chrono::NaiveDate::from_ymd_opt(2024, 1, 1).unwrap()),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(
        error.contains("until") && error.contains("since"),
        "{error}"
    );
}

#[test]
fn selected_author_id_uses_mailmap_and_aliases() {
    let f = Fixture::new();
    f.commit("a", b"one\n", "old@x", "2024-01-01T10:00:00 +0000");
    f.commit("b", b"two\n", "other@x", "2024-01-02T10:00:00 +0000");
    fs::write(f.dir.path().join(".mailmap"), "Team <new@x> <old@x>\n").unwrap();
    let mut config = Config::default();
    config
        .insert_alias_group("Team", &["new@x", "team@x"])
        .unwrap();
    let data = analyze_with_options(
        &discover(f.dir.path()).unwrap(),
        &config,
        &AnalysisOptions {
            author_ids: vec!["TEAM@X".into()],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(data.repository.total_commits, 1);
    assert_eq!(data.contributors[0].id, "new@x");
    assert_eq!(data.repository.selected_authors, ["new@x"]);
}

#[test]
fn no_merges_excludes_merge_commit_but_default_counts_it() {
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
    let repo = discover(f.dir.path()).unwrap();
    assert_eq!(
        analyze(&repo, &Config::default())
            .unwrap()
            .repository
            .total_commits,
        4
    );
    let selected = analyze_with_options(
        &repo,
        &Config::default(),
        &AnalysisOptions {
            include_merges: false,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(selected.repository.total_commits, 3);
    assert!(!selected.repository.include_merges);
    let exported = cli(&["--no-merges".as_ref(), "export".as_ref()], f.dir.path());
    assert!(
        exported.status.success(),
        "{}",
        String::from_utf8_lossy(&exported.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&exported.stdout).unwrap();
    assert_eq!(json["repository"]["total_commits"], 3);
}

#[test]
fn filtered_release_cadence_uses_selected_tag_targets() {
    let f = Fixture::new();
    f.commit("a", b"one\n", "a@x", "2024-01-01T10:00:00 +0000");
    assert!(f.git(&["tag", "v1"]).status.success());
    f.commit("b", b"two\n", "a@x", "2024-01-02T10:00:00 +0000");
    assert!(f.git(&["tag", "v2"]).status.success());
    let data = analyze_with_options(
        &discover(f.dir.path()).unwrap(),
        &Config::default(),
        &AnalysisOptions {
            since: Some(chrono::NaiveDate::from_ymd_opt(2024, 1, 2).unwrap()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(data.repository.total_commits, 1);
    assert_eq!(data.insights.first_tag_days, Some(0));
    assert_eq!(data.insights.release_interval_median_days, None);
}

#[test]
fn cli_global_selection_applies_to_export_and_views() {
    let f = Fixture::new();
    f.commit("a", b"one\n", "a@x", "2024-01-01T23:30:00 -0800");
    f.commit("b", b"two\n", "b@x", "2024-01-02T10:00:00 +0000");
    let export = cli(
        &[
            "--since".as_ref(),
            "2024-01-02".as_ref(),
            "--timezone".as_ref(),
            "utc".as_ref(),
            "--author".as_ref(),
            "A@X".as_ref(),
            "export".as_ref(),
        ],
        f.dir.path(),
    );
    assert!(
        export.status.success(),
        "{}",
        String::from_utf8_lossy(&export.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&export.stdout).unwrap();
    assert_eq!(json["repository"]["total_commits"], 1);
    assert_eq!(json["repository"]["timezone"], "utc");
    let view = cli(
        &[
            "contributors".as_ref(),
            "--since".as_ref(),
            "2024-01-02".as_ref(),
            "--timezone".as_ref(),
            "utc".as_ref(),
            "--author".as_ref(),
            "A@X".as_ref(),
        ],
        f.dir.path(),
    );
    assert!(
        view.status.success(),
        "{}",
        String::from_utf8_lossy(&view.stderr)
    );
    assert!(String::from_utf8_lossy(&view.stdout).contains("a@x"));
    assert!(!String::from_utf8_lossy(&view.stdout).contains("b@x"));
    let invalid = cli(
        &[
            "--timezone".as_ref(),
            "Mars/Olympus".as_ref(),
            "export".as_ref(),
        ],
        f.dir.path(),
    );
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("timezone"));
}

#[test]
fn filtered_report_labels_selected_totals_and_active_selection() {
    let f = Fixture::new();
    f.commit("a", b"one\n", "a@x", "2024-01-01T10:00:00 +0000");
    f.commit("b", b"two\n", "b@x", "2024-01-02T10:00:00 +0000");
    let out = tempdir();
    let output = out.path().join("selected");
    let selected = [
        "--since".as_ref(),
        "2024-01-01".as_ref(),
        "--until".as_ref(),
        "2024-01-01".as_ref(),
        "--author".as_ref(),
        "a@x".as_ref(),
        "--timezone".as_ref(),
        "utc".as_ref(),
        "--no-merges".as_ref(),
    ];
    let mut report_args = selected.to_vec();
    report_args.extend(["--no-png".as_ref(), "--output".as_ref(), output.as_os_str()]);
    let report = cli(&report_args, f.dir.path());
    assert!(
        report.status.success(),
        "{}",
        String::from_utf8_lossy(&report.stderr)
    );
    let stdout = String::from_utf8_lossy(&report.stdout);
    assert!(
        stdout.contains("Selected 1 commit · 1 contributor"),
        "{stdout}"
    );
    assert!(stdout.contains("selected additions"), "{stdout}");
    for label in [
        "Since 2024-01-01",
        "Until 2024-01-01",
        "Authors: a@x",
        "Timezone: utc",
        "Merges excluded",
    ] {
        assert!(stdout.contains(label), "missing {label}: {stdout}");
    }

    let svg = fs::read_to_string(output.join("summary.svg")).unwrap();
    for label in [
        "Selected commits",
        "Selected contributors",
        "Selected additions",
        "Selected deletions",
        "Since 2024-01-01",
        "Until 2024-01-01",
        "Authors: a@x",
        "Timezone: utc",
        "Merges excluded",
    ] {
        assert!(svg.contains(label), "missing {label}");
    }
    assert!(!svg.contains("Lifetime additions"));

    let mut export_args = selected.to_vec();
    export_args.push("export".as_ref());
    let export = cli(&export_args, f.dir.path());
    assert!(
        export.status.success(),
        "{}",
        String::from_utf8_lossy(&export.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&export.stdout).unwrap();
    assert_eq!(json["repository"]["total_commits"], 1);
    assert_eq!(json["repository"]["selected_since"], "2024-01-01");
    assert_eq!(json["repository"]["selected_until"], "2024-01-01");
    assert_eq!(
        json["repository"]["selected_authors"],
        serde_json::json!(["a@x"])
    );
    assert_eq!(json["repository"]["timezone"], "utc");
    assert_eq!(json["repository"]["include_merges"], false);
}

#[test]
fn config_timezone_applies_unless_cli_overrides_it() {
    let f = Fixture::new();
    f.commit("late", b"late\n", "a@x", "2024-01-01T23:30:00 -0800");
    fs::write(
        f.dir.path().join(".git-wrapped.json"),
        r#"{"timezone":"utc"}"#,
    )
    .unwrap();
    let configured = cli(
        &["--since".as_ref(), "2024-01-02".as_ref(), "export".as_ref()],
        f.dir.path(),
    );
    assert!(
        configured.status.success(),
        "{}",
        String::from_utf8_lossy(&configured.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&configured.stdout).unwrap();
    assert_eq!(json["repository"]["total_commits"], 1);
    assert_eq!(json["repository"]["timezone"], "utc");
    let overridden = cli(
        &[
            "--since".as_ref(),
            "2024-01-02".as_ref(),
            "--timezone".as_ref(),
            "commit".as_ref(),
            "export".as_ref(),
        ],
        f.dir.path(),
    );
    assert!(!overridden.status.success());
    assert!(String::from_utf8_lossy(&overridden.stderr).contains("no selected commits"));
    fs::write(
        f.dir.path().join(".git-wrapped.json"),
        r#"{"timezone":"Mars/Olympus"}"#,
    )
    .unwrap();
    let invalid = cli(&["export".as_ref()], f.dir.path());
    assert!(!invalid.status.success());
    let error = String::from_utf8_lossy(&invalid.stderr);
    assert!(
        error.contains(".git-wrapped.json") && error.contains("timezone"),
        "{error}"
    );
    let override_invalid = cli(
        &["--timezone".as_ref(), "utc".as_ref(), "export".as_ref()],
        f.dir.path(),
    );
    assert!(
        override_invalid.status.success(),
        "{}",
        String::from_utf8_lossy(&override_invalid.stderr)
    );
}

#[test]
fn focused_views_sort_and_sanitize() {
    let f = Fixture::new();
    f.commit("a", b"one\n", "b@x", "2024-01-01T10:00:00 +0000");
    f.commit("b", b"one\n", "a@x", "2024-01-02T10:00:00 +0000");
    f.commit("c", b"one\n", "a@x", "2024-01-03T10:00:00 +0000");
    let output = cli(
        &["contributors".as_ref(), "--by".as_ref(), "commits".as_ref()],
        f.dir.path(),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("commits"));
    assert!(text.find("a@x").unwrap() < text.find("b@x").unwrap());
    assert!(!text.contains('\x1b'));

    f.commit("d", b"one\n", "b@x", "2024-01-04T10:00:00 +0000");
    assert!(f
        .git(&[
            "commit",
            "--amend",
            "--author",
            "Bad\x1b[31m <b@x>",
            "--no-edit"
        ])
        .status
        .success());
    let output = cli(&["top".as_ref(), "files".as_ref()], f.dir.path());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout.clone()).unwrap();
    assert!(text.contains("files") && text.contains("b@x"), "{text}");
    assert!(text.contains("Bad [31m"), "{text}");
    assert!(!output.stdout.contains(&0x1b));
}

#[test]
fn report_summary_names_top_contributor_peak_and_award() {
    let f = Fixture::new();
    f.commit("a", b"one\n", "a@x", "2024-01-01T10:00:00 +0000");
    f.commit("b", b"one\n", "b@x", "2024-01-02T10:00:00 +0000");
    f.commit("c", b"one\n", "a@x", "2024-01-02T11:00:00 +0000");
    assert!(f
        .git(&[
            "commit",
            "--amend",
            "--author",
            "A\x1b[31m <a@x>",
            "--no-edit"
        ])
        .status
        .success());
    for args in [vec![], vec!["report".as_ref()]] {
        let output = cli(&args, f.dir.path());
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(
            text.contains("Top contributors:\n  A [31m (a@x): 2 commits\n  Test (b@x): 1 commit\n"),
            "{text}"
        );
        assert!(text.contains("Peak day: 2024-01-02 (2 commits)"), "{text}");
        assert!(text.contains("Commit Machine: A [31m"), "{text}");
        assert!(!text.contains('\x1b'));
    }
}

#[test]
fn contributor_lookup_reports_ambiguous_and_missing_names() {
    let f = Fixture::new();
    f.commit("a", b"one\n", "a@x", "2024-01-01T10:00:00 +0000");
    f.commit("b", b"one\n", "b@x", "2024-01-02T10:00:00 +0000");
    let ambiguous = cli(&["contributor".as_ref(), "tEsT".as_ref()], f.dir.path());
    assert!(!ambiguous.status.success());
    let error = String::from_utf8_lossy(&ambiguous.stderr);
    assert!(error.contains("ambiguous"), "{error}");
    assert!(error.contains("a@x") && error.contains("b@x"), "{error}");
    let missing = cli(&["contributor".as_ref(), "Nobody".as_ref()], f.dir.path());
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("not found"));
    let exact = cli(&["contributor".as_ref(), "a@x".as_ref()], f.dir.path());
    assert!(
        exact.status.success(),
        "{}",
        String::from_utf8_lossy(&exact.stderr)
    );
    assert!(String::from_utf8_lossy(&exact.stdout).contains("a@x"));
}

#[test]
fn activity_buckets_fill_sparse_periods_and_reject_invalid_name() {
    let f = Fixture::new();
    f.commit("a", b"one\n", "a@x", "2024-01-01T10:00:00 +0000");
    f.commit("b", b"one\n", "a@x", "2024-07-01T10:00:00 +0000");
    let quarter = cli(
        &["activity".as_ref(), "--bucket".as_ref(), "quarter".as_ref()],
        f.dir.path(),
    );
    assert!(
        quarter.status.success(),
        "{}",
        String::from_utf8_lossy(&quarter.stderr)
    );
    let text = String::from_utf8(quarter.stdout).unwrap();
    assert!(text.contains("2024-Q1\t1"), "{text}");
    assert!(text.contains("2024-Q2\t0"), "{text}");
    assert!(text.contains("2024-Q3\t1"), "{text}");
    let invalid = cli(
        &["activity".as_ref(), "--bucket".as_ref(), "decade".as_ref()],
        f.dir.path(),
    );
    assert_eq!(invalid.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("invalid value"));
}

#[test]
fn archaeology_lists_binary_only_path_with_zero_churn() {
    let f = Fixture::new();
    f.commit("image.bin", b"\0\x01", "a@x", "2024-01-01T10:00:00 +0000");
    let output = cli(&["archaeology".as_ref()], f.dir.path());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("image.bin"), "{text}");
    assert!(text.contains("\t0\t"), "{text}");
}

#[test]
fn contributor_tenure_overlap_words_and_trees_use_selected_history() {
    let f = Fixture::new();
    f.commit("a", b"a\n", "a@x", "2024-01-01T10:00:00 +0000");
    assert!(f
        .git(&["commit", "--amend", "-qm", "Repair\u{1b}repair repair"])
        .status
        .success());
    f.commit("d", b"d\n", "a@x", "2024-01-02T10:00:00 +0000");
    f.commit("b", b"b\n", "b@x", "2024-01-03T10:00:00 +0000");
    f.commit("c", b"c\n", "a@x", "2024-04-02T10:00:00 +0000");
    let x = analyze(&discover(f.dir.path()).unwrap(), &Config::default()).unwrap();
    let a = x.contributors.iter().find(|c| c.id == "a@x").unwrap();
    assert_eq!(a.tenure_days, 92);
    assert_eq!(a.longest_streak, 2);
    assert_eq!(a.first_seen_month, "2024-01");
    assert_eq!(a.returning_after_90_days, 1);
    assert_eq!(x.newcomers_by_month[0].label, "2024-01");
    assert_eq!(x.newcomers_by_month[0].count, 2);
    assert_eq!(
        (
            x.collaboration_overlap[0].first_id.as_str(),
            x.collaboration_overlap[0].second_id.as_str(),
            x.collaboration_overlap[0].weeks
        ),
        ("a@x", "b@x", 1)
    );
    assert_eq!(
        x.subject_words
            .iter()
            .find(|w| w.word == "repair")
            .unwrap()
            .count,
        3
    );
    assert!(x
        .subject_words
        .iter()
        .all(|w| !w.word.chars().any(char::is_control)));
    assert!(x.tree_samples.is_empty());
    assert_eq!(
        sample_trees(
            &discover(f.dir.path()).unwrap(),
            &x.commits,
            TimezoneChoice::Commit,
            &[],
            &CancelFlag::default(),
            None
        )
        .unwrap()
        .iter()
        .map(|s| s.tracked_files)
        .collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );
}

#[test]
fn deep_tree_samples_include_endpoints_and_stop_at_twenty_four() {
    let f = Fixture::new();
    for day in 1..=25 {
        f.commit(
            "file",
            format!("{day}\n").as_bytes(),
            "a@x",
            &format!("2024-01-{day:02}T10:00:00 +0000"),
        );
    }
    let repo = discover(f.dir.path()).unwrap();
    let x = analyze(&repo, &Config::default()).unwrap();
    let samples = sample_trees(
        &repo,
        &x.commits,
        TimezoneChoice::Commit,
        &[],
        &CancelFlag::default(),
        None,
    )
    .unwrap();
    assert!(x.tree_samples.is_empty());
    assert_eq!(samples.len(), 24);
    assert_eq!(samples.first().unwrap().author_date, "2024-01-01");
    assert_eq!(samples.last().unwrap().author_date, "2024-01-25");
}

#[test]
fn overlap_and_vocabulary_caps_keep_deterministic_ties() {
    let f = Fixture::new();
    for index in 0..21 {
        f.commit(
            &format!("file{index}"),
            b"a\n",
            &format!("person{index:02}@x"),
            "2024-01-01T10:00:00 +0000",
        );
        if index == 0 {
            let subject = (0..31)
                .map(|word| format!("topic{word:02} topic{word:02} topic{word:02}"))
                .collect::<Vec<_>>()
                .join(" ");
            assert!(f
                .git(&["commit", "--amend", "-qm", &subject])
                .status
                .success());
        }
    }
    let x = analyze(&discover(f.dir.path()).unwrap(), &Config::default()).unwrap();
    assert_eq!(x.collaboration_overlap.len(), 190);
    assert!(x
        .collaboration_overlap
        .iter()
        .all(|row| row.first_id != "person20@x" && row.second_id != "person20@x"));
    assert_eq!(x.subject_words.len(), 30);
    assert_eq!(x.subject_words.first().unwrap().word, "fixture");
    assert_eq!(x.subject_words[1].word, "topic00");
    assert_eq!(x.subject_words.last().unwrap().word, "topic28");
}

#[test]
fn streaks_and_peaks_use_author_calendar_not_wall_clock() {
    let f = Fixture::new();
    f.commit("a", b"a\n", "a@x", "2024-01-01T23:00:00 -0800");
    f.commit("b", b"b\n", "a@x", "2024-01-02T00:01:00 +1400");
    f.commit("c", b"c\n", "a@x", "2024-03-01T12:00:00 +0000");
    let x = analyze(&discover(f.dir.path()).unwrap(), &Config::default()).unwrap();
    assert_eq!(x.insights.longest_streak, 2);
    assert_eq!(x.insights.current_streak, 1);
    assert_eq!(x.insights.busiest_day.as_ref().unwrap().label, "2024-01-01");
    assert_eq!(x.insights.busiest_month.as_ref().unwrap().label, "2024-01");
    assert_eq!(x.insights.peak_hour.as_ref().unwrap().label, "00");
    assert_eq!(x.activity_by_week.len(), 9);
    assert!((x.insights.burstiness.unwrap() - 2.0).abs() < 1e-12);
    let days = activity_by_day(&x).unwrap();
    assert_eq!(days.len(), 61);
    assert_eq!(days[2].date, "2024-01-03");
    assert_eq!(days[2].commits, 0);
}

#[test]
fn sparse_history_zero_fills_months_without_storing_days() {
    let f = Fixture::new();
    f.commit("b", b"b\n", "a@x", "2024-01-01T12:00:00 +0000");
    let mut x = analyze(&discover(f.dir.path()).unwrap(), &Config::default()).unwrap();
    // Git rejects pre-epoch author dates; use a sparse historical analytics fixture.
    x.activity.insert(
        0,
        git_wrapped::model::Activity {
            month: "1960-01".into(),
            commits: 1,
            additions: 1,
            deletions: 0,
        },
    );
    x.activity_heatmap.insert(
        0,
        git_wrapped::model::ActivityCell {
            date: "1960-01-01".into(),
            commits: 1,
        },
    );
    let months = activity_by_month(&x).unwrap();
    assert_eq!(months.len(), 769);
    assert_eq!(months[0].month, "1960-01");
    assert_eq!(months[1].month, "1960-02");
    assert_eq!(months[1].commits, 0);
    assert_eq!(months.last().unwrap().month, "2024-01");
    assert_eq!(x.activity_heatmap.len(), 2);
    assert_eq!(x.activity_by_week.len(), 1);
    assert!(activity_by_day(&x).unwrap_err().contains("20,000"));
    assert!(serde_json::to_value(&x)
        .unwrap()
        .get("activity_by_day")
        .is_none());
}

#[test]
fn weekly_activity_rejects_more_than_twenty_thousand_buckets() {
    let f = Fixture::new();
    f.commit("a", b"a\n", "a@x", "2024-01-01T12:00:00 +0000");
    // Git accepts the raw positive epoch for 2500 even though it rejects ISO dates there.
    f.commit("b", b"b\n", "a@x", "@16725268800 +0000");
    let error = analyze(&discover(f.dir.path()).unwrap(), &Config::default()).unwrap_err();
    assert!(
        error.contains("weekly activity exceeds 20,000 buckets"),
        "{error}"
    );
}

#[test]
fn reachable_lightweight_and_annotated_tags_set_release_cadence() {
    let f = Fixture::new();
    f.commit("a", b"a\n", "a@x", "2024-01-01T12:00:00 +0000");
    assert!(f.git(&["tag", "v1"]).status.success());
    f.commit("b", b"b\n", "a@x", "2024-01-11T12:00:00 +0000");
    assert!(f
        .git(&["tag", "-a", "v2", "-m", "release"])
        .status
        .success());
    f.commit("c", b"c\n", "a@x", "2024-01-21T12:00:00 +0000");
    assert!(f.git(&["tag", "unreachable"]).status.success());
    assert!(f.git(&["reset", "--hard", "HEAD~1"]).status.success());
    let repo = discover(f.dir.path()).unwrap();
    let tags = reachable_tag_dates(&repo).unwrap();
    assert_eq!(
        tags.iter().map(|tag| tag.name.as_str()).collect::<Vec<_>>(),
        ["v1", "v2"]
    );
    let x = analyze(&repo, &Config::default()).unwrap();
    assert_eq!(x.insights.first_tag_days, Some(0));
    assert_eq!(x.insights.release_interval_median_days, Some(10.0));
}

#[test]
fn release_cadence_counts_same_day_targets_once_each() {
    let f = Fixture::new();
    f.commit("a", b"a\n", "a@x", "2024-01-01T10:00:00 +0000");
    assert!(f.git(&["tag", "first"]).status.success());
    assert!(f.git(&["tag", "first-alias"]).status.success());
    f.commit("b", b"b\n", "a@x", "2024-01-01T12:00:00 +0000");
    assert!(f.git(&["tag", "second"]).status.success());
    let repo = discover(f.dir.path()).unwrap();
    assert_eq!(
        analyze(&repo, &Config::default())
            .unwrap()
            .insights
            .release_interval_median_days,
        None
    );
    f.commit("c", b"c\n", "a@x", "2024-01-02T10:00:00 +0000");
    assert!(f.git(&["tag", "third"]).status.success());
    let repo = discover(f.dir.path()).unwrap();
    let tags = reachable_tag_dates(&repo).unwrap();
    assert_eq!(tags.len(), 4);
    assert_eq!(tags[0].target_sha, tags[1].target_sha);
    assert_ne!(tags[1].target_sha, tags[2].target_sha);
    let cadence = analyze(&repo, &Config::default())
        .unwrap()
        .insights
        .release_interval_median_days;
    assert_eq!(cadence, Some(0.5));
}

#[test]
fn growth_churn_and_commit_records_use_canonical_counts() {
    let f = Fixture::new();
    f.commit("a", b"one\ntwo\n", "a@x", "2024-01-01T12:00:00 +0000");
    f.commit("a", b"one\n", "b@x", "2024-02-01T12:00:00 +0000");
    f.commit("b", b"new\n", "c@x", "2024-03-01T12:00:00 +0000");
    let x = analyze(&discover(f.dir.path()).unwrap(), &Config::default()).unwrap();
    let i = &x.insights;
    assert_eq!(i.highest_growth_month.as_ref().unwrap().label, "2024-01");
    assert_eq!(i.highest_growth_month.as_ref().unwrap().count, 2);
    assert_eq!(i.highest_churn_month.as_ref().unwrap().label, "2024-01");
    assert_eq!(i.largest_commit.as_ref().unwrap().additions, 2);
    assert_eq!(i.largest_cleanup.as_ref().unwrap().author_id, "b@x");
    assert_eq!(i.largest_cleanup.as_ref().unwrap().deletions, 1);
    let out = tempdir();
    git_wrapped::render::render_report(&x, out.path(), git_wrapped::render::Theme::Dark).unwrap();
    let highlights = fs::read_to_string(out.path().join("highlights.svg")).unwrap();
    assert!(highlights.contains("2024-01 grew by 2 historical net lines"));
    assert!(highlights.contains("deleted 1 historical line"));
}

#[test]
fn file_history_keeps_binary_rename_and_distinct_paths() {
    let f = Fixture::new();
    f.commit("old.txt", b"one\n", "a@x", "2024-01-01T10:00:00 +0000");
    assert!(f.git(&["mv", "old.txt", "new.txt"]).status.success());
    assert!(f.git(&["commit", "-qm", "rename"]).status.success());
    f.commit("image.bin", b"\0\x01", "b@x", "2024-01-03T10:00:00 +0000");
    let data = analyze(&discover(f.dir.path()).unwrap(), &Config::default()).unwrap();
    let renamed = data
        .files
        .iter()
        .find(|x| x.display_path == "new.txt")
        .unwrap();
    assert_eq!(renamed.revisions, 1);
    assert_eq!(renamed.rename_from, ["6f6c642e747874"]);
    assert!(renamed.exists_at_head);
    assert_eq!(
        data.files
            .iter()
            .find(|x| x.display_path == "image.bin")
            .unwrap()
            .churn,
        0
    );
    assert_eq!(
        data.extensions
            .iter()
            .find(|x| x.extension == "txt")
            .unwrap()
            .current_files,
        1
    );
    assert_eq!(
        data.extensions
            .iter()
            .find(|x| x.extension == "txt")
            .unwrap()
            .historical_churn,
        1
    );
    assert_eq!(
        data.directories
            .iter()
            .find(|x| x.display_path == ".")
            .unwrap()
            .current_file_count,
        2
    );
}

#[test]
fn directory_history_uses_immediate_parent_and_recursive_head_count() {
    let f = Fixture::new();
    f.commit("src/lib/a.rs", b"a\n", "a@x", "2024-01-01T10:00:00 +0000");
    f.commit(
        "src/lib/deep/b.rs",
        b"b\n",
        "b@x",
        "2024-01-02T10:00:00 +0000",
    );
    let data = analyze(&discover(f.dir.path()).unwrap(), &Config::default()).unwrap();
    let lib = data
        .directories
        .iter()
        .find(|dir| dir.display_path == "src/lib")
        .unwrap();
    assert_eq!(lib.commits, 1);
    assert_eq!(lib.churn, 1);
    assert_eq!(lib.current_file_count, 2);
    assert!(data
        .directories
        .iter()
        .all(|dir| dir.display_path != "src" && dir.display_path != "."));
}

#[cfg(unix)]
#[test]
fn file_history_distinguishes_non_utf8_paths_with_same_display() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let f = Fixture::new();
    let blob = f.git(&["hash-object", "-w", "/dev/null"]);
    assert!(blob.status.success());
    let blob = String::from_utf8(blob.stdout).unwrap();
    for byte in [0xfe, 0xff] {
        let path = OsString::from_vec(vec![b'x', byte]);
        assert!(Command::new("git")
            .arg("-C")
            .arg(f.dir.path())
            .args([
                "update-index",
                "--add",
                "--cacheinfo",
                "100644",
                blob.trim()
            ])
            .arg(path)
            .status()
            .unwrap()
            .success());
    }
    assert!(f.git(&["commit", "-qm", "paths"]).status.success());
    let data = analyze(&discover(f.dir.path()).unwrap(), &Config::default()).unwrap();
    assert_eq!(data.files.len(), 2);
    assert_eq!(data.files[0].display_path, data.files[1].display_path);
    assert!(data.files[0].path_id < data.files[1].path_id);
}

#[test]
fn explicit_repo_creates_first_report() {
    let f = Fixture::new();
    f.commit(
        "a.txt",
        b"a\n",
        "a@example.com",
        "2024-01-01T12:00:00 +0000",
    );
    let out = tempdir();
    let output = out.path().join("report with spaces");
    let result = cli(
        &[
            f.dir.path().as_os_str(),
            "--output".as_ref(),
            output.as_os_str(),
        ],
        out.path(),
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let mut entries: Vec<_> = fs::read_dir(&output)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    entries.sort();
    assert_eq!(
        entries,
        [
            ".git-wrapped-cache.json",
            "activity-heatmap.svg",
            "activity.svg",
            "additions-deletions.svg",
            "awards",
            "card-manifest.json",
            "commits-over-time.svg",
            "contributor-mix.svg",
            "contributors",
            "contributors.svg",
            "data.json",
            "directories.svg",
            "file-churn.svg",
            "highlights.svg",
            "rhythm.svg",
            "summary.png",
            "summary.svg"
        ]
    );
    assert!(output.join("awards/commit-machine.svg").is_file());
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("1 commit · 1 contributor"));
    assert!(stdout.contains("lifetime additions"));
    assert!(stdout.contains(output.to_str().unwrap()));
    assert!(result.stderr.is_empty());
}

#[test]
fn summary_png_is_real_scaled_and_optional() {
    let f = Fixture::new();
    f.commit(
        "a.txt",
        b"a\n",
        "a@example.com",
        "2024-01-01T12:00:00 +0000",
    );
    let data = analyze(&discover(f.dir.path()).unwrap(), &Config::default()).unwrap();
    let out = tempdir();
    git_wrapped::render::render_report(&data, out.path(), git_wrapped::render::Theme::Dark)
        .unwrap();
    let png = fs::read(out.path().join("summary.png")).unwrap();
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), 2400);
    assert_eq!(u32::from_be_bytes(png[20..24].try_into().unwrap()), 3200);

    let no_png = tempdir();
    let result = cli(
        &[
            f.dir.path().as_os_str(),
            "--output".as_ref(),
            no_png.path().as_os_str(),
            "--no-png".as_ref(),
        ],
        f.dir.path(),
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!no_png.path().join("summary.png").exists());
    assert!(no_png.path().join("summary.svg").exists());
}

#[cfg(unix)]
#[test]
fn summary_png_symlink_target_is_rejected() {
    let f = Fixture::new();
    f.commit(
        "a.txt",
        b"a\n",
        "a@example.com",
        "2024-01-01T12:00:00 +0000",
    );
    let data = analyze(&discover(f.dir.path()).unwrap(), &Config::default()).unwrap();
    let protected = tempfile::NamedTempFile::new().unwrap();
    fs::write(protected.path(), "untouched").unwrap();
    let out = tempdir();
    std::os::unix::fs::symlink(protected.path(), out.path().join("summary.png")).unwrap();
    assert!(git_wrapped::render::render_report(
        &data,
        out.path(),
        git_wrapped::render::Theme::Dark
    )
    .is_err());
    assert_eq!(fs::read_to_string(protected.path()).unwrap(), "untouched");
}

#[test]
fn default_and_report_commands_use_invocation_directory() {
    let f = Fixture::new();
    f.commit(
        "a.txt",
        b"a\n",
        "a@example.com",
        "2024-01-01T12:00:00 +0000",
    );
    for args in [vec![], vec!["report".as_ref()]] {
        let result = cli(&args, f.dir.path());
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(f.dir.path().join("git-wrapped-report/data.json").is_file());
    }
    let help = cli(&["--help".as_ref()], f.dir.path());
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("\nUsage:"));
    let ambiguous = cli(&[f.dir.path().as_os_str(), "report".as_ref()], f.dir.path());
    assert_eq!(ambiguous.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&ambiguous.stderr)
        .contains("repository path cannot precede a subcommand"));
}

#[test]
fn invalid_argument_does_not_print_terminal_controls() {
    let cwd = tempdir();
    let bad_theme = "bad\r\n\u{85}\x1b[31m";
    let result = cli(&["--theme".as_ref(), bad_theme.as_ref()], cwd.path());
    assert_eq!(result.status.code(), Some(2));
    assert!(!result.stderr.contains(&b'\r'));
    assert!(!result.stderr.contains(&0x1b));
    assert!(!String::from_utf8_lossy(&result.stderr).contains('\u{85}'));
    assert!(String::from_utf8_lossy(&result.stderr).contains("invalid value"));
    assert!(cli(&["--version".as_ref()], cwd.path()).status.success());
}

#[test]
fn export_is_valid_json_only() {
    let f = Fixture::new();
    f.commit(
        "a.txt",
        b"a\n",
        "a@example.com",
        "2024-01-01T12:00:00 +0000",
    );
    let result = cli(
        &[
            "export".as_ref(),
            "--format".as_ref(),
            "json".as_ref(),
            f.dir.path().as_os_str(),
        ],
        f.dir.path(),
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let data: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(data["repository"]["total_commits"], 1);
    assert!(!f.dir.path().join("git-wrapped-report").exists());
}

#[test]
fn cancelled_scan_returns_before_visiting_commits() {
    let f = Fixture::new();
    f.commit(
        "a.txt",
        b"a\n",
        "a@example.com",
        "2024-01-01T12:00:00 +0000",
    );
    let flag = CancelFlag::default();
    flag.cancel();
    let mut visited = false;
    let result = scan_with_cancel(&discover(f.dir.path()).unwrap(), &flag, |_| {
        visited = true;
        Ok(())
    });
    assert!(result.unwrap_err().contains("cancelled"));
    assert!(!visited);
}

#[test]
fn verbose_export_keeps_progress_off_stdout() {
    let f = Fixture::new();
    f.commit(
        "a.txt",
        b"a\n",
        "a@example.com",
        "2024-01-01T12:00:00 +0000",
    );
    let normal = cli(&["export".as_ref()], f.dir.path());
    assert!(normal.status.success());
    assert!(!String::from_utf8_lossy(&normal.stderr).contains("Progress:"));
    let verbose = cli(&["--verbose".as_ref(), "export".as_ref()], f.dir.path());
    assert!(verbose.status.success());
    assert!(verbose.stdout.starts_with(b"{"));
    assert!(!String::from_utf8_lossy(&verbose.stdout).contains("Progress:"));
    assert!(String::from_utf8_lossy(&verbose.stderr).contains("Progress:"));
}

#[test]
fn cli_errors_leave_output_intact() {
    let f = Fixture::new();
    let out = tempdir();
    let output = out.path().join("report");
    fs::create_dir(&output).unwrap();
    fs::write(output.join("sentinel"), b"keep").unwrap();
    let args = [
        f.dir.path().as_os_str(),
        "--output".as_ref(),
        output.as_os_str(),
    ];
    let empty = cli(&args, out.path());
    assert!(!empty.status.success());
    assert!(String::from_utf8_lossy(&empty.stderr).contains("no commits"));
    assert!(!output.join("data.json").exists());
    f.commit(
        "a.txt",
        b"a\n",
        "a@example.com",
        "2024-01-01T12:00:00 +0000",
    );
    fs::write(f.dir.path().join(".git-wrapped.json"), b"{").unwrap();
    let malformed = cli(&args, out.path());
    assert!(!malformed.status.success());
    assert!(String::from_utf8_lossy(&malformed.stderr).contains(".git-wrapped.json"));
    assert_eq!(fs::read(output.join("sentinel")).unwrap(), b"keep");
}

#[test]
fn terminal_output_sanitizes_control_characters_and_warns_for_shallow_history() {
    let f = Fixture::new();
    f.commit(
        "a.txt",
        b"a\n",
        "a@example.com",
        "2024-01-01T12:00:00 +0000",
    );
    let subject = "bad\x1b[31m subject";
    assert!(f
        .git(&["commit", "--allow-empty", "-qm", subject])
        .status
        .success());
    let output = cli(&["export".as_ref(), f.dir.path().as_os_str()], f.dir.path());
    assert!(output.status.success());
    assert!(!output.stderr.contains(&0x1b));
    let shallow = tempdir();
    let clone = shallow.path().join("clone");
    let cloned = Command::new("git")
        .args([
            "clone",
            "-q",
            "--depth",
            "1",
            &format!("file://{}", f.dir.path().display()),
            clone.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        cloned.status.success(),
        "{}",
        String::from_utf8_lossy(&cloned.stderr)
    );
    let result = cli(
        &[
            clone.as_os_str(),
            "--output".as_ref(),
            shallow.path().join("out").as_os_str(),
        ],
        shallow.path(),
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stderr).contains("available history"));
}

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
    assert_eq!(data.repository.age_days, 5);
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
    for slug in [
        "commit-machine",
        "code-creator",
        "code-destroyer",
        "net-positive",
        "night-owl",
        "early-bird",
        "weekend-warrior",
        "biggest-bang",
        "biggest-cleanup",
        "repo-explorer",
    ] {
        assert!(slugs.contains(&slug), "{slug}");
    }
    for award in &data.awards {
        assert!(!award.winner_id.is_empty());
        assert!(!award.winner.is_empty());
        assert!(!award.metric.is_empty());
        assert!(!award.value.is_empty());
        assert!(!award.explanation.is_empty());
    }
}

#[test]
fn concentration_uses_normalized_contributors() {
    let f = Fixture::new();
    f.commit("a", b"a\n", "old@example.com", "2024-01-01T10:00:00 +0000");
    f.commit(
        "b",
        b"b\n",
        "other@example.com",
        "2024-01-02T10:00:00 +0000",
    );
    f.commit(
        "c",
        b"c\n",
        "third@example.com",
        "2024-01-03T10:00:00 +0000",
    );
    f.commit(
        "d",
        b"d\n",
        "fourth@example.com",
        "2024-01-04T10:00:00 +0000",
    );
    let mut config = Config::default();
    config
        .insert_alias_group("Merged", &["old@example.com", "other@example.com"])
        .unwrap();
    let data = analyze(&discover(f.dir.path()).unwrap(), &config).unwrap();
    assert_eq!(data.contributors.len(), 3);
    assert_eq!(data.contributors[0].commits, 2);
    assert_eq!(data.insights.commit_concentration_50, 1);
    assert_eq!(
        serde_json::to_value(&data.insights).unwrap()["bus_factor_proxy"],
        1
    );
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
    let dir = tempdir();
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
    let dir = tempdir();
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
fn deepening_shallow_clone_invalidates_cache_with_unchanged_head() {
    let f = Fixture::new();
    f.commit("one", b"one\n", "a@x", "2024-01-01T10:00:00 +0000");
    f.commit("two", b"two\n", "a@x", "2024-01-02T10:00:00 +0000");
    f.commit("three", b"three\n", "a@x", "2024-01-03T10:00:00 +0000");
    let dir = tempdir();
    let clone = dir.path().join("clone");
    let cloned = Command::new("git")
        .args(["clone", "-q", "--depth=1"])
        .arg(format!("file://{}", f.dir.path().display()))
        .arg(&clone)
        .output()
        .unwrap();
    assert!(
        cloned.status.success(),
        "{}",
        String::from_utf8_lossy(&cloned.stderr)
    );
    let head = Command::new("git")
        .arg("-C")
        .arg(&clone)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap()
        .stdout;
    let out = dir.path().join("report");
    let args: &[&std::ffi::OsStr] = &[
        "--no-png".as_ref(),
        "--output".as_ref(),
        out.as_os_str(),
        "report".as_ref(),
    ];
    let run = || cli(args, &clone);
    assert!(run().status.success());
    assert!(String::from_utf8_lossy(&run().stderr).contains("Using cached analysis"));
    let initial: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("data.json")).unwrap()).unwrap();
    assert_eq!(initial["repository"]["total_commits"], 1);

    let deepen = Command::new("git")
        .arg("-C")
        .arg(&clone)
        .args(["fetch", "-q", "--deepen=1", "origin"])
        .output()
        .unwrap();
    assert!(
        deepen.status.success(),
        "{}",
        String::from_utf8_lossy(&deepen.stderr)
    );
    assert_eq!(
        Command::new("git")
            .arg("-C")
            .arg(&clone)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
        head
    );
    let deeper = run();
    assert!(deeper.status.success());
    assert!(!String::from_utf8_lossy(&deeper.stderr).contains("Using cached analysis"));
    let data: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("data.json")).unwrap()).unwrap();
    assert_eq!(data["repository"]["total_commits"], 2);

    let unshallow = Command::new("git")
        .arg("-C")
        .arg(&clone)
        .args(["fetch", "-q", "--unshallow", "origin"])
        .output()
        .unwrap();
    assert!(
        unshallow.status.success(),
        "{}",
        String::from_utf8_lossy(&unshallow.stderr)
    );
    assert_eq!(
        Command::new("git")
            .arg("-C")
            .arg(&clone)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
        head
    );
    let complete = run();
    assert!(complete.status.success());
    assert!(!String::from_utf8_lossy(&complete.stderr).contains("Using cached analysis"));
    let data: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("data.json")).unwrap()).unwrap();
    assert_eq!(data["repository"]["total_commits"], 3);
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
    let out = tempdir();
    let other = tempdir();
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
        let linked = tempdir();
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

#[test]
fn render_heatmap_keeps_multidecade_history_readable() {
    use git_wrapped::{
        model::ActivityCell,
        render::{render_report, Theme},
    };
    let f = Fixture::new();
    f.commit("a", b"a\n", "a@x", "2024-12-31T12:00:00 +0000");
    let mut data = analyze(&discover(f.dir.path()).unwrap(), &Config::default()).unwrap();
    data.repository.first_commit = "1960-01-01T12:00:00+00:00".into();
    data.activity_heatmap = ["1960-01-01", "2024-01-01", "2024-01-02", "2024-12-31"]
        .into_iter()
        .map(|date| ActivityCell {
            date: date.into(),
            commits: 1,
        })
        .collect();
    let out = tempdir();
    render_report(&data, out.path(), Theme::Dark).unwrap();
    let svg = std::fs::read_to_string(out.path().join("activity-heatmap.svg")).unwrap();
    assert!(svg.contains("Trailing 365 days · 2024-01-02 — 2024-12-31"));
    assert!(!svg.contains("1960-01-01"));
    assert!(!svg.contains("<title>2024-01-01:"));
    assert!(svg.contains("<title>2024-01-02: 1 commits</title>"));
    assert!(svg.contains("<title>2024-12-31: 1 commits</title>"));
    assert!(svg.contains(">Jan</text>") && svg.contains(">Dec</text>"));
    for size in svg.split("font-size=\"").skip(1) {
        assert!(size.split('"').next().unwrap().parse::<u32>().unwrap() >= 16);
    }
    for rect in svg.split("<rect ").skip(2) {
        for attr in ["width=\"", "height=\""] {
            let value = rect.split(attr).nth(1).unwrap().split('"').next().unwrap();
            assert!(value.parse::<f64>().unwrap() >= 16.0);
        }
    }
}

#[test]
fn export_never_runs_repository_signature_program() {
    use std::{io::Write, os::unix::fs::PermissionsExt, process::Stdio};
    let f = Fixture::new();
    f.commit("a", b"a\n", "a@x", "2024-01-01T12:00:00 +0000");
    let tree = String::from_utf8(f.git(&["rev-parse", "HEAD^{tree}"]).stdout).unwrap();
    let commit = format!("tree {}\nauthor Test <a@x> 1704110400 +0000\ncommitter Test <a@x> 1704110400 +0000\ngpgsig -----BEGIN PGP SIGNATURE-----\n fake\n -----END PGP SIGNATURE-----\n\nfixture\n", tree.trim());
    let mut hash = Command::new("git")
        .arg("-C")
        .arg(f.dir.path())
        .args(["hash-object", "-t", "commit", "-w", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    hash.stdin
        .take()
        .unwrap()
        .write_all(commit.as_bytes())
        .unwrap();
    let hashed = hash.wait_with_output().unwrap();
    assert!(hashed.status.success());
    let sha = String::from_utf8(hashed.stdout).unwrap();
    assert!(f.git(&["update-ref", "HEAD", sha.trim()]).status.success());
    let program = f.dir.path().join("verify-signature");
    fs::write(
        &program,
        "#!/bin/sh\nprintf executed > signature-marker\nexit 1\n",
    )
    .unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(f
        .git(&["config", "gpg.program", program.to_str().unwrap()])
        .status
        .success());
    assert!(f
        .git(&["config", "log.showSignature", "true"])
        .status
        .success());
    let result = cli(&["export".as_ref()], f.dir.path());
    assert!(
        !f.dir.path().join("signature-marker").exists(),
        "repository signature program executed"
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let data: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(data["repository"]["total_commits"], 1);
}

#[test]
fn default_report_rejects_symlink_root_before_writes() {
    let f = Fixture::new();
    f.commit("a", b"a\n", "a@x", "2024-01-01T12:00:00 +0000");
    let external = tempdir();
    fs::write(external.path().join("summary.svg"), "untouched").unwrap();
    std::os::unix::fs::symlink(external.path(), f.dir.path().join("git-wrapped-report")).unwrap();
    let result = cli(&[], f.dir.path());
    assert_eq!(
        fs::read_to_string(external.path().join("summary.svg")).unwrap(),
        "untouched"
    );
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("symlink"));
    assert_eq!(fs::read_dir(external.path()).unwrap().count(), 1);
}

fn cross_offset_data() -> git_wrapped::model::RepositoryAnalytics {
    let f = Fixture::new();
    f.commit("a", b"a\n", "a@x", "2024-01-02T00:30:00 +1400");
    f.commit("b", b"b\n", "a@x", "2024-01-01T23:30:00 -1200");
    analyze(&discover(f.dir.path()).unwrap(), &Config::default()).unwrap()
}

#[test]
fn growth_rhythm_and_highlights_use_canonical_units() {
    let mut data = cross_offset_data();
    data.activity.push(git_wrapped::model::Activity {
        month: "2024-03".into(),
        commits: 1,
        additions: 3,
        deletions: 1,
    });
    let out = tempdir();
    git_wrapped::render::render_report(&data, out.path(), git_wrapped::render::Theme::Light)
        .unwrap();
    let growth = fs::read_to_string(out.path().join("additions-deletions.svg")).unwrap();
    for label in ["Lines added", "Lines deleted", "Cumulative historical net"] {
        assert!(growth.contains(label), "{label}");
    }
    assert!(growth.contains("<title>2024-02: +0 additions, −0 deletions"));
    let commits = fs::read_to_string(out.path().join("commits-over-time.svg")).unwrap();
    assert!(commits.contains("<title>2024-02: 0 commits</title>"));
    let rhythm = fs::read_to_string(out.path().join("rhythm.svg")).unwrap();
    assert!(rhythm.contains("Author local hour"));
    assert!(rhythm.contains("Monday"));
    assert!(rhythm.contains("<title>Monday 23:00: 1 commits</title>"));
    assert!(rhythm.contains("<title>Tuesday 00:00: 1 commits</title>"));
    let highlights = fs::read_to_string(out.path().join("highlights.svg")).unwrap();
    assert!(highlights.contains("2024-01-01"));
    assert!(highlights.contains("new contributor"));
    assert!(highlights.contains("1 new contributor"));
    assert!(!highlights.contains("largest single cleanup"));
    assert!(!highlights.contains("oldest surviving"));
}

#[test]
fn new_charts_handle_empty_one_month_and_sixty_years() {
    let mut data = cross_offset_data();
    let out = tempdir();
    for activity in [vec![], data.activity.clone()] {
        data.activity = activity;
        git_wrapped::render::render_report(&data, out.path(), git_wrapped::render::Theme::Dark)
            .unwrap();
        for name in [
            "commits-over-time.svg",
            "additions-deletions.svg",
            "rhythm.svg",
        ] {
            let svg = fs::read_to_string(out.path().join(name)).unwrap();
            assert!(!svg.contains("NaN") && !svg.contains("inf"), "{name}");
        }
    }
    data.activity.insert(
        0,
        git_wrapped::model::Activity {
            month: "1960-01".into(),
            commits: 1,
            additions: 1,
            deletions: 0,
        },
    );
    git_wrapped::render::render_report(&data, out.path(), git_wrapped::render::Theme::Dark)
        .unwrap();
    for name in ["commits-over-time.svg", "additions-deletions.svg"] {
        let svg = fs::read_to_string(out.path().join(name)).unwrap();
        assert!(svg.contains("<title>1960-02:"), "{name}");
        assert!(svg.matches("data-x-label=").count() <= 12, "{name}");
        assert!(!svg.contains("NaN") && !svg.contains("inf"), "{name}");
    }
}

#[test]
fn poster_contains_rich_sections_and_escapes_long_names() {
    let mut data = cross_offset_data();
    data.repository.name = "<script>&report".into();
    data.contributors[0].name = format!("{}<script>", "A".repeat(80));
    let out = tempdir();
    git_wrapped::render::render_report(&data, out.path(), git_wrapped::render::Theme::Dark)
        .unwrap();
    let svg = fs::read_to_string(out.path().join("summary.svg")).unwrap();
    assert!(svg.contains("viewBox=\"0 0 1200 1600\""));
    for label in ["GROWTH", "PEOPLE", "RHYTHM", "AWARDS", "Lifetime additions"] {
        assert!(svg.contains(label), "missing {label}");
    }
    assert!(svg.contains("&lt;script&gt;&amp;report"));
    assert!(!svg.contains("<script>"));
    assert!(!svg.contains(&"A".repeat(80)));
}

#[cfg(unix)]
#[test]
fn report_rejects_symlinked_awards_directory_and_json_target() {
    let data = cross_offset_data();
    let out = tempdir();
    let external = tempdir();
    std::os::unix::fs::symlink(external.path(), out.path().join("awards")).unwrap();
    assert!(git_wrapped::render::render_report(
        &data,
        out.path(),
        git_wrapped::render::Theme::Dark
    )
    .is_err());
    assert_eq!(fs::read_dir(external.path()).unwrap().count(), 0);
    fs::remove_file(out.path().join("awards")).unwrap();
    let protected = tempfile::NamedTempFile::new().unwrap();
    fs::write(protected.path(), "untouched").unwrap();
    std::os::unix::fs::symlink(protected.path(), out.path().join("data.json")).unwrap();
    assert!(git_wrapped::render::render_report(
        &data,
        out.path(),
        git_wrapped::render::Theme::Dark
    )
    .is_err());
    assert_eq!(fs::read_to_string(protected.path()).unwrap(), "untouched");
}

#[test]
fn cross_offset_age_counts_elapsed_full_days() {
    let data = cross_offset_data();
    // The instants are 25 hours apart, despite reversed local dates.
    assert_eq!(data.repository.age_days, 1);
    assert_eq!(data.contributors[0].commits_by_hour[0], 1);
    assert_eq!(data.contributors[0].commits_by_hour[23], 1);
    assert_eq!(data.contributors[0].tenure_days, 1);
    assert_eq!(data.contributors[0].longest_streak, 2);
}

#[test]
fn cross_offset_heatmap_includes_latest_activity_date() {
    let data = cross_offset_data();
    let out = tempdir();
    git_wrapped::render::render_report(&data, out.path(), git_wrapped::render::Theme::Dark)
        .unwrap();
    let svg = fs::read_to_string(out.path().join("activity-heatmap.svg")).unwrap();
    assert!(svg.contains("<title>2024-01-01: 1 commits</title>"));
    assert!(svg.contains("<title>2024-01-02: 1 commits</title>"));
}

#[test]
fn activity_maximum_bar_reaches_axis_maximum() {
    let data = cross_offset_data();
    let out = tempdir();
    git_wrapped::render::render_report(&data, out.path(), git_wrapped::render::Theme::Dark)
        .unwrap();
    let svg = fs::read_to_string(out.path().join("activity.svg")).unwrap();
    assert!(svg.contains("<text x=\"64\" y=\"200\""));
    assert!(svg.contains("<rect x=\"110.00\" y=\"200.00\" width=\"808.00\" height=\"470.00\""));
}

#[test]
fn summary_growth_band_labels_calendar_gaps() {
    let mut data = cross_offset_data();
    let out = tempdir();
    git_wrapped::render::render_report(&data, out.path(), git_wrapped::render::Theme::Dark)
        .unwrap();
    let svg = fs::read_to_string(out.path().join("summary.svg")).unwrap();
    assert!(svg.contains("2024-01: +2 additions, −0 deletions"));
    assert!(!svg.contains("2024-02:"));
    data.activity.push(git_wrapped::model::Activity {
        month: "2024-03".into(),
        commits: 1,
        additions: 3,
        deletions: 1,
    });
    git_wrapped::render::render_report(&data, out.path(), git_wrapped::render::Theme::Dark)
        .unwrap();
    let svg = fs::read_to_string(out.path().join("summary.svg")).unwrap();
    assert!(svg.contains("2024-01 → 2024-03"));
    assert!(svg.contains("2024-02: +0 additions, −0 deletions"));
    assert!(svg.contains("2024-03: +3 additions, −1 deletions"));
}

#[test]
fn summary_explains_months_without_line_changes() {
    let mut data = cross_offset_data();
    for month in &mut data.activity {
        month.additions = 0;
        month.deletions = 0;
    }
    let out = tempdir();
    git_wrapped::render::render_report(&data, out.path(), git_wrapped::render::Theme::Dark)
        .unwrap();
    let svg = fs::read_to_string(out.path().join("summary.svg")).unwrap();
    assert!(svg.contains("No line changes to chart"));
}

#[test]
fn summary_labels_each_visible_contributor_share() {
    let f = Fixture::new();
    for i in 0..4 {
        f.commit(
            &format!("{i}.txt"),
            b"a\n",
            &format!("{i}@example.com"),
            "2024-01-01T12:00:00 +0000",
        );
    }
    let mut data = analyze(&discover(f.dir.path()).unwrap(), &Config::default()).unwrap();
    for (i, contributor) in data.contributors.iter_mut().enumerate() {
        contributor.name = format!("Contributor {i}");
    }
    let out = tempdir();
    git_wrapped::render::render_report(&data, out.path(), git_wrapped::render::Theme::Dark)
        .unwrap();
    let svg = fs::read_to_string(out.path().join("summary.svg")).unwrap();
    for i in 0..4 {
        assert!(svg.contains(&format!("Contributor {i}")));
    }
}

#[test]
fn gallery_cards_are_stable_and_paths_are_fixed() {
    let mut data = cross_offset_data();
    data.contributors[0].id = "abcdefgh-one@example.com".into();
    data.contributors[0].name = "../<Same & Name>".into();
    let mut second = data.contributors[0].clone();
    second.id = "abcdefgh-two@example.com".into();
    data.contributors.push(second);
    data.awards.push(git_wrapped::model::Award {
        slug: "repo-explorer".into(),
        title: "Repo Explorer".into(),
        winner_id: "abcdefgh-one@example.com".into(),
        winner: "../<Same & Name>".into(),
        metric: "files touched".into(),
        value: "2".into(),
        explanation: "Touched two paths".into(),
    });
    data.files[0].exists_at_head = false;
    let first = tempdir();
    let second_out = tempdir();
    for out in [first.path(), second_out.path()] {
        git_wrapped::render::render_report(&data, out, git_wrapped::render::Theme::Dark).unwrap();
        for name in [
            "contributor-mix.svg",
            "file-churn.svg",
            "directories.svg",
            "awards/repo-explorer.svg",
        ] {
            assert!(out.join(name).is_file(), "missing {name}");
        }
        let names: Vec<_> = fs::read_dir(out.join("contributors"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"6162636465666768.svg".to_string()));
        assert!(names.contains(&"6162636465666768-2.svg".to_string()));
        let card = fs::read_to_string(out.join("contributors/6162636465666768.svg")).unwrap();
        assert!(card.contains("&lt;Same &amp; Name&gt;"));
        assert!(card.contains("Active days"));
        assert!(card.contains("Author local hour"));
        assert!(!card.contains("<Same & Name>"));
        assert!(fs::read_to_string(out.join("file-churn.svg"))
            .unwrap()
            .contains("historical path"));
        assert!(!out
            .join("contributors/..")
            .join("<Same & Name>.svg")
            .exists());
    }
    for name in [
        "contributor-mix.svg",
        "file-churn.svg",
        "directories.svg",
        "contributors/6162636465666768.svg",
        "contributors/6162636465666768-2.svg",
        "awards/repo-explorer.svg",
    ] {
        assert_eq!(
            fs::read(first.path().join(name)).unwrap(),
            fs::read(second_out.path().join(name)).unwrap(),
            "nondeterministic {name}"
        );
    }
}

#[test]
fn rerender_removes_only_stale_generated_cards() {
    use git_wrapped::render::{render_report, Theme};
    let mut data = cross_offset_data();
    data.contributors[0].id = "alpha@x".into();
    let mut other = data.contributors[0].clone();
    other.id = "beta@x".into();
    data.contributors.push(other);
    #[cfg(unix)]
    {
        let mut linked = data.contributors[0].clone();
        linked.id = "gamma@x".into();
        data.contributors.push(linked);
    }
    data.awards.push(git_wrapped::model::Award {
        slug: "repo-explorer".into(),
        title: "Repo Explorer".into(),
        winner_id: "beta@x".into(),
        winner: "Beta".into(),
        metric: "files touched".into(),
        value: "1".into(),
        explanation: "One file".into(),
    });
    let out = tempdir();
    render_report(&data, out.path(), Theme::Dark).unwrap();
    let stale_contributor = out.path().join("contributors/626574614078.svg");
    let stale_award = out.path().join("awards/repo-explorer.svg");
    assert!(stale_contributor.exists() && stale_award.exists());
    fs::write(out.path().join("contributors/notes.svg"), "keep").unwrap();
    fs::write(out.path().join("awards/notes.svg"), "keep").unwrap();
    #[cfg(unix)]
    {
        let target = out.path().join("target.txt");
        fs::write(&target, "keep").unwrap();
        std::os::unix::fs::symlink(&target, out.path().join("contributors/linked.svg")).unwrap();
        let generated_link = out.path().join("contributors/67616d6d614078.svg");
        fs::remove_file(&generated_link).unwrap();
        std::os::unix::fs::symlink(&target, generated_link).unwrap();
    }
    data.contributors.pop();
    #[cfg(unix)]
    data.contributors.pop();
    data.awards.clear();
    render_report(&data, out.path(), Theme::Dark).unwrap();
    assert!(!stale_contributor.exists());
    assert!(!stale_award.exists());
    assert!(out.path().join("contributors/616c7068614078.svg").exists());
    assert!(out.path().join("awards/commit-machine.svg").exists());
    assert_eq!(
        fs::read_to_string(out.path().join("contributors/notes.svg")).unwrap(),
        "keep"
    );
    assert_eq!(
        fs::read_to_string(out.path().join("awards/notes.svg")).unwrap(),
        "keep"
    );
    #[cfg(unix)]
    {
        assert!(
            fs::symlink_metadata(out.path().join("contributors/linked.svg"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(
            fs::symlink_metadata(out.path().join("contributors/67616d6d614078.svg"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read_to_string(out.path().join("target.txt")).unwrap(),
            "keep"
        );
    }
}

#[test]
fn forged_data_and_copied_card_do_not_delete_unrelated_svg() {
    use git_wrapped::render::{render_report, Theme};
    let data = cross_offset_data();
    let out = tempdir();
    render_report(&data, out.path(), Theme::Dark).unwrap();
    let original = fs::read_dir(out.path().join("contributors"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let unrelated = out.path().join("contributors/666f726765.svg");
    fs::copy(original, &unrelated).unwrap();
    let mut metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(out.path().join("data.json")).unwrap()).unwrap();
    metadata["contributors"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "id": "forge", "commits": 1
        }));
    fs::write(
        out.path().join("data.json"),
        serde_json::to_vec(&metadata).unwrap(),
    )
    .unwrap();
    render_report(&data, out.path(), Theme::Dark).unwrap();
    assert!(unrelated.exists());
}

#[test]
fn oversized_card_manifest_skips_stale_cleanup() {
    use git_wrapped::render::{render_report, Theme};
    let mut data = cross_offset_data();
    data.contributors[0].id = "alpha@x".into();
    let mut other = data.contributors[0].clone();
    other.id = "beta@x".into();
    data.contributors.push(other);
    let out = tempdir();
    render_report(&data, out.path(), Theme::Dark).unwrap();
    let stale = out.path().join("contributors/626574614078.svg");
    assert!(stale.exists());
    let manifest = out.path().join("card-manifest.json");
    fs::OpenOptions::new()
        .write(true)
        .open(manifest)
        .unwrap()
        .set_len(4 * 1024 * 1024 + 1)
        .unwrap();
    data.contributors.pop();
    render_report(&data, out.path(), Theme::Dark).unwrap();
    assert!(stale.exists());
}

#[test]
fn edited_generated_card_is_preserved_when_it_becomes_stale() {
    use git_wrapped::render::{render_report, Theme};
    let mut data = cross_offset_data();
    data.contributors[0].id = "alpha@x".into();
    let mut other = data.contributors[0].clone();
    other.id = "beta@x".into();
    data.contributors.push(other);
    let out = tempdir();
    render_report(&data, out.path(), Theme::Dark).unwrap();
    let stale = out.path().join("contributors/626574614078.svg");
    let mut edited = fs::read_to_string(&stale).unwrap();
    edited = edited.replace("GIT WRAPPED", "GIT WRAPPED edited");
    fs::write(&stale, &edited).unwrap();
    data.contributors.pop();
    render_report(&data, out.path(), Theme::Dark).unwrap();
    assert_eq!(fs::read_to_string(stale).unwrap(), edited);
}

#[test]
fn gallery_truncates_wide_labels_to_available_width() {
    use git_wrapped::render::{render_report, Theme};
    let mut data = cross_offset_data();
    data.contributors[0].name = "W".repeat(100);
    data.files[0].display_path = "W".repeat(200);
    let out = tempdir();
    render_report(&data, out.path(), Theme::Dark).unwrap();
    let card = fs::read_dir(out.path().join("contributors"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let card = fs::read_to_string(card).unwrap();
    let heading = card
        .split("y=\"125\"")
        .nth(1)
        .unwrap()
        .split("</text>")
        .next()
        .unwrap();
    assert!(heading.matches('W').count() <= 25, "{heading}");
    assert!(heading.contains('…'));
    let files = fs::read_to_string(out.path().join("file-churn.svg")).unwrap();
    let row = files
        .split("y=\"222\"")
        .nth(1)
        .unwrap()
        .split("</text>")
        .next()
        .unwrap();
    assert!(row.matches('W').count() <= 40, "{row}");
    assert!(row.contains("historical path") || row.contains("current"));
}

#[test]
fn report_creates_nested_output_directory() {
    let data = cross_offset_data();
    let out = tempdir();
    let nested = out.path().join("new").join("report");
    git_wrapped::render::render_report(&data, &nested, git_wrapped::render::Theme::Dark).unwrap();
    assert!(nested.join("summary.svg").is_file());
}

#[cfg(unix)]
#[test]
fn report_rejects_existing_directory_below_symlink_ancestor() {
    let data = cross_offset_data();
    let base = tempdir();
    let external = tempdir();
    fs::create_dir(external.path().join("report")).unwrap();
    std::os::unix::fs::symlink(external.path(), base.path().join("link")).unwrap();

    let output = base.path().join("link/report");
    let error =
        git_wrapped::render::render_report(&data, &output, git_wrapped::render::Theme::Dark)
            .unwrap_err();
    assert!(error.contains("symlink"), "{error}");
    assert_eq!(
        fs::read_dir(external.path().join("report"))
            .unwrap()
            .count(),
        0
    );
}
