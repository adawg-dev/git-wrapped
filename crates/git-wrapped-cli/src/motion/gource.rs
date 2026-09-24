use crate::{
    analysis::{select_commit, AnalysisOptions, Selection},
    config::Config,
    git::{scan, Repository},
};
use std::{
    fs,
    io::{BufWriter, Write},
    path::Path,
};

/// Gource input is capped so a huge history cannot exhaust memory or disk.
pub const MAX_EVENTS: usize = 200_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LogStats {
    pub events: usize,
    pub commits: usize,
}

struct Event {
    timestamp: i64,
    sha: String,
    path: Vec<u8>,
    author: String,
}

/// Custom-log fields cannot contain the `|` delimiter, line breaks, or other controls.
fn gource_field(value: &str) -> String {
    value
        .chars()
        .map(|c| if c == '|' || c.is_control() { '_' } else { c })
        .collect()
}

/// Write Gource's `timestamp|username|M|path` log from the canonical scanner, applying
/// the same author, date, merge, and path selection as the report. Names are normalized
/// display names and paths are lossy, sanitized display paths.
pub fn write_custom_log(
    repo: &Repository,
    config: &Config,
    filters: &AnalysisOptions,
    path: &Path,
) -> Result<LogStats, String> {
    let selection = Selection::new(repo, config, filters)?;
    let mut events = Vec::new();
    let mut commits = 0;
    scan(repo, |raw| {
        let Some((_, name, time)) = select_commit(&raw, config, filters, &selection.authors)?
        else {
            return Ok(());
        };
        let before = events.len();
        for change in &raw.changes {
            if selection
                .excluded
                .is_match(String::from_utf8_lossy(&change.path).as_ref())
            {
                continue;
            }
            if events.len() == MAX_EVENTS {
                return Err(format!(
                    "history has more than {MAX_EVENTS} file changes; narrow it with --since, --until, --author, or --exclude"
                ));
            }
            events.push(Event {
                timestamp: time.timestamp(),
                sha: raw.sha.clone(),
                path: change.path.clone(),
                author: name.clone(),
            });
        }
        if events.len() > before {
            commits += 1;
        }
        Ok(())
    })?;
    events.sort_by(|a, b| (a.timestamp, &a.sha, &a.path).cmp(&(b.timestamp, &b.sha, &b.path)));
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("create {}: {e}", path.display()))?;
    let mut writer = BufWriter::new(file);
    for event in &events {
        writeln!(
            writer,
            "{}|{}|M|{}",
            event.timestamp,
            gource_field(&event.author),
            gource_field(&String::from_utf8_lossy(&event.path))
        )
        .map_err(|e| format!("write {}: {e}", path.display()))?;
    }
    writer
        .into_inner()
        .map_err(|e| format!("write {}: {e}", path.display()))?
        .sync_all()
        .map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(LogStats {
        events: events.len(),
        commits,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_log_cannot_escape_pipe_fields() {
        assert_eq!(gource_field("A|B\n\u{1b}"), "A_B__");
        assert_eq!(gource_field("a\r\0\u{85}b"), "a___b");
    }
}
