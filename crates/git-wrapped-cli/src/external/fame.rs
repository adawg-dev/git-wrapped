//! git-fame JSON capture, kept source-labeled and apart from canonical analytics.
use super::{check_target, failure, head_sha, run_capped, version};
use crate::{
    model::RepositoryAnalytics,
    render::{fit_text, svg, text, write_artifact, Theme},
};
use serde_json::Value;
use std::{path::Path, process::Command};

const ARGS: &[&str] = &["--silent-progress", "--loc=surviving", "--format=json"];
const ROWS: usize = 8;
const MAX_JSON: usize = 16 * 1024 * 1024;
pub const SOURCE_METRIC: &str = "git-fame surviving LOC";
const WARNING: &str = "Third-party git-fame analysis: identities are not matched to Git Wrapped contributor IDs (git-fame may apply .mailmap and aliases differently), and its values are not canonical Git Wrapped metrics.";

pub struct Fame {
    /// Column names from git-fame's JSON header, when it provides one.
    pub columns: Option<Vec<String>>,
    pub rows: Vec<(String, Vec<Value>)>,
}

pub fn parse_fame(bytes: &[u8]) -> Result<Fame, String> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|e| format!("invalid git-fame JSON: {e}"))?;
    let data = value
        .get("data")
        .and_then(Value::as_array)
        .ok_or("git-fame JSON has no data array")?;
    let mut rows = Vec::with_capacity(data.len());
    for row in data {
        let items = row.as_array().ok_or("git-fame row is not an array")?;
        let name = items
            .first()
            .and_then(Value::as_str)
            .ok_or("git-fame row has no name")?;
        rows.push((name.to_owned(), items[1..].to_vec()));
    }
    let columns = value
        .get("columns")
        .and_then(Value::as_array)
        .and_then(|columns| {
            columns
                .iter()
                .map(|c| c.as_str().map(|c| c.trim().to_owned()))
                .collect()
        });
    Ok(Fame { columns, rows })
}

/// "loc 12 · coms 3": numeric fields named by the header, whole fields only, within `max_chars`.
fn metrics(fame: &Fame, values: &[Value], max_chars: usize) -> String {
    let mut label = String::new();
    if let Some(columns) = fame
        .columns
        .as_ref()
        .filter(|c| c.len() == values.len() + 1)
    {
        for (column, value) in columns[1..].iter().zip(values) {
            if !value.is_number() {
                continue;
            }
            let field = format!(
                "{}{column} {value}",
                if label.is_empty() { "" } else { " · " }
            );
            if label.chars().count() + field.chars().count() > max_chars {
                break;
            }
            label += &field;
        }
    }
    label
}

fn comparison(
    fame: &Fame,
    version: Option<&str>,
    data: Option<&RepositoryAnalytics>,
    theme: Theme,
) -> String {
    let p = theme.palette();
    let mut body = text(64, 66, 18, p.accent, "GIT WRAPPED · EXTERNAL")
        + &text(64, 125, 42, p.fg, "git-fame comparison")
        + &text(
            64,
            163,
            17,
            p.muted,
            "Side by side only: identities are not matched and the numbers are not equivalent",
        )
        + &text(
            64,
            215,
            20,
            p.secondary,
            &format!(
                "git-fame (external){}",
                version.map(|v| format!(" {v}")).unwrap_or_default()
            ),
        )
        + &text(
            64,
            240,
            15,
            p.muted,
            "git-fame surviving LOC · first 8 rows, names as git-fame reports them",
        );
    for (i, (name, values)) in fame.rows.iter().take(ROWS).enumerate() {
        let y = 295 + 44 * i as u32;
        body += &text(64, y, 18, p.fg, &fit_text(name, 520, 18));
        body += &text(64, y + 18, 14, p.muted, &metrics(fame, values, 520 / 14));
    }
    match data.and_then(|d| d.deep.as_ref()) {
        Some(deep) => {
            body += &text(
                640,
                215,
                20,
                p.secondary,
                "Git Wrapped current HEAD ownership",
            );
            body += &text(
                640,
                240,
                15,
                p.muted,
                "IDs normalized by .mailmap then .git-wrapped.json",
            );
            let c = &deep.coverage;
            body += &text(
                640,
                262,
                13,
                p.muted,
                &format!(
                    "{}/{} regular files analyzed · {} unknown lines{}",
                    c.analyzed_files,
                    c.eligible_files,
                    c.unknown_lines,
                    if c.truncated {
                        " · partial coverage"
                    } else {
                        ""
                    }
                ),
            );
            for (i, row) in deep.ownership.iter().take(ROWS).enumerate() {
                let y = 295 + 44 * i as u32;
                body += &text(640, y, 18, p.fg, &fit_text(&row.author_id, 520, 18));
                body += &text(
                    640,
                    y + 18,
                    14,
                    p.muted,
                    &format!("{} lines · {:.1}%", row.lines, row.percent),
                );
            }
        }
        None => {
            body += &text(
                640,
                215,
                20,
                p.secondary,
                "Git Wrapped ownership not included",
            );
            body += &text(
                640,
                240,
                15,
                p.muted,
                "Re-run with --deep to show current HEAD ownership",
            );
        }
    }
    body += &text(
        64,
        660,
        15,
        p.muted,
        "Raw git-fame output: raw.json · source details: manifest.json",
    );
    svg(700, p.bg, &body)
}

/// Run git-fame on `root` and publish raw JSON, a comparison chart, then the manifest.
pub fn capture(
    executable: &Path,
    root: &Path,
    output: &Path,
    theme: Theme,
    data: Option<&RepositoryAnalytics>,
) -> Result<(), String> {
    check_target(&output.join("external/git-fame"))?;
    let version = version(executable);
    let head = head_sha(root)?;
    let mut command = Command::new(executable);
    command.args(ARGS).arg(root).current_dir(root);
    let captured = run_capped(command, MAX_JSON, None)?;
    if !captured.status.success() {
        return Err(failure("git-fame", &captured));
    }
    if captured.stdout_overflow {
        return Err("git-fame JSON exceeds 16 MB".into());
    }
    let fame = parse_fame(&captured.stdout)?;
    let chart = comparison(&fame, version.as_deref(), data, theme);
    let ownership = data.is_some_and(|d| d.deep.is_some());
    let manifest = serde_json::json!({
        "tool": "git-fame",
        "version": version,
        "head_sha": head,
        "command": ["git-fame", ARGS[0], ARGS[1], ARGS[2], "<repository>"],
        "source_metric": SOURCE_METRIC,
        "warning": WARNING,
        "git_wrapped_ownership_included": ownership,
        "files": ["raw.json", "comparison.svg"],
    });
    let mut manifest = serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?;
    manifest.push(b'\n');
    write_artifact(output, "external/git-fame/raw.json", &captured.stdout)?;
    write_artifact(output, "external/git-fame/comparison.svg", chart.as_bytes())?;
    write_artifact(output, "external/git-fame/manifest.json", &manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fame_rejects_missing_data_array() {
        assert!(parse_fame(br#"{"data":"wrong"}"#).is_err());
        assert!(parse_fame(br#"{"data":[[12]]}"#).is_err());
        assert!(parse_fame(br#"{"data":[["Ada",12],["Bob",3]]}"#).is_ok());
    }

    #[test]
    fn labels_only_use_header_named_numbers() {
        let fame =
            parse_fame(br#"{"data":[["Ada",12,"x"]],"columns":["Author","loc","distribution"]}"#)
                .unwrap();
        assert_eq!(metrics(&fame, &fame.rows[0].1, 40), "loc 12");
        let wide = parse_fame(br#"{"data":[["Ada",123456,7]],"columns":["Author","loc","coms"]}"#)
            .unwrap();
        assert_eq!(metrics(&wide, &wide.rows[0].1, 20), "loc 123456 · coms 7");
        assert_eq!(metrics(&wide, &wide.rows[0].1, 12), "loc 123456");
        let bare = parse_fame(br#"{"data":[["Ada",12]]}"#).unwrap();
        assert_eq!(metrics(&bare, &bare.rows[0].1, 40), "");
    }
}
