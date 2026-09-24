//! Git of Theseus cohort capture: a third-party method, stored beside canonical data.
use super::{check_target, failure, head_sha, run_capped, version};
use crate::render::write_artifact;
use std::{
    fs,
    io::{ErrorKind, Read},
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const NAMES: &[&str] = &["cohorts.json", "authors.json", "exts.json", "survival.json"];
const MAX_JSON: u64 = 16 * 1024 * 1024;
const NOTE: &str = "third-party method; not canonical Git Wrapped survival.";

/// Read each named output that exists, refusing non-regular, oversized, or invalid JSON files.
fn read_outputs(directory: &Path) -> Result<Vec<(&'static str, Vec<u8>)>, String> {
    let mut outputs = Vec::new();
    for &name in NAMES {
        let path = directory.join(name);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("inspect git-of-theseus {name}: {error}")),
            Ok(meta) if !meta.file_type().is_file() => {
                return Err(format!("git-of-theseus {name} is not a regular file"))
            }
            Ok(_) => {}
        }
        let mut bytes = Vec::new();
        fs::File::open(&path)
            .and_then(|file| file.take(MAX_JSON + 1).read_to_end(&mut bytes))
            .map_err(|e| format!("read git-of-theseus {name}: {e}"))?;
        if bytes.len() as u64 > MAX_JSON {
            return Err(format!("git-of-theseus {name} exceeds 16 MB"));
        }
        serde_json::from_slice::<serde_json::Value>(&bytes)
            .map_err(|e| format!("invalid git-of-theseus {name}: {e}"))?;
        outputs.push((name, bytes));
    }
    Ok(outputs)
}

/// Paths of the recognized, valid Git of Theseus JSON outputs in `directory`.
pub fn validated_outputs(directory: &Path) -> Result<Vec<PathBuf>, String> {
    Ok(read_outputs(directory)?
        .into_iter()
        .map(|(name, _)| directory.join(name))
        .collect())
}

/// A private scratch directory for the tool's output, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Self, String> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "git-wrapped-theseus-{}-{nanos}",
            std::process::id()
        ));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .map_err(|e| format!("create {}: {e}", path.display()))?;
        Ok(Self(path))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Run `git-of-theseus-analyze` into a scratch directory and publish its named JSON files.
pub fn capture(executable: &Path, root: &Path, output: &Path) -> Result<(), String> {
    check_target(&output.join("external/git-of-theseus"))?;
    let mut help = Command::new(executable);
    help.arg("--help");
    let help = run_capped(help, 64 * 1024, Some(Duration::from_secs(10)))?;
    if !String::from_utf8_lossy(&help.stdout).contains("--outdir") {
        return Err(
            "incompatible git-of-theseus-analyze: its --help does not offer --outdir".into(),
        );
    }
    let version = version(executable);
    let head = head_sha(root)?;
    let scratch = Scratch::new()?;
    let mut command = Command::new(executable);
    command
        .arg(root)
        .arg("--outdir")
        .arg(&scratch.0)
        .current_dir(root);
    let captured = run_capped(command, 1024 * 1024, None)?;
    if !captured.status.success() {
        return Err(failure("git-of-theseus-analyze", &captured));
    }
    let outputs = read_outputs(&scratch.0)?;
    if outputs.is_empty() {
        return Err(format!(
            "git-of-theseus-analyze produced none of {}",
            NAMES.join(", ")
        ));
    }
    let manifest = serde_json::json!({
        "tool": "git-of-theseus-analyze",
        "version": version,
        "head_sha": head,
        "command": ["git-of-theseus-analyze", "<repository>", "--outdir", "<temporary-directory>"],
        "files": outputs.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
        "note": NOTE,
    });
    let mut manifest = serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?;
    manifest.push(b'\n');
    for (name, bytes) in &outputs {
        write_artifact(output, &format!("external/git-of-theseus/{name}"), bytes)?;
    }
    write_artifact(output, "external/git-of-theseus/manifest.json", &manifest)
}
