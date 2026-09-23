use crate::{
    analysis::AnalysisOptions, config::Config, git::Repository, model::RepositoryAnalytics,
};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, ErrorKind, Read, Write},
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

pub const SCHEMA_VERSION: u32 = 2;
const MAX_BYTES: u64 = 64 * 1024 * 1024;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CacheKey {
    pub schema_version: u32,
    pub canonical_root: String,
    pub head_sha: String,
    pub options_json: String,
    pub config_json: String,
    pub deep: bool,
}

#[derive(Deserialize)]
struct CacheEnvelope {
    key: CacheKey,
    data: RepositoryAnalytics,
}

#[derive(Serialize)]
struct CacheWrite<'a> {
    key: &'a CacheKey,
    data: &'a RepositoryAnalytics,
}

fn checked_parent(path: &Path) -> Result<(), String> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        checked_parent(parent)?;
    }
    let meta =
        fs::symlink_metadata(path).map_err(|e| format!("inspect {}: {e}", path.display()))?;
    if meta.file_type().is_symlink() {
        return Err(format!(
            "refusing symlink cache directory: {}",
            path.display()
        ));
    }
    if !meta.is_dir() {
        return Err(format!(
            "cache parent is not a directory: {}",
            path.display()
        ));
    }
    Ok(())
}

fn checked_target(path: &Path) -> Result<(), String> {
    checked_parent(path.parent().ok_or("cache path has no parent")?)?;
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            Err(format!("refusing symlink cache: {}", path.display()))
        }
        Ok(meta) if !meta.is_file() => {
            Err(format!("cache target is not a file: {}", path.display()))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("inspect {}: {error}", path.display())),
    }
}

pub fn load(path: &Path, expected_key: &CacheKey) -> Result<Option<RepositoryAnalytics>, String> {
    if expected_key.schema_version != SCHEMA_VERSION || checked_target(path).is_err() {
        return Ok(None);
    }
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("inspect {}: {error}", path.display())),
    };
    if !meta.is_file() || meta.len() > MAX_BYTES {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|e| format!("open {}: {e}", path.display()))?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("read {}: {e}", path.display()))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Ok(None);
    }
    let Ok(envelope) = serde_json::from_slice::<CacheEnvelope>(&bytes) else {
        return Ok(None);
    };
    Ok((envelope.key == *expected_key).then_some(envelope.data))
}

pub fn save(path: &Path, key: &CacheKey, data: &RepositoryAnalytics) -> Result<(), String> {
    if key.schema_version != SCHEMA_VERSION {
        return Err("unsupported cache schema".into());
    }
    checked_target(path)?;
    let bytes = serde_json::to_vec(&CacheWrite { key, data })
        .map_err(|e| format!("serialize cache: {e}"))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("cache exceeds 64 MiB".into());
    }
    let parent = path.parent().ok_or("cache path has no parent")?;
    let temporary = parent.join(format!(
        ".git-wrapped-cache.{}.{}.tmp",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|e| format!("create {}: {e}", temporary.display()))?;
        file.write_all(&bytes)
            .map_err(|e| format!("write {}: {e}", temporary.display()))?;
        file.sync_all()
            .map_err(|e| format!("sync {}: {e}", temporary.display()))?;
        checked_target(path)?;
        fs::rename(&temporary, path).map_err(|e| format!("rename {}: {e}", temporary.display()))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub fn key(
    repo: &Repository,
    config: &Config,
    options: &AnalysisOptions,
    deep: bool,
) -> Result<CacheKey, String> {
    let root = fs::canonicalize(&repo.root).map_err(|e| format!("canonicalize repository: {e}"))?;
    let canonical_root = root
        .to_str()
        .map(str::to_owned)
        .unwrap_or_else(|| hex(root.as_os_str().as_bytes()));
    let head = Command::new("git")
        .arg("-C")
        .arg(&repo.root)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .map_err(|e| format!("read HEAD: {e}"))?;
    if !head.status.success() {
        return Err(format!(
            "read HEAD: {}",
            String::from_utf8_lossy(&head.stderr).trim()
        ));
    }
    let head_sha = String::from_utf8(head.stdout)
        .map_err(|e| format!("read HEAD: {e}"))?
        .trim()
        .to_owned();
    let shallow_path = git_bytes(&root, &["rev-parse", "--git-path", "shallow"])?;
    let shallow_path = PathBuf::from(OsString::from_vec(
        shallow_path
            .strip_suffix(b"\n")
            .unwrap_or(&shallow_path)
            .to_vec(),
    ));
    let shallow_boundary = file_bytes_if_present(&if shallow_path.is_absolute() {
        shallow_path
    } else {
        root.join(shallow_path)
    })?;
    // Git's mailmap and tag refs can change analytics even when HEAD does not.
    let tags = git_bytes(
        &root,
        &[
            "for-each-ref",
            "--sort=refname",
            "--format=%(refname)%00%(objectname)",
            "refs/tags",
        ],
    )?;
    let mailmap = file_bytes_if_present(&root.join(".mailmap"))?;
    let configured_mailmap = git_config_value(&root, "mailmap.file", true)?;
    let configured_mailmap_bytes = configured_mailmap
        .as_ref()
        .map(|path| {
            let path = PathBuf::from(OsString::from_vec(path.clone()));
            file_bytes_if_present(&if path.is_absolute() {
                path
            } else {
                root.join(path)
            })
        })
        .transpose()?;
    let configured_blob = git_config_value(&root, "mailmap.blob", false)?;
    let resolved_blob = configured_blob
        .as_ref()
        .map(|reference| {
            let mut reference = reference.clone();
            reference.extend_from_slice(b"^{blob}");
            let reference = OsString::from_vec(reference);
            let output = Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["rev-parse", "--verify"])
                .arg(reference)
                .output()
                .map_err(|e| format!("read mailmap blob: {e}"))?;
            if !output.status.success() {
                return Err("resolve mailmap blob failed".to_owned());
            }
            Ok(hex(&output.stdout))
        })
        .transpose()?;
    let config_json = serde_json::to_string(&serde_json::json!({
        "settings": config.cache_key_json()?,
        "mailmap": mailmap.as_deref().map(hex),
        "mailmap_file": configured_mailmap_bytes.as_ref().and_then(|bytes| bytes.as_deref()).map(hex),
        "mailmap_file_path": configured_mailmap.as_deref().map(hex),
        "mailmap_blob": resolved_blob,
        "tags": hex(&tags),
        "shallow": repo.shallow,
        "shallow_boundary": shallow_boundary.as_deref().map(hex),
    }))
    .map_err(|e| format!("serialize cache key: {e}"))?;
    Ok(CacheKey {
        schema_version: SCHEMA_VERSION,
        canonical_root,
        head_sha,
        options_json: options.cache_key_json()?,
        config_json,
        deep,
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn file_bytes_if_present(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("read {}: {error}", path.display())),
    }
}

fn git_bytes(root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| format!("read repository inputs: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "read repository inputs: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

fn git_config_value(root: &Path, name: &str, path: bool) -> Result<Option<Vec<u8>>, String> {
    let mut args = vec!["config"];
    if path {
        args.push("--path");
    }
    args.extend(["--get", name]);
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| format!("read {name}: {e}"))?;
    if output.status.code() == Some(1) {
        return Ok(None);
    }
    if !output.status.success() {
        return Err(format!(
            "read {name}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(Some(
        output
            .stdout
            .strip_suffix(b"\n")
            .unwrap_or(&output.stdout)
            .to_vec(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(head_sha: &str) -> CacheKey {
        CacheKey {
            schema_version: SCHEMA_VERSION,
            canonical_root: "/tmp/example".into(),
            head_sha: head_sha.into(),
            options_json: "{}".into(),
            config_json: "{}".into(),
            deep: false,
        }
    }

    #[test]
    fn corrupt_oversized_and_wrong_schema_are_misses() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");
        fs::write(&path, b"not json").unwrap();
        assert!(load(&path, &key("one")).unwrap().is_none());
        fs::write(&path, vec![b'x'; MAX_BYTES as usize + 1]).unwrap();
        assert!(load(&path, &key("one")).unwrap().is_none());
        let mut old = key("one");
        old.schema_version = 1;
        assert!(load(&path, &old).unwrap().is_none());
    }
}
