mod common;
use common::Fixture;
use git_wrapped::git::{discover, scan};

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
    assert_eq!(repo.root, f.dir.path());
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
    let dir = tempfile::tempdir_in("/private/tmp").unwrap();
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
    assert_eq!(discover(&path).unwrap().root, path);
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
    let dir = tempfile::tempdir_in("/private/tmp").unwrap();
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

#[cfg(unix)]
#[test]
fn scan_preserves_non_utf8_path_bytes() {
    use std::os::unix::ffi::OsStrExt;
    let f = Fixture::new();
    let name = std::ffi::OsStr::from_bytes(b"raw-\xff.txt");
    if let Err(error) = std::fs::write(f.dir.path().join(name), b"text\n") {
        assert_eq!(error.raw_os_error(), Some(92)); // macOS filesystem rejects invalid UTF-8 names.
        return;
    }
    assert!(f.git(&["add", "--all"]).status.success());
    assert!(f.git(&["commit", "-qm", "raw path"]).status.success());
    let mut paths = Vec::new();
    scan(&discover(f.dir.path()).unwrap(), |c| {
        paths.extend(c.changes.into_iter().map(|change| change.path));
        Ok(())
    })
    .unwrap();
    assert_eq!(paths, vec![b"raw-\xff.txt".to_vec()]);
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
