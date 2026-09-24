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
