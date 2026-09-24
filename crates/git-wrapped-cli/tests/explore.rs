mod common;
use common::Fixture;
use std::process::{Command, Stdio};

#[test]
fn explore_requires_an_interactive_terminal() {
    let f = Fixture::new();
    f.commit("a.txt", b"one\n", "a@x", "2024-01-01T10:00:00 +0000");
    let output = Command::new(env!("CARGO_BIN_EXE_git-wrapped"))
        .current_dir(f.dir.path())
        .arg("explore")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("requires an interactive terminal"));
    assert!(!output.stdout.contains(&0x1b));
}
