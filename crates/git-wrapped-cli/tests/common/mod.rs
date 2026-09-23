use std::{
    fs,
    process::{Command, Output},
};

pub struct Fixture {
    pub dir: tempfile::TempDir,
}

impl Fixture {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let f = Self { dir };
        assert!(f.git(&["init", "-q", "-b", "master"]).status.success());
        assert!(f.git(&["config", "user.name", "Test"]).status.success());
        assert!(f
            .git(&["config", "user.email", "test@example.com"])
            .status
            .success());
        f
    }

    pub fn git(&self, args: &[&str]) -> Output {
        Command::new("git")
            .arg("-C")
            .arg(self.dir.path())
            .args(args)
            .output()
            .unwrap()
    }

    pub fn commit(&self, path: &str, content: &[u8], author_email: &str, date: &str) {
        let path = self.dir.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
        assert!(self.git(&["add", "--all"]).status.success());
        let status = Command::new("git")
            .arg("-C")
            .arg(self.dir.path())
            .args(["commit", "-qm", "fixture"])
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", author_email)
            .status()
            .unwrap();
        assert!(status.success());
    }
}
