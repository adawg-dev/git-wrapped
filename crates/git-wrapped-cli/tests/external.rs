mod common;
use common::{tempdir, Fixture};
use std::{ffi::OsStr, fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

fn real_git() -> std::path::PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|dir| dir.join("git"))
        .find(|path| path.is_file())
        .unwrap()
}

/// A PATH holding only Git plus the given fake tool scripts.
fn fake_path(tools: &[(&str, &str)]) -> tempfile::TempDir {
    let bin = tempdir();
    std::os::unix::fs::symlink(real_git(), bin.path().join("git")).unwrap();
    for (name, script) in tools {
        let path = bin.path().join(name);
        fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    bin
}

fn cli(args: &[&OsStr], cwd: &Path, path: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_git-wrapped"))
        .current_dir(cwd)
        .env("PATH", path)
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn external_list_is_read_only_and_names_capabilities() {
    let f = Fixture::new();
    f.commit("a", b"one\n", "a@x", "2024-01-01T10:00:00 +0000");
    let bin = fake_path(&[
        ("git-fame", "echo 'git-fame 3.1.1'"),
        ("gource", "echo \"ran $*\" > \"$0.log\"; exit 1"),
    ]);
    fs::write(bin.path().join("hercules"), b"not executable").unwrap();
    let result = cli(
        &["external".as_ref(), "list".as_ref()],
        f.dir.path(),
        bin.path(),
    );
    assert!(result.status.success(), "{result:?}");
    let text = String::from_utf8_lossy(&result.stdout);
    for tool in [
        "gource",
        "ffmpeg",
        "git-fame",
        "git-of-theseus-analyze",
        "hercules",
        "git-quick-stats",
    ] {
        assert!(text.contains(tool), "{text}");
    }
    assert!(text.contains("git-fame\tinstalled\t3.1.1\tJSON comparison"));
    assert!(text.contains("gource\tinstalled\tunknown\tMP4 renderer"));
    assert!(text.contains("hercules\tmissing\t-\tmanual companion"));
    // Only `--version` is ever run by discovery.
    assert_eq!(
        fs::read_to_string(bin.path().join("gource.log")).unwrap(),
        "ran --version\n"
    );
    assert!(!f.dir.path().join("git-wrapped-report").exists());
}

const FAME_JSON: &str = r#"{"total":{"loc":15},"data":[["Ada",12,1,1,"80.0/50.0/50.0"],["Bob",3,1,1,"20.0/50.0/50.0"]],"columns":["Author","loc","coms","fils"," distribution"]}"#;

fn fame_script(body: &str) -> String {
    format!("if [ \"$1\" = --version ]; then echo 'git-fame 3.1.1'; exit 0; fi\necho \"$*\" > \"$0.args\"\n{body}")
}

fn run_fame(f: &Fixture, bin: &Path, out: &Path, deep: bool) -> std::process::Output {
    let mut args: Vec<&OsStr> = vec![
        "--output".as_ref(),
        out.as_os_str(),
        "external".as_ref(),
        "run".as_ref(),
        "git-fame".as_ref(),
    ];
    if deep {
        args.push("--deep".as_ref());
    }
    cli(&args, f.dir.path(), bin)
}

fn fame_fixture() -> Fixture {
    let f = Fixture::new();
    f.commit("a", b"one\n", "a@x", "2024-01-01T10:00:00 +0000");
    f
}

#[test]
fn git_fame_capture_is_source_labeled_and_leaves_canonical_data_alone() {
    let f = fame_fixture();
    let bin = fake_path(&[(
        "git-fame",
        &fame_script(&format!("printf '%s' '{FAME_JSON}'")),
    )]);
    let out = tempdir();
    let report = cli(
        &[
            "--no-png".as_ref(),
            "--output".as_ref(),
            out.path().as_os_str(),
            "report".as_ref(),
        ],
        f.dir.path(),
        bin.path(),
    );
    assert!(report.status.success(), "{report:?}");
    let canonical = fs::read(out.path().join("data.json")).unwrap();

    let result = run_fame(&f, bin.path(), out.path(), false);
    assert!(result.status.success(), "{result:?}");
    let args = fs::read_to_string(bin.path().join("git-fame.args")).unwrap();
    assert!(args.starts_with("--silent-progress --loc=surviving --format=json "));
    assert_eq!(fs::read(out.path().join("data.json")).unwrap(), canonical);
    let dir = out.path().join("external/git-fame");
    assert_eq!(fs::read_to_string(dir.join("raw.json")).unwrap(), FAME_JSON);
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["tool"], "git-fame");
    assert_eq!(manifest["version"], "3.1.1");
    assert_eq!(manifest["source_metric"], "git-fame surviving LOC");
    assert_eq!(manifest["head_sha"].as_str().unwrap().len(), 40);
    assert!(manifest["warning"]
        .as_str()
        .unwrap()
        .contains("not matched"));
    let chart = fs::read_to_string(dir.join("comparison.svg")).unwrap();
    assert!(
        chart.contains("git-fame (external)") && chart.contains("Ada") && chart.contains("loc 12")
    );
    assert!(chart.contains("not matched") && chart.contains("--deep"));
    assert!(!chart.contains("a@x"));

    let deep = run_fame(&f, bin.path(), out.path(), true);
    assert!(deep.status.success(), "{deep:?}");
    let chart = fs::read_to_string(dir.join("comparison.svg")).unwrap();
    assert!(chart.contains("Ada") && chart.contains("a@x") && chart.contains("1 lines"));
    assert_eq!(fs::read(out.path().join("data.json")).unwrap(), canonical);
}

#[test]
fn git_fame_without_header_lists_names_only() {
    let f = fame_fixture();
    let bin = fake_path(&[(
        "git-fame",
        &fame_script(r#"printf '%s' '{"data":[["Ada",12],["Bob",3]]}'"#),
    )]);
    let out = tempdir();
    assert!(run_fame(&f, bin.path(), out.path(), false).status.success());
    let chart = fs::read_to_string(out.path().join("external/git-fame/comparison.svg")).unwrap();
    assert!(chart.contains("Ada") && !chart.contains("Ada ·"));
}

#[test]
fn git_fame_failures_publish_nothing() {
    let f = fame_fixture();
    let big = tempdir();
    let big_file = big.path().join("big.json");
    fs::write(&big_file, vec![b' '; 17 * 1024 * 1024]).unwrap();
    for (script, message) in [
        (r#"printf '%s' '{"data":"wrong"}'"#.to_owned(), "data array"),
        ("printf 'not json'".to_owned(), "git-fame JSON"),
        (format!("/bin/cat '{}'", big_file.display()), "16 MB"),
        ("echo boom >&2; exit 3".to_owned(), "boom"),
    ] {
        let bin = fake_path(&[("git-fame", &fame_script(&script))]);
        let out = tempdir();
        let result = run_fame(&f, bin.path(), out.path(), false);
        assert!(!result.status.success(), "{script}");
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(stderr.contains(message), "{stderr}");
        assert!(!out.path().join("external").exists(), "{script}");
    }
}

#[test]
fn missing_git_fame_is_actionable_and_creates_nothing() {
    let f = fame_fixture();
    let bin = fake_path(&[]);
    let parent = tempdir();
    let out = parent.path().join("report");
    let result = run_fame(&f, bin.path(), &out, false);
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("git-fame is not installed"), "{stderr}");
    assert!(!out.exists());
}

#[test]
fn git_fame_refuses_symlinked_external_directory() {
    let f = fame_fixture();
    let bin = fake_path(&[(
        "git-fame",
        &fame_script(&format!("printf '%s' '{FAME_JSON}'")),
    )]);
    let out = tempdir();
    let elsewhere = tempdir();
    std::os::unix::fs::symlink(elsewhere.path(), out.path().join("external")).unwrap();
    let result = run_fame(&f, bin.path(), out.path(), false);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("symlink"));
    assert!(!bin.path().join("git-fame.args").exists());
    assert_eq!(fs::read_dir(elsewhere.path()).unwrap().count(), 0);
}

#[test]
fn theseus_capture_publishes_only_named_json_files() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("cohorts.json"), b"{}").unwrap();
    fs::write(source.join("escape.txt"), b"bad").unwrap();
    let files = git_wrapped::external::theseus::validated_outputs(&source).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].file_name().unwrap(), "cohorts.json");

    fs::write(source.join("authors.json"), b"not json").unwrap();
    assert!(git_wrapped::external::theseus::validated_outputs(&source).is_err());
    fs::remove_file(source.join("authors.json")).unwrap();
    std::os::unix::fs::symlink(source.join("escape.txt"), source.join("exts.json")).unwrap();
    assert!(git_wrapped::external::theseus::validated_outputs(&source)
        .unwrap_err()
        .contains("regular file"));
}

fn theseus_script(body: &str) -> String {
    format!(
        "case \"$1\" in --version) echo 'git-of-theseus 0.3.4'; exit 0;; --help) echo 'usage: [--outdir OUTDIR]'; exit 0;; esac\n\
         echo \"$*\" > \"$0.args\"\n{body}"
    )
}

fn run_theseus(f: &Fixture, bin: &Path, out: &Path) -> std::process::Output {
    cli(
        &[
            "--output".as_ref(),
            out.as_os_str(),
            "external".as_ref(),
            "run".as_ref(),
            "git-of-theseus".as_ref(),
        ],
        f.dir.path(),
        bin,
    )
}

#[test]
fn theseus_run_copies_validated_cohorts_with_manifest() {
    let f = fame_fixture();
    let bin = fake_path(&[(
        "git-of-theseus-analyze",
        &theseus_script(
            "printf '{\"y\":[[1]]}' > \"$3/cohorts.json\"; printf '{}' > \"$3/authors.json\"; printf x > \"$3/junk.txt\"",
        ),
    )]);
    let out = tempdir();
    let result = run_theseus(&f, bin.path(), out.path());
    assert!(result.status.success(), "{result:?}");
    let args = fs::read_to_string(bin.path().join("git-of-theseus-analyze.args")).unwrap();
    assert!(args.contains(" --outdir "), "{args}");
    let dir = out.path().join("external/git-of-theseus");
    let mut names: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["authors.json", "cohorts.json", "manifest.json"]);
    assert_eq!(
        fs::read(dir.join("cohorts.json")).unwrap(),
        br#"{"y":[[1]]}"#
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["tool"], "git-of-theseus-analyze");
    assert_eq!(manifest["version"], "0.3.4");
    assert_eq!(
        manifest["files"],
        serde_json::json!(["cohorts.json", "authors.json"])
    );
    assert_eq!(manifest["head_sha"].as_str().unwrap().len(), 40);
    assert!(manifest["note"]
        .as_str()
        .unwrap()
        .contains("not canonical Git Wrapped survival"));
    // The tool's temporary output directory is removed.
    let outdir = args.split_whitespace().last().unwrap();
    assert!(!Path::new(outdir).exists());
}

#[test]
fn theseus_failures_publish_nothing() {
    let f = fame_fixture();
    for (script, message) in [
        (
            theseus_script("printf '{}' > \"$3/cohorts.json\"; echo broke >&2; exit 2"),
            "broke",
        ),
        (theseus_script("true"), "produced none"),
        (
            "case \"$1\" in --help) echo 'usage: [--out DIR]'; exit 0;; esac; echo ran > \"$0.args\""
                .to_owned(),
            "incompatible",
        ),
    ] {
        let bin = fake_path(&[("git-of-theseus-analyze", &script)]);
        let out = tempdir();
        let result = run_theseus(&f, bin.path(), out.path());
        assert!(!result.status.success(), "{script}");
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(stderr.contains(message), "{stderr}");
        assert!(!out.path().join("external").exists(), "{script}");
        if message == "incompatible" {
            assert!(!bin.path().join("git-of-theseus-analyze.args").exists());
        }
    }
}

#[test]
fn theseus_refuses_symlinked_output_before_running() {
    let f = fame_fixture();
    let bin = fake_path(&[(
        "git-of-theseus-analyze",
        &theseus_script("printf '{}' > \"$3/cohorts.json\""),
    )]);
    let out = tempdir();
    let elsewhere = tempdir();
    fs::create_dir(out.path().join("external")).unwrap();
    std::os::unix::fs::symlink(elsewhere.path(), out.path().join("external/git-of-theseus"))
        .unwrap();
    let result = run_theseus(&f, bin.path(), out.path());
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("symlink"));
    assert!(!bin.path().join("git-of-theseus-analyze.args").exists());
    assert_eq!(fs::read_dir(elsewhere.path()).unwrap().count(), 0);
}
