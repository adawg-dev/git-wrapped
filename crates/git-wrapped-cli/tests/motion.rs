mod common;
use common::{tempdir, Fixture};
use git_wrapped::{analysis::analyze, config::Config, git::discover, render::Theme};
use image::{codecs::gif::GifDecoder, AnimationDecoder};
use std::fs;

fn frames(path: &std::path::Path) -> Vec<image::Frame> {
    let decoder = GifDecoder::new(std::io::BufReader::new(fs::File::open(path).unwrap())).unwrap();
    decoder.into_frames().collect_frames().unwrap()
}

fn two_commit_data() -> git_wrapped::model::RepositoryAnalytics {
    let f = Fixture::new();
    f.commit("a", b"a\n", "a@x", "2024-01-02T00:30:00 +1400");
    f.commit("b", b"b\n", "a@x", "2024-01-01T23:30:00 -1200");
    analyze(&discover(f.dir.path()).unwrap(), &Config::default()).unwrap()
}

#[test]
fn gif_story_has_fixed_frames_and_delays() {
    let data = two_commit_data();
    let dir = tempdir();
    let out = dir.path().join("story.gif");
    git_wrapped::motion::write_gif(&data, &out, Theme::Dark).unwrap();
    let bytes = fs::read(&out).unwrap();
    assert_eq!(&bytes[..6], b"GIF89a");
    assert!(bytes.len() < 20_000_000);
    let frames = frames(&out);
    assert_eq!(frames.len(), 49);
    for (index, frame) in frames.iter().enumerate() {
        assert_eq!(frame.buffer().dimensions(), (720, 720));
        let (numer, denom) = frame.delay().numer_denom_ms();
        let expected = if index % 7 == 6 { 1800 } else { 100 };
        assert_eq!(numer / denom, expected, "frame {index}");
    }
    let leftovers: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
    assert_eq!(leftovers.len(), 1);
}

#[test]
fn gif_story_is_complete_and_deterministic_for_one_commit() {
    let f = Fixture::new();
    f.commit("a", b"one\n", "a@x", "2024-01-01T10:00:00 +0000");
    let data = analyze(&discover(f.dir.path()).unwrap(), &Config::default()).unwrap();
    let dir = tempdir();
    let first = dir.path().join("one.gif");
    let second = dir.path().join("two.gif");
    git_wrapped::motion::write_gif(&data, &first, Theme::Light).unwrap();
    git_wrapped::motion::write_gif(&data, &second, Theme::Light).unwrap();
    assert_eq!(frames(&first).len(), 49);
    assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());
}

#[test]
fn story_scenes_escape_hostile_text() {
    let mut data = two_commit_data();
    data.repository.name = "<svg onload=x>&\"".into();
    data.contributors[0].name = "</text><script>".into();
    data.files[0].display_path = "a<b>|c\n.rs".into();
    let scenes = git_wrapped::motion::story::scenes(&data, Theme::Dark);
    assert_eq!(scenes.len(), 7);
    for scene in &scenes {
        for index in 0..7 {
            let svg = scene.at(index, 7);
            assert!(!svg.contains("<script") && !svg.contains("<svg onload"));
            assert_eq!(svg.matches("<svg").count(), 1);
            git_wrapped::render::raster::render_rgba(&svg, 720, 720).unwrap();
        }
    }
}

#[test]
fn gif_refuses_symlink_output_and_keeps_target() {
    let data = two_commit_data();
    let dir = tempdir();
    let target = dir.path().join("target.txt");
    fs::write(&target, "untouched").unwrap();
    let link = dir.path().join("story.gif");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let error = git_wrapped::motion::write_gif(&data, &link, Theme::Dark).unwrap_err();
    assert!(error.contains("symlink"), "{error}");
    assert_eq!(fs::read_to_string(&target).unwrap(), "untouched");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
}

#[test]
fn gource_log_sanitizes_fields_normalizes_authors_and_sorts() {
    use std::os::unix::ffi::OsStrExt;
    let f = Fixture::new();
    fs::write(
        f.dir.path().join(".mailmap"),
        "Real Name <real@x> <alias@x>\n",
    )
    .unwrap();
    f.commit(
        "b|pipe\nline",
        b"two\n",
        "alias@x",
        "2024-01-02T10:00:00 +0000",
    );
    f.commit("a", b"one\n", "other@x", "2024-01-01T10:00:00 +0000");
    // APFS rejects non-UTF8 names, so stage the odd path through the index only.
    let blob = f.git(&["hash-object", "-w", "a"]).stdout;
    let mut info = b"100644,".to_vec();
    info.extend_from_slice(String::from_utf8(blob).unwrap().trim().as_bytes());
    info.extend_from_slice(b",odd\xff.rs");
    assert!(std::process::Command::new("git")
        .arg("-C")
        .arg(f.dir.path())
        .args(["update-index", "--add", "--cacheinfo"])
        .arg(std::ffi::OsStr::from_bytes(&info))
        .status()
        .unwrap()
        .success());
    assert!(std::process::Command::new("git")
        .arg("-C")
        .arg(f.dir.path())
        .args(["commit", "-qm", "odd"])
        .env("GIT_AUTHOR_DATE", "2024-01-03T10:00:00 +0000")
        .env("GIT_AUTHOR_EMAIL", "other@x")
        .status()
        .unwrap()
        .success());
    let repo = discover(f.dir.path()).unwrap();
    let dir = tempdir();
    let log = dir.path().join("history.log");
    let stats = git_wrapped::motion::gource::write_custom_log(
        &repo,
        &Config::default(),
        &Default::default(),
        &log,
    )
    .unwrap();
    assert_eq!((stats.commits, stats.events), (3, 4));
    let contents = fs::read_to_string(&log).unwrap();
    let lines: Vec<&str> = contents.lines().collect();
    assert_eq!(lines.len(), 4);
    let mut previous = 0_i64;
    for line in &lines {
        let fields: Vec<&str> = line.split('|').collect();
        assert_eq!(fields.len(), 4, "{line}");
        assert_eq!(fields[2], "M");
        let timestamp: i64 = fields[0].parse().unwrap();
        assert!(timestamp >= previous);
        previous = timestamp;
    }
    assert!(contents.contains("|Real Name|M|b_pipe_line\n"));
    assert!(contents.contains("|Test|M|odd\u{fffd}.rs\n"));
    assert!(!contents.contains("alias@x"));
    let filtered = git_wrapped::motion::gource::write_custom_log(
        &repo,
        &Config::default(),
        &git_wrapped::analysis::AnalysisOptions {
            exclusions: vec!["*.rs".into()],
            since: chrono::NaiveDate::from_ymd_opt(2024, 1, 2),
            ..Default::default()
        },
        &dir.path().join("filtered.log"),
    )
    .unwrap();
    assert_eq!((filtered.commits, filtered.events), (1, 2));
}

fn cli(
    args: &[&str],
    cwd: &std::path::Path,
    path: Option<&std::ffi::OsStr>,
) -> std::process::Output {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_git-wrapped"));
    command.current_dir(cwd).args(args);
    if let Some(path) = path {
        command.env("PATH", path);
    }
    command.output().unwrap()
}

/// A PATH holding only fake tools and the real Git.
fn fake_path(tools: &[(&str, &str)]) -> (tempfile::TempDir, std::ffi::OsString) {
    use std::os::unix::fs::PermissionsExt;
    let bin = tempdir();
    for (name, script) in tools {
        let path = bin.path().join(name);
        fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let git = std::process::Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    let git = std::path::PathBuf::from(String::from_utf8(git.stdout).unwrap().trim());
    std::os::unix::fs::symlink(git, bin.path().join("git")).unwrap();
    let path = bin.path().as_os_str().to_owned();
    (bin, path)
}

fn repo_fixture() -> Fixture {
    let f = Fixture::new();
    f.commit("a|b", b"one\n", "a@x", "2024-01-01T10:00:00 +0000");
    f.commit("c", b"two\n", "b@x", "2024-01-02T10:00:00 +0000");
    f
}

fn alive(pid_file: &std::path::Path) -> bool {
    let pid = fs::read_to_string(pid_file).unwrap();
    std::process::Command::new("kill")
        .args(["-0", pid.trim()])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success()
}

const VERSION_OK: &str = r#"case "$1" in --version|-version) exit 0;; esac"#;

#[test]
fn animate_gif_works_without_external_tools() {
    let f = repo_fixture();
    let (_bin, path) = fake_path(&[]);
    let result = cli(&["animate", "--theme", "light"], f.dir.path(), Some(&path));
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let bytes = fs::read(f.dir.path().join("git-wrapped-report/story.gif")).unwrap();
    assert_eq!(&bytes[..6], b"GIF89a");
    let out = tempdir();
    let target = out.path().join("custom.gif");
    let target_arg = target.to_str().unwrap();
    let result = cli(
        &["animate", "--format", "gif", "--output", target_arg],
        f.dir.path(),
        None,
    );
    assert!(result.status.success());
    assert!(target.exists());
}

#[test]
fn animate_mp4_names_missing_tools_and_writes_nothing() {
    let f = repo_fixture();
    let out = tempdir();
    let video = out.path().join("history.mp4");
    for tools in [
        vec![],
        vec![("gource", VERSION_OK)],
        vec![("ffmpeg", VERSION_OK)],
    ] {
        let (_bin, path) = fake_path(&tools);
        let result = cli(
            &[
                "animate",
                "--format",
                "mp4",
                "--output",
                video.to_str().unwrap(),
            ],
            f.dir.path(),
            Some(&path),
        );
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(!result.status.success());
        let missing = if tools.first().is_some_and(|t| t.0 == "gource") {
            "FFmpeg is not installed"
        } else {
            "Gource is not installed"
        };
        assert!(stderr.contains(missing), "{stderr}");
        assert_eq!(fs::read_dir(out.path()).unwrap().count(), 0);
    }
}

#[test]
fn animate_mp4_pipes_sanitized_log_through_both_tools() {
    let f = repo_fixture();
    let (_bin, path) = fake_path(&[
        (
            "gource",
            &format!(
                r#"{VERSION_OK}
for a; do log=$a; done
printf '%s\n' "$@" > "${{log%/*}}/gource-args"
/bin/cat "$log""#
            ),
        ),
        (
            "ffmpeg",
            &format!(
                r#"{VERSION_OK}
for a; do out=$a; done
/bin/cat > "$out""#
            ),
        ),
    ]);
    let out = tempdir();
    let video = out.path().join("history.mp4");
    let result = cli(
        &[
            "animate",
            "--format",
            "mp4",
            "--hide-filenames",
            "--seconds-per-day",
            "2",
            "--output",
            video.to_str().unwrap(),
        ],
        f.dir.path(),
        Some(&path),
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let contents = fs::read_to_string(&video).unwrap();
    assert!(contents.contains("|Test|M|a_b\n") && contents.contains("|Test|M|c\n"));
    let args = fs::read_to_string(out.path().join("gource-args")).unwrap();
    assert!(args.contains("--log-format\ncustom\n-1280x720\n--output-ppm-stream\n-\n"));
    assert!(args.contains("--seconds-per-day\n2\n") && args.contains("--hide\nfilenames\n"));
    let names: Vec<_> = fs::read_dir(out.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
}

#[test]
fn animate_mp4_child_failure_reaps_peer_and_leaves_no_output() {
    let f = repo_fixture();
    let out = tempdir();
    let pids = tempdir();
    let pid_file = pids.path().join("gource");
    let (_bin, path) = fake_path(&[
        (
            "gource",
            &format!(
                "{VERSION_OK}\necho $$ > '{}'\nexec /bin/sleep 30",
                pid_file.display()
            ),
        ),
        ("ffmpeg", &format!("{VERSION_OK}\nexit 7")),
    ]);
    let video = out.path().join("history.mp4");
    let started = std::time::Instant::now();
    let result = cli(
        &[
            "animate",
            "--format",
            "mp4",
            "--output",
            video.to_str().unwrap(),
        ],
        f.dir.path(),
        Some(&path),
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        !result.status.success() && stderr.contains("FFmpeg exited"),
        "{stderr}"
    );
    assert!(!alive(&pid_file));
    assert_eq!(fs::read_dir(out.path()).unwrap().count(), 0);
}

#[test]
fn animate_mp4_interrupt_reaps_both_children() {
    let f = repo_fixture();
    let out = tempdir();
    let pids = tempdir();
    let block = |name: &str| {
        format!(
            "{VERSION_OK}\necho $$ > '{}'\nexec /bin/sleep 30",
            pids.path().join(name).display()
        )
    };
    let (_bin, path) = fake_path(&[("gource", &block("gource")), ("ffmpeg", &block("ffmpeg"))]);
    let video = out.path().join("history.mp4");
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_git-wrapped"))
        .current_dir(f.dir.path())
        .args([
            "animate",
            "--format",
            "mp4",
            "--output",
            video.to_str().unwrap(),
        ])
        .env("PATH", &path)
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    for _ in 0..500 {
        if pids.path().join("gource").exists() && pids.path().join("ffmpeg").exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert!(std::process::Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap()
        .success());
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(130));
    assert!(!alive(&pids.path().join("gource")) && !alive(&pids.path().join("ffmpeg")));
    assert_eq!(fs::read_dir(out.path()).unwrap().count(), 0);
}

#[test]
fn animate_mp4_refuses_symlink_output() {
    let f = repo_fixture();
    let (_bin, path) = fake_path(&[("gource", VERSION_OK), ("ffmpeg", VERSION_OK)]);
    let out = tempdir();
    let target = out.path().join("target.txt");
    fs::write(&target, "untouched").unwrap();
    let link = out.path().join("history.mp4");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let result = cli(
        &[
            "animate",
            "--format",
            "mp4",
            "--output",
            link.to_str().unwrap(),
        ],
        f.dir.path(),
        Some(&path),
    );
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("symlink"));
    assert_eq!(fs::read_to_string(&target).unwrap(), "untouched");
    assert_eq!(fs::read_dir(out.path()).unwrap().count(), 2);
}

#[test]
fn animate_mp4_rejects_invalid_seconds_per_day() {
    let f = repo_fixture();
    let result = cli(
        &["animate", "--format", "mp4", "--seconds-per-day", "0"],
        f.dir.path(),
        None,
    );
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("--seconds-per-day"));
}
