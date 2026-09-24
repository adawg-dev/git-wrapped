use crate::model::{Activity, OwnershipSlice, RepositoryAnalytics};
pub mod raster;
use chrono::{DateTime, Datelike, NaiveDate, Timelike};
use std::{
    collections::BTreeMap,
    fs,
    io::{ErrorKind, Read},
    path::{Component, Path},
};

#[derive(Clone, Copy, Debug)]
pub enum Theme {
    Dark,
    Light,
}

#[derive(Clone, Copy)]
pub(crate) struct Palette {
    pub(crate) bg: &'static str,
    pub(crate) fg: &'static str,
    pub(crate) muted: &'static str,
    pub(crate) accent: &'static str,
    pub(crate) secondary: &'static str,
    pub(crate) positive: &'static str,
    pub(crate) negative: &'static str,
}

impl Theme {
    pub(crate) fn palette(self) -> Palette {
        match self {
            Theme::Dark => Palette {
                bg: "#10131f",
                fg: "#f4f5fb",
                muted: "#a7aec6",
                accent: "#8f7cff",
                secondary: "#55c9d2",
                positive: "#72d9ad",
                negative: "#ee9a9a",
            },
            Theme::Light => Palette {
                bg: "#f7f7fc",
                fg: "#172033",
                muted: "#535e76",
                accent: "#5b43c9",
                secondary: "#087b83",
                positive: "#18734d",
                negative: "#a33f48",
            },
        }
    }
}

fn escape_xml(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '&' => "&amp;".into(),
            '<' => "&lt;".into(),
            '>' => "&gt;".into(),
            '"' => "&quot;".into(),
            '\'' => "&apos;".into(),
            c if c.is_control() || c == '\u{fffe}' || c == '\u{ffff}' => " ".into(),
            c => c.to_string(),
        })
        .collect()
}

pub(crate) fn text(x: u32, y: u32, size: u32, color: &str, value: &str) -> String {
    format!("<text x=\"{x}\" y=\"{y}\" font-family=\"Lato,system-ui,sans-serif\" font-size=\"{size}\" fill=\"{color}\">{}</text>", escape_xml(value))
}

fn short(value: &str, limit: usize) -> String {
    let mut s: String = value.chars().take(limit).collect();
    if value.chars().count() > limit {
        s.push('…');
    }
    s
}

// One em per Unicode scalar is conservative for system-ui, including wide capitals.
fn fit_text(value: &str, width: u32, size: u32) -> String {
    let max_chars = (width / size).max(1) as usize;
    if value.chars().count() <= max_chars {
        value.to_owned()
    } else {
        short(value, max_chars - 1)
    }
}

fn contributor_card_names(mut people: Vec<(String, u64)>) -> BTreeMap<String, String> {
    people.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    people.truncate(20);
    people.sort_by(|a, b| a.0.cmp(&b.0));
    let mut collisions = BTreeMap::<String, usize>::new();
    people
        .into_iter()
        .map(|(id, _)| {
            let prefix = id
                .as_bytes()
                .iter()
                .take(8)
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            let next = collisions.entry(prefix.clone()).or_default();
            *next += 1;
            let name = if *next == 1 {
                prefix
            } else {
                format!("{prefix}-{next}")
            };
            (id, name)
        })
        .collect()
}

fn valid_award_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

const MAX_CARD_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;

fn valid_card_name(name: &str) -> bool {
    if matches!(
        name,
        "ship-of-theseus.svg"
            | "ownership.svg"
            | "ownership-over-time.svg"
            | "contributor-interactions.svg"
            | "file-coupling.svg"
    ) {
        return true;
    }
    let Some((directory, file)) = name.split_once('/') else {
        return false;
    };
    let Some(stem) = file.strip_suffix(".svg") else {
        return false;
    };
    match directory {
        "awards" => valid_award_slug(stem),
        "contributors" => {
            let (hex, suffix) = stem.split_once('-').unwrap_or((stem, ""));
            (1..=16).contains(&hex.len())
                && hex
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                && !stem.ends_with('-')
                && (suffix.is_empty()
                    || suffix
                        .parse::<usize>()
                        .is_ok_and(|n| n >= 2 && n.to_string() == suffix))
        }
        _ => false,
    }
}

fn previous_cards(output: &Path) -> BTreeMap<String, String> {
    let path = output.join("card-manifest.json");
    if !fs::symlink_metadata(&path)
        .is_ok_and(|meta| meta.is_file() && meta.len() <= MAX_CARD_MANIFEST_BYTES)
    {
        return BTreeMap::new();
    }
    let Ok(file) = fs::File::open(path) else {
        return BTreeMap::new();
    };
    let mut bytes = Vec::new();
    if file
        .take(MAX_CARD_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() as u64 > MAX_CARD_MANIFEST_BYTES
    {
        return BTreeMap::new();
    }
    let Ok(cards) = serde_json::from_slice::<BTreeMap<String, String>>(&bytes) else {
        return BTreeMap::new();
    };
    cards
        .into_iter()
        .filter(|(name, _)| valid_card_name(name))
        .collect()
}

fn remove_stale_card(output: &Path, name: &str, expected: &str) -> Result<(), String> {
    let path = output.join(name);
    if let Some(parent) = path.parent() {
        checked_directory(parent)?;
    }
    let Ok(meta) = fs::symlink_metadata(&path) else {
        return Ok(());
    };
    if !meta.is_file() || meta.len() != expected.len() as u64 {
        return Ok(());
    }
    let Ok(contents) = fs::read_to_string(&path) else {
        return Ok(());
    };
    if contents == expected {
        fs::remove_file(&path).map_err(|e| format!("remove {}: {e}", path.display()))?;
    }
    Ok(())
}

fn svg(height: u32, background: &str, body: &str) -> String {
    format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1200\" height=\"{height}\" viewBox=\"0 0 1200 {height}\"><rect width=\"100%\" height=\"100%\" fill=\"{background}\"/>{body}</svg>\n")
}

pub(crate) fn rect(x: f64, y: f64, w: f64, h: f64, color: &str) -> String {
    format!("<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{h:.2}\" rx=\"3\" fill=\"{color}\"/>")
}

fn top_ownership(parts: &[OwnershipSlice], total: u64) -> Vec<(String, u64, f64)> {
    let mut ranked: Vec<_> = parts
        .iter()
        .filter(|part| part.author_id != "unknown")
        .collect();
    ranked.sort_by(|a, b| {
        b.lines
            .cmp(&a.lines)
            .then_with(|| a.author_id.cmp(&b.author_id))
    });
    let other: u64 = ranked.iter().skip(8).map(|part| part.lines).sum();
    let mut shown: Vec<_> = ranked
        .into_iter()
        .take(8)
        .map(|part| (part.author_id.clone(), part.lines))
        .collect();
    shown.sort_by(|a, b| a.0.cmp(&b.0));
    if other > 0 {
        shown.push(("Other".into(), other));
    }
    if let Some(unknown) = parts.iter().find(|part| part.author_id == "unknown") {
        shown.push(("unknown".into(), unknown.lines));
    }
    shown
        .into_iter()
        .map(|(id, lines)| {
            let percent = if total == 0 {
                0.0
            } else {
                100.0 * lines as f64 / total as f64
            };
            (id, lines, percent)
        })
        .collect()
}

// Preserve calendar gaps so bars and sparkline positions represent elapsed time.
pub(crate) fn monthly_counts(data: &RepositoryAnalytics) -> Vec<(String, u64)> {
    let counts: std::collections::BTreeMap<_, _> = data
        .activity
        .iter()
        .filter_map(|a| {
            NaiveDate::parse_from_str(&format!("{}-01", a.month), "%Y-%m-%d")
                .ok()
                .map(|d| (d.year() * 12 + d.month0() as i32, a.commits))
        })
        .collect();
    let (Some(first), Some(last)) = (counts.keys().next(), counts.keys().next_back()) else {
        return vec![];
    };
    (*first..=*last)
        .map(|m| {
            (
                format!("{:04}-{:02}", m.div_euclid(12), m.rem_euclid(12) + 1),
                counts.get(&m).copied().unwrap_or(0),
            )
        })
        .collect()
}

pub(crate) fn checked_directory(path: &Path) -> Result<(), String> {
    if let Some(parent) = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty() && *p != path)
    {
        checked_directory(parent)?;
    }
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(format!(
            "refusing symlink output directory: {}",
            path.display()
        )),
        Ok(meta) if !meta.is_dir() => Err(format!("output is not a directory: {}", path.display())),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            fs::create_dir(path).map_err(|e| format!("create {}: {e}", path.display()))
        }
        Err(error) => Err(format!("inspect {}: {error}", path.display())),
    }
}

pub(crate) fn write_artifact(
    output: &Path,
    relative_name: &str,
    bytes: &[u8],
) -> Result<(), String> {
    if !Path::new(relative_name)
        .components()
        .all(|c| matches!(c, Component::Normal(_)))
    {
        return Err(format!("invalid artifact name: {relative_name}"));
    }
    checked_directory(output)?;
    let path = output.join(relative_name);
    if let Some(parent) = path.parent() {
        let mut current = output.to_path_buf();
        for part in parent
            .strip_prefix(output)
            .map_err(|e| e.to_string())?
            .components()
        {
            current.push(part);
            checked_directory(&current)?;
        }
    }
    match fs::symlink_metadata(&path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(format!("refusing symlink output: {}", path.display()))
        }
        Ok(meta) if !meta.is_file() => {
            return Err(format!("output is not a file: {}", path.display()))
        }
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(format!("inspect {}: {error}", path.display())),
    }
    fs::write(&path, bytes).map_err(|e| format!("write {}: {e}", path.display()))
}

fn poster(data: &RepositoryAnalytics, p: Palette) -> String {
    let r = &data.repository;
    let selection = r.selection_labels();
    let selected = !selection.is_empty();
    let mut body = text(64, 70, 20, p.accent, "GIT WRAPPED");
    body += &text(64, 138, 48, p.fg, &fit_text(&r.name, 1072, 48));
    body += &text(
        64,
        182,
        18,
        p.muted,
        &format!(
            "{} — {} · {} days{}",
            r.first_commit.split('T').next().unwrap_or(""),
            r.latest_commit.split('T').next().unwrap_or(""),
            r.age_days,
            if r.shallow {
                " · Available shallow history"
            } else {
                ""
            }
        ),
    );
    for (i, label) in selection.iter().enumerate() {
        body += &text(
            64,
            207 + i as u32 * 20,
            16,
            p.muted,
            &fit_text(label, 1072, 16),
        );
    }
    body += &text(
        64,
        if selected { 295 } else { 244 },
        18,
        p.muted,
        "GROWTH  ·  PEOPLE  ·  RHYTHM  ·  AWARDS",
    );

    for (i, (value, label)) in [
        (
            r.total_commits.to_string(),
            if selected {
                "Selected commits"
            } else {
                "Commits"
            },
        ),
        (
            r.total_contributors.to_string(),
            if selected {
                "Selected contributors"
            } else {
                "Contributors"
            },
        ),
        (r.tracked_files.to_string(), "Tracked files at HEAD"),
        (
            r.age_days.to_string(),
            if selected {
                "Selected span (days)"
            } else {
                "Days of history"
            },
        ),
        (
            format!("+{}", r.additions),
            if selected {
                "Selected additions"
            } else {
                "Lifetime additions"
            },
        ),
        (
            format!("−{}", r.deletions),
            if selected {
                "Selected deletions"
            } else {
                "Lifetime deletions"
            },
        ),
    ]
    .iter()
    .enumerate()
    {
        let x = 64 + (i % 3) as u32 * 365;
        let y = if selected { 355 } else { 322 } + (i / 3) as u32 * 103;
        body += &text(x, y, 34, p.fg, value);
        body += &text(x, y + 28, 17, p.muted, label);
    }

    body += &text(64, 555, 20, p.accent, "GROWTH");
    body += &text(
        64,
        585,
        17,
        p.muted,
        "Historical line changes by month · 12 calendar buckets max",
    );
    let months = monthly_counts(data);
    if months.is_empty() {
        body += &text(64, 695, 20, p.muted, "No monthly activity to chart");
    } else {
        let changes: std::collections::BTreeMap<_, _> = data
            .activity
            .iter()
            .map(|a| (a.month.as_str(), (a.additions, a.deletions)))
            .collect();
        let count = months.len().min(12);
        let mut buckets = vec![(0_u64, 0_u64); count];
        let mut labels = vec![(String::new(), String::new()); count];
        for (i, (month, _)) in months.iter().enumerate() {
            let (adds, deletes) = changes.get(month.as_str()).copied().unwrap_or((0, 0));
            let index = i * count / months.len();
            let bucket = &mut buckets[index];
            bucket.0 += adds;
            bucket.1 += deletes;
            if labels[index].0.is_empty() {
                labels[index].0 = month.clone();
            }
            labels[index].1 = month.clone();
        }
        let max = buckets
            .iter()
            .map(|(a, d)| a.saturating_add(*d))
            .max()
            .unwrap_or(0);
        if max == 0 {
            body += &text(64, 695, 20, p.muted, "No line changes to chart");
        } else {
            let step = 1048.0 / count as f64;
            for (i, (adds, deletes)) in buckets.iter().enumerate() {
                let x = 72.0 + i as f64 * step;
                let add_h = *adds as f64 / max as f64 * 126.0;
                let del_h = *deletes as f64 / max as f64 * 126.0;
                let month = if labels[i].0 == labels[i].1 {
                    labels[i].0.clone()
                } else {
                    format!("{}–{}", labels[i].0, labels[i].1)
                };
                body += &format!(
                    "<g><title>{}: +{} additions, −{} deletions</title>",
                    escape_xml(&month),
                    adds,
                    deletes
                );
                body += &rect(x, 738.0 - add_h, step * 0.64, add_h, p.positive);
                body += &rect(x, 738.0 - add_h - del_h, step * 0.64, del_h, p.negative);
                body += "</g>";
            }
            body += &text(
                64,
                773,
                16,
                p.muted,
                &format!(
                    "{} → {} · lines changed, stacked: additions",
                    months.first().unwrap().0,
                    months.last().unwrap().0
                ),
            );
            body += &rect(785.0, 758.0, 16.0, 16.0, p.positive);
            body += &text(809, 773, 16, p.muted, "added");
            body += &rect(932.0, 758.0, 16.0, 16.0, p.negative);
            body += &text(956, 773, 16, p.muted, "deleted");
        }
    }

    body += &text(64, 832, 20, p.accent, "PEOPLE");
    body += &text(
        64,
        863,
        17,
        p.muted,
        "Share of commits · author identities after mailmap",
    );
    let total = r.total_commits.max(1) as f64;
    let mut x = 64.0;
    for (i, c) in data.contributors.iter().take(4).enumerate() {
        let width = c.commits as f64 / total * 1072.0;
        let color = [p.accent, p.secondary, p.positive, p.negative][i];
        body += &rect(x, 889.0, width, 28.0, color);
        x += width;
    }
    if x < 1136.0 {
        body += &rect(x, 889.0, 1136.0 - x, 28.0, p.muted);
    }
    if data.contributors.is_empty() {
        body += &text(64, 962, 18, p.muted, "No contributors to chart");
    } else {
        for (i, c) in data.contributors.iter().take(4).enumerate() {
            let x = 64 + (i % 2) as u32 * 555;
            let y = 961 + (i / 2) as u32 * 38;
            body += &rect(
                x as f64,
                (y - 16) as f64,
                16.0,
                16.0,
                [p.accent, p.secondary, p.positive, p.negative][i],
            );
            body += &text(
                x + 28,
                y,
                18,
                p.fg,
                &format!(
                    "{}  ·  {} commits ({:.1}%)",
                    short(&c.name, 25),
                    c.commits,
                    c.commit_percent
                ),
            );
        }
        if data.contributors.len() > 4 {
            body += &text(
                64,
                1048,
                17,
                p.muted,
                &format!(
                    "Top 4 of {} shown; muted segment = everyone else",
                    data.contributors.len()
                ),
            );
        }
    }

    body += &text(64, 1101, 20, p.accent, "RHYTHM");
    body += &text(
        64,
        1131,
        17,
        p.muted,
        "Commits by author local hour · 00:00–23:00",
    );
    let mut hours = [0_u64; 24];
    for c in &data.contributors {
        for (hour, value) in c.commits_by_hour.iter().enumerate() {
            hours[hour] += value;
        }
    }
    let max = hours.iter().copied().max().unwrap_or(0);
    if max == 0 {
        body += &text(64, 1230, 20, p.muted, "No hourly activity to chart");
    } else {
        for (i, value) in hours.iter().enumerate() {
            let x = 68.0 + i as f64 * 44.0;
            let h = *value as f64 / max as f64 * 100.0;
            body += &rect(x, 1262.0 - h, 28.0, h, p.secondary);
        }
        for hour in [0, 6, 12, 18, 23] {
            body += &text(68 + hour * 44, 1292, 16, p.muted, &format!("{hour:02}"));
        }
    }

    body += &text(64, 1365, 20, p.accent, "AWARDS");
    if data.awards.is_empty() {
        body += &text(64, 1425, 20, p.muted, "No eligible winners");
    } else {
        for (i, award) in data.awards.iter().take(2).enumerate() {
            let x = 64 + i as u32 * 555;
            body += &text(x, 1410, 18, p.muted, &short(&award.title, 26));
            body += &text(x, 1451, 26, p.fg, &short(&award.winner, 28));
            body += &text(
                x,
                1484,
                17,
                p.secondary,
                &short(&format!("{} · {}", award.value, award.metric), 42),
            );
        }
    }
    body += &text(
        64,
        1553,
        16,
        p.muted,
        if selected {
            "Selected additions/deletions count changed lines in selected commits, not current file size."
        } else {
            "Lifetime additions/deletions count historical changed lines, not current file size."
        },
    );
    svg(1600, p.bg, &body)
}

fn page_heading(title: &str, subtitle: &str, p: Palette) -> String {
    text(64, 66, 18, p.accent, "GIT WRAPPED")
        + &text(64, 125, 42, p.fg, title)
        + &text(64, 166, 18, p.muted, subtitle)
}

fn month_tick_step(count: usize) -> usize {
    let minimum = count.div_ceil(12).max(1);
    if count > 48 {
        minimum.div_ceil(12) * 12
    } else if count > 12 {
        minimum.div_ceil(3) * 3
    } else {
        1
    }
}

fn month_ticks(months: &[(String, u64)], p: Palette) -> String {
    let step = month_tick_step(months.len());
    let mut body = String::new();
    for (i, (month, _)) in months.iter().enumerate().step_by(step) {
        let x = 110.0 + (i as f64 + 0.5) * 1026.0 / months.len() as f64;
        let label = if step >= 12 { &month[..4] } else { month };
        body += &format!(
            "<g data-x-label=\"true\">{}</g>",
            text(x as u32, 711, 16, p.muted, label)
        );
    }
    body
}

fn commits_over_time(data: &RepositoryAnalytics, p: Palette) -> String {
    let months = monthly_counts(data);
    let mut body = page_heading(
        "Commits over time",
        "Monthly commits · author calendar dates · includes merge and empty commits",
        p,
    );
    if months.is_empty() {
        body += &text(64, 390, 22, p.muted, "No monthly activity to chart");
    } else {
        let max = months.iter().map(|m| m.1).max().unwrap_or(0).max(1);
        body += &format!(
            "<path d=\"M 110 220 V 670 H 1136\" fill=\"none\" stroke=\"{}\"/>",
            p.muted
        );
        body += &text(64, 226, 16, p.muted, &max.to_string());
        body += &text(80, 670, 16, p.muted, "0");
        let step = 1026.0 / months.len() as f64;
        for (i, (month, count)) in months.iter().enumerate() {
            let height = *count as f64 / max as f64 * 450.0;
            let width = (step * 0.72).min(52.0);
            let x = 110.0 + (i as f64 + 0.5) * step - width / 2.0;
            body += &format!(
                "<g><title>{month}: {count} commits</title>{}</g>",
                rect(x, 670.0 - height, width, height, p.accent)
            );
        }
        body += &month_ticks(&months, p);
        body += &text(
            110,
            758,
            17,
            p.muted,
            &format!(
                "{} → {} · commits per month",
                months[0].0,
                months[months.len() - 1].0
            ),
        );
    }
    svg(800, p.bg, &body)
}

fn additions_deletions(data: &RepositoryAnalytics, p: Palette) -> String {
    let months = monthly_counts(data);
    let changes: std::collections::BTreeMap<&str, &Activity> = data
        .activity
        .iter()
        .map(|a| (a.month.as_str(), a))
        .collect();
    let mut body = page_heading(
        "Growth and turnover",
        "Monthly historical line changes · cumulative net is historical arithmetic",
        p,
    );
    if months.is_empty() {
        body += &text(64, 390, 22, p.muted, "No monthly activity to chart");
    } else {
        let max = data
            .activity
            .iter()
            .map(|a| a.additions.max(a.deletions))
            .max()
            .unwrap_or(0)
            .max(1);
        let step = 1026.0 / months.len() as f64;
        let mut cumulative = 0_i128;
        let mut totals = Vec::with_capacity(months.len());
        for (month, _) in &months {
            let (added, deleted) = changes
                .get(month.as_str())
                .map(|a| (a.additions, a.deletions))
                .unwrap_or((0, 0));
            cumulative += i128::from(added) - i128::from(deleted);
            totals.push(cumulative);
        }
        let net_scale = totals
            .iter()
            .map(|n| n.unsigned_abs())
            .max()
            .unwrap_or(0)
            .max(1) as f64;
        body += &format!(
            "<path d=\"M 110 215 V 475 H 1136 M 110 605 H 1136\" fill=\"none\" stroke=\"{}\"/>",
            p.muted
        );
        body += &text(64, 226, 16, p.muted, &max.to_string());
        body += &text(80, 475, 16, p.muted, "0");
        body += &text(64, 610, 16, p.muted, "0");
        let mut points = String::new();
        for (i, (month, _)) in months.iter().enumerate() {
            let (added, deleted) = changes
                .get(month.as_str())
                .map(|a| (a.additions, a.deletions))
                .unwrap_or((0, 0));
            let center = 110.0 + (i as f64 + 0.5) * step;
            let width = (step * 0.31).min(22.0);
            let added_h = added as f64 / max as f64 * 250.0;
            let deleted_h = deleted as f64 / max as f64 * 250.0;
            body += &format!(
                "<g><title>{month}: +{added} additions, −{deleted} deletions, cumulative historical net {}</title>{}{}</g>",
                totals[i],
                rect(center - width - 1.0, 475.0 - added_h, width, added_h, p.positive),
                rect(center + 1.0, 475.0 - deleted_h, width, deleted_h, p.negative),
            );
            let net_y = 605.0 - totals[i] as f64 / net_scale * 60.0;
            points += &format!("{center:.2},{net_y:.2} ");
        }
        body += &format!(
            "<polyline points=\"{points}\" fill=\"none\" stroke=\"{}\" stroke-width=\"3\"/>",
            p.secondary
        );
        body += &rect(110.0, 504.0, 18.0, 18.0, p.positive);
        body += &text(138, 520, 17, p.fg, "Lines added");
        body += &rect(306.0, 504.0, 18.0, 18.0, p.negative);
        body += &text(334, 520, 17, p.fg, "Lines deleted");
        body += &format!(
            "<path d=\"M 534 513 H 558\" fill=\"none\" stroke=\"{}\" stroke-width=\"3\"/>",
            p.secondary
        );
        body += &text(570, 520, 17, p.fg, "Cumulative historical net");
        body += &month_ticks(&months, p);
        body += &text(
            110,
            758,
            17,
            p.muted,
            &format!(
                "{} → {} · lines, not current file size",
                months[0].0,
                months[months.len() - 1].0
            ),
        );
    }
    svg(800, p.bg, &body)
}

fn rhythm(data: &RepositoryAnalytics, p: Palette) -> String {
    let mut body = page_heading(
        "A week of rhythms",
        "Commits by author local weekday × author local hour · recorded offsets",
        p,
    );
    let mut grid = [[0_u64; 24]; 7];
    for commit in &data.commits {
        if let Ok(time) = DateTime::parse_from_rfc3339(&commit.author_time) {
            grid[time.weekday().num_days_from_monday() as usize][time.hour() as usize] += 1;
        }
    }
    let max = grid.iter().flatten().copied().max().unwrap_or(0);
    if max == 0 {
        body += &text(64, 390, 22, p.muted, "No author-local activity to chart");
    } else {
        body += &text(64, 217, 17, p.muted, "Author local weekday");
        body += &text(446, 744, 17, p.muted, "Author local hour (00:00–23:00)");
        for (day, name) in [
            "Monday",
            "Tuesday",
            "Wednesday",
            "Thursday",
            "Friday",
            "Saturday",
            "Sunday",
        ]
        .iter()
        .enumerate()
        {
            let y = 257.0 + day as f64 * 56.0;
            body += &text(64, (y + 29.0) as u32, 17, p.fg, name);
            for (hour, count) in grid[day].iter().copied().enumerate() {
                let x = 200.0 + hour as f64 * 39.0;
                let color = if count == 0 { p.muted } else { p.accent };
                body += &format!("<g opacity=\"{:.2}\"><title>{name} {hour:02}:00: {count} commits</title>{}</g>", if count == 0 { 0.15 } else { 0.35 + 0.65 * count as f64 / max as f64 }, rect(x, y, 33.0, 38.0, color));
            }
        }
        for hour in [0, 3, 6, 9, 12, 15, 18, 21, 23] {
            body += &text(198 + hour * 39, 682, 16, p.muted, &format!("{hour:02}"));
        }
        body += &text(
            200,
            714,
            16,
            p.muted,
            &format!("Stronger color = more commits · maximum {max} commits per cell"),
        );
    }
    svg(800, p.bg, &body)
}

fn highlights(data: &RepositoryAnalytics, p: Palette) -> String {
    let mut body = page_heading(
        "A few true things",
        "Fixed facts from selected history · author calendar dates",
        p,
    );
    let mut facts = Vec::new();
    if let Some(peak) = &data.insights.busiest_day {
        facts.push(format!(
            "{} had {} commit{}, the busiest author-calendar day.",
            peak.label,
            peak.count,
            if peak.count == 1 { "" } else { "s" }
        ));
    }
    if let Some(peak) = data
        .insights
        .highest_growth_month
        .as_ref()
        .filter(|peak| peak.count > 0)
    {
        facts.push(format!(
            "{} grew by {} historical net line{}, the highest monthly gain.",
            peak.label,
            peak.count,
            if peak.count == 1 { "" } else { "s" }
        ));
    }
    if let Some(cleanup) = data
        .insights
        .largest_cleanup
        .as_ref()
        .filter(|cleanup| cleanup.deletions > 0)
    {
        facts.push(format!(
            "Commit {} deleted {} historical line{}, the largest single cleanup.",
            short(&cleanup.sha, 12),
            cleanup.deletions,
            if cleanup.deletions == 1 { "" } else { "s" }
        ));
    }
    let newcomers: i64 = data.newcomers_by_month.iter().map(|peak| peak.count).sum();
    if newcomers > 0 {
        facts.push(format!(
            "{newcomers} new contributor{} first appeared in selected history.",
            if newcomers == 1 { "" } else { "s" }
        ));
    }
    if facts.is_empty() {
        body += &text(64, 350, 22, p.muted, "No highlight facts available");
    } else {
        for (i, fact) in facts.iter().enumerate() {
            let y = 260 + i as u32 * 116;
            body += &text(64, y, 19, p.accent, &format!("0{}", i + 1));
            body += &text(130, y, 23, p.fg, &short(fact, 86));
        }
    }
    svg(800, p.bg, &body)
}

/// Write the fixed report artifacts. Repository data never determines output paths.
pub fn render_report(
    data: &RepositoryAnalytics,
    output: &Path,
    theme: Theme,
) -> Result<(), String> {
    render_report_with_options(data, output, theme, true)
}

/// Write the fixed report artifacts, optionally including the 2× PNG poster.
pub fn render_report_with_options(
    data: &RepositoryAnalytics,
    output: &Path,
    theme: Theme,
    png: bool,
) -> Result<(), String> {
    checked_directory(output)?;
    let previous_cards = previous_cards(output);
    let mut current_cards = BTreeMap::new();
    let palette = theme.palette();
    let (bg, fg, accent, muted) = (palette.bg, palette.fg, palette.accent, palette.muted);
    let write = |name: &str, height, body: String| -> Result<(), String> {
        write_artifact(output, name, svg(height, bg, &body).as_bytes())
    };
    let empty = match theme {
        Theme::Dark => "#282e43",
        Theme::Light => "#e0e2ec",
    };
    let months = monthly_counts(data);
    let r = &data.repository;
    let heading =
        |title: &str| text(64, 66, 18, accent, "GIT WRAPPED") + &text(64, 125, 42, fg, title);
    let summary = poster(data, palette);
    write_artifact(output, "summary.svg", summary.as_bytes())?;
    if png {
        let bytes = raster::rasterize(&summary, 2)?;
        write_artifact(output, "summary.png", &bytes)?;
    }
    for (name, page) in [
        ("commits-over-time.svg", commits_over_time(data, palette)),
        (
            "additions-deletions.svg",
            additions_deletions(data, palette),
        ),
        ("rhythm.svg", rhythm(data, palette)),
        ("highlights.svg", highlights(data, palette)),
    ] {
        write_artifact(output, name, page.as_bytes())?;
    }
    if let Some(deep) = &data.deep {
        let mut body = heading("Current HEAD ownership");
        body += &text(
            64,
            163,
            18,
            muted,
            "Current HEAD regular text lines; date and author filters do not apply",
        );
        body += &text(64, 193, 17, muted, &format!("{} attributed + {} unknown lines · {}/{} regular files analyzed · {} binary skipped · {} submodules skipped{}",
            deep.coverage.attributed_lines, deep.coverage.unknown_lines,
            deep.coverage.analyzed_files, deep.coverage.eligible_files,
            deep.coverage.skipped_binary, deep.coverage.skipped_submodules,
            if deep.coverage.truncated { " · partial coverage" } else { "" }));
        body += &text(64, 245, 26, fg, "Contributors · surviving lines");
        let shown = top_ownership(&deep.ownership, deep.surviving_loc);
        if shown.is_empty() {
            body += &text(64, 320, 20, muted, "No attributed text lines");
        }
        for (index, (id, lines, percent)) in shown.iter().enumerate() {
            let y = 285 + index as u32 * 43;
            body += &text(64, y + 20, 17, fg, &fit_text(id, 225, 17));
            body += &rect(305.0, y as f64, percent * 6.8, 25.0, accent);
            body += &text(1010, y + 20, 16, fg, &format!("{lines} · {percent:.1}%"));
        }
        for (title, groups, y) in [
            (
                "Directories · surviving lines",
                &deep.ownership_by_directory,
                750_u32,
            ),
            (
                "Extensions · surviving lines",
                &deep.ownership_by_extension,
                1010_u32,
            ),
        ] {
            body += &text(64, y, 26, fg, title);
            let mut ranked: Vec<_> = groups.iter().collect();
            ranked.sort_by(|a, b| b.lines.cmp(&a.lines).then_with(|| a.group.cmp(&b.group)));
            if ranked.is_empty() {
                body += &text(64, y + 43, 18, muted, "No text lines");
            }
            for (index, group) in ranked.into_iter().take(5).enumerate() {
                let row = y + 35 + index as u32 * 40;
                let percent = if deep.surviving_loc == 0 {
                    0.0
                } else {
                    100.0 * group.lines as f64 / deep.surviving_loc as f64
                };
                body += &text(64, row + 18, 16, fg, &fit_text(&group.group, 230, 16));
                body += &rect(305.0, row as f64, percent * 6.8, 23.0, palette.secondary);
                body += &text(
                    1010,
                    row + 18,
                    16,
                    fg,
                    &format!("{} · {percent:.1}%", group.lines),
                );
            }
        }
        let contents = svg(1300, bg, &body);
        write_artifact(output, "ownership.svg", contents.as_bytes())?;
        current_cards.insert("ownership.svg".to_owned(), contents);

        let mut body = heading("Sampled ownership over time");
        body += &text(64, 165, 18, muted, "At most 12 selected commits, evenly sampled by commit index; plotted on calendar dates");
        body += &text(
            64,
            192,
            17,
            muted,
            "Each stacked column is one measured snapshot; gaps between samples are unmeasured.",
        );
        let colors = [
            accent,
            palette.secondary,
            palette.positive,
            palette.negative,
            fg,
            muted,
            "#d2a861",
            "#8ea8ff",
        ];
        let mut totals = BTreeMap::<String, u64>::new();
        for snapshot in &deep.historical_ownership {
            for part in &snapshot.by_author {
                if part.author_id != "unknown" {
                    *totals.entry(part.author_id.clone()).or_default() += part.lines;
                }
            }
        }
        let mut ranking: Vec<_> = totals.into_iter().collect();
        ranking.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let mut selected: Vec<_> = ranking.into_iter().take(8).map(|(id, _)| id).collect();
        selected.sort();
        let mut dated = deep
            .historical_ownership
            .iter()
            .map(|snapshot| {
                NaiveDate::parse_from_str(&snapshot.author_date, "%Y-%m-%d")
                    .map(|date| (date, snapshot))
                    .map_err(|e| format!("invalid ownership snapshot date: {e}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        dated.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.sha.cmp(&b.1.sha)));
        for percent in [100, 50, 0] {
            let y = 250 + (100 - percent) * 4;
            body += &format!("<path d=\"M 100 {y} H 1100\" stroke=\"{muted}\" opacity=\"0.35\"/>");
            body += &text(48, y + 5, 15, muted, &format!("{percent}%"));
        }
        if let (Some((first, _)), Some((last, _))) = (dated.first(), dated.last()) {
            let span = (*last - *first).num_days();
            for (date, snapshot) in &dated {
                let x = if span == 0 {
                    600.0
                } else {
                    120.0 + (*date - *first).num_days() as f64 * 960.0 / span as f64
                };
                let mut y = 650.0;
                let mut segments = String::new();
                let mut breakdown = Vec::new();
                let mut other = 0_u64;
                for part in &snapshot.by_author {
                    if part.author_id != "unknown" && !selected.contains(&part.author_id) {
                        other += part.lines;
                    }
                }
                for (part_index, id) in selected.iter().enumerate() {
                    let lines = snapshot
                        .by_author
                        .iter()
                        .find(|part| &part.author_id == id)
                        .map_or(0, |part| part.lines);
                    if lines > 0 {
                        breakdown.push(format!("{id}: {lines} lines"));
                    }
                    let height = if snapshot.total_lines == 0 {
                        0.0
                    } else {
                        400.0 * lines as f64 / snapshot.total_lines as f64
                    };
                    y -= height;
                    segments += &rect(x - 18.0, y, 36.0, height, colors[part_index]);
                }
                for (lines, color) in [
                    (other, "#a6a6a6"),
                    (snapshot.coverage.unknown_lines, "#777777"),
                ] {
                    let height = if snapshot.total_lines == 0 {
                        0.0
                    } else {
                        400.0 * lines as f64 / snapshot.total_lines as f64
                    };
                    y -= height;
                    segments += &rect(x - 18.0, y, 36.0, height, color);
                }
                if other > 0 {
                    breakdown.push(format!("Other: {other} lines"));
                }
                if snapshot.coverage.unknown_lines > 0 {
                    breakdown.push(format!(
                        "unknown: {} lines",
                        snapshot.coverage.unknown_lines
                    ));
                }
                let detail = format!("{}: {} sampled lines; {}/{} regular files; {} binary skipped; {} submodules skipped; {}{}",
                    snapshot.author_date, snapshot.total_lines,
                    snapshot.coverage.analyzed_files, snapshot.coverage.eligible_files,
                    snapshot.coverage.skipped_binary, snapshot.coverage.skipped_submodules,
                    breakdown.join(", "),
                    if snapshot.coverage.truncated { "; partial coverage" } else { "" });
                body += &format!("<g><title>{}</title>{segments}</g>", escape_xml(&detail));
                body += &format!("<text transform=\"translate({x:.1},685) rotate(-60)\" font-size=\"14\" fill=\"{muted}\" font-family=\"Lato,system-ui,sans-serif\">{}</text>", escape_xml(&snapshot.author_date));
            }
        } else {
            body += &text(64, 440, 23, fg, "No snapshots within the deep budget");
        }
        let mut legend: Vec<_> = selected
            .iter()
            .enumerate()
            .map(|(i, id)| (id.as_str(), colors[i]))
            .collect();
        if deep.historical_ownership.iter().any(|snapshot| {
            snapshot
                .by_author
                .iter()
                .any(|part| part.author_id != "unknown" && !selected.contains(&part.author_id))
        }) {
            legend.push(("Other", "#a6a6a6"));
        }
        if deep
            .historical_ownership
            .iter()
            .any(|snapshot| snapshot.coverage.unknown_lines > 0)
        {
            legend.push(("unknown", "#777777"));
        }
        for (index, (id, color)) in legend.iter().enumerate() {
            let col = index / 5;
            let row = index % 5;
            let x = 64 + col as u32 * 535;
            let y = 825 + row as u32 * 35;
            body += &rect(x as f64, y as f64 - 15.0, 18.0, 18.0, color);
            body += &text(x + 30, y, 16, fg, &fit_text(id, 420, 16));
        }
        body += &text(
            64,
            1015,
            16,
            muted,
            &format!(
                "{} sampled snapshots · {}/{} regular files analyzed · {} unknown lines · {}",
                deep.historical_ownership.len(),
                deep.historical_ownership
                    .iter()
                    .map(|s| s.coverage.analyzed_files)
                    .sum::<u64>(),
                deep.historical_ownership
                    .iter()
                    .map(|s| s.coverage.eligible_files)
                    .sum::<u64>(),
                deep.historical_ownership
                    .iter()
                    .map(|s| s.coverage.unknown_lines)
                    .sum::<u64>(),
                if deep.coverage.truncated {
                    "partial coverage"
                } else {
                    "within budget"
                }
            ),
        );
        let contents = svg(1060, bg, &body);
        write_artifact(output, "ownership-over-time.svg", contents.as_bytes())?;
        current_cards.insert("ownership-over-time.svg".to_owned(), contents);

        let mut body = heading("Ship of Theseus");
        body += &text(64, 166, 21, muted, "sampled surviving line identities");
        body += &text(
            64,
            202,
            17,
            muted,
            "Git blame origins; rewrites and unexamined files may change this estimate.",
        );
        for (percent, y) in [(100, 270), (50, 410), (0, 550)] {
            body += &format!("<path d=\"M 100 {y} H 1100\" stroke=\"{muted}\" opacity=\"0.35\"/>");
            body += &text(48, y + 5, 16, muted, &format!("{percent}%"));
        }
        let mut dated = deep
            .survival
            .iter()
            .map(|point| {
                NaiveDate::parse_from_str(&point.snapshot_date, "%Y-%m-%d")
                    .map(|date| (date, point))
                    .map_err(|error| format!("invalid survival snapshot date: {error}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        dated.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then_with(|| a.1.snapshot_sha.cmp(&b.1.snapshot_sha))
        });
        let mut coordinates = Vec::new();
        let mut markers = String::new();
        if let (Some((first, _)), Some((last, _))) = (dated.first(), dated.last()) {
            let span = (*last - *first).num_days();
            for (date, point) in &dated {
                let x = if span == 0 {
                    600.0
                } else {
                    100.0 + (*date - *first).num_days() as f64 * 1000.0 / span as f64
                };
                let percent = if point.percent.is_finite() {
                    point.percent.clamp(0.0, 100.0)
                } else {
                    0.0
                };
                let y = 550.0 - percent * 2.8;
                coordinates.push(format!("{x:.1},{y:.1}"));
                markers += &format!("<g><title>{}: {percent:.1}% ({}/{} sampled line identities)</title><circle cx=\"{x:.1}\" cy=\"{y:.1}\" r=\"7\" fill=\"{accent}\"/></g>", escape_xml(&point.snapshot_date), point.surviving_lines, point.original_lines);
            }
            body += &text(100, 585, 16, muted, &first.to_string());
            if last != first {
                body += &text(988, 585, 16, muted, &last.to_string());
            }
        }
        if !coordinates.is_empty() {
            body += &format!(
                "<polyline fill=\"none\" stroke=\"{accent}\" stroke-width=\"4\" points=\"{}\"/>",
                coordinates.join(" ")
            );
            body += &markers;
        } else {
            body += &text(
                64,
                360,
                25,
                fg,
                "No selected snapshots within the deep budget",
            );
        }
        let covered_files: u64 = deep.survival.iter().map(|point| point.sampled_files).sum();
        let eligible_files: u64 = deep.survival.iter().map(|point| point.eligible_files).sum();
        body += &text(
            64,
            625,
            17,
            muted,
            &format!(
                "{} snapshots selected evenly by commit index; plotted by calendar day",
                deep.survival.len()
            ),
        );
        body += &text(
            64,
            652,
            17,
            muted,
            &format!(
                "{covered_files}/{eligible_files} sampled text / regular files · {}",
                if deep.coverage.truncated {
                    "partial coverage"
                } else {
                    "within budget"
                }
            ),
        );
        body += &text(
            64,
            700,
            17,
            fg,
            &format!(
                "Current line age: median {} days · oldest {} days · {} future-dated",
                deep.code_age
                    .median_days
                    .map_or("unknown".into(), |n| n.to_string()),
                deep.code_age
                    .oldest_days
                    .map_or("unknown".into(), |n| n.to_string()),
                deep.code_age.future_dated_lines
            ),
        );
        let cohorts = deep
            .code_age
            .year_cohorts
            .iter()
            .map(|c| format!("{}: {}", c.year, c.surviving_lines))
            .collect::<Vec<_>>()
            .join(" · ");
        body += &text(
            64,
            748,
            17,
            muted,
            &short(&format!("Surviving line origin years: {cohorts}"), 110),
        );
        let contents = svg(800, bg, &body);
        write_artifact(output, "ship-of-theseus.svg", contents.as_bytes())?;
        current_cards.insert("ship-of-theseus.svg".to_owned(), contents);

        let mut body = heading("Lines removed by / originally authored by");
        body += &text(64, 165, 18, muted, "Nonblank lines removed in first-parent diffs of selected non-merge commits, by parent-revision blame");
        let mut involvement = BTreeMap::<&str, u64>::new();
        for cell in &deep.interactions {
            *involvement.entry(&cell.deleting_author_id).or_default() += cell.deleted_lines;
            *involvement.entry(&cell.original_author_id).or_default() += cell.deleted_lines;
        }
        let mut people: Vec<_> = involvement.into_iter().collect();
        people.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        let total_people = people.len();
        let mut people: Vec<_> = people.into_iter().take(10).map(|(id, _)| id).collect();
        people.sort_unstable();
        let max = deep
            .interactions
            .iter()
            .map(|cell| cell.deleted_lines)
            .max()
            .unwrap_or(1)
            .max(1) as f64;
        body += &text(
            64,
            215,
            16,
            muted,
            "Rows: lines removed by · columns: originally authored by",
        );
        for (column, id) in people.iter().enumerate() {
            let x = 330 + column as u32 * 82 + 40;
            body += &format!("<text transform=\"translate({x},370) rotate(-45)\" font-size=\"14\" fill=\"{fg}\" font-family=\"Lato,system-ui,sans-serif\">{}</text>", escape_xml(&fit_text(id, 170, 14)));
        }
        for (row, deleting) in people.iter().enumerate() {
            let y = 390 + row as u32 * 40;
            body += &text(64, y + 26, 16, fg, &fit_text(deleting, 250, 16));
            for (column, original) in people.iter().enumerate() {
                let x = 330 + column as u32 * 82;
                let lines = deep
                    .interactions
                    .iter()
                    .find(|cell| {
                        cell.deleting_author_id == *deleting && cell.original_author_id == *original
                    })
                    .map_or(0, |cell| cell.deleted_lines);
                if lines == 0 {
                    body += &rect(x as f64, y as f64, 78.0, 36.0, empty);
                } else {
                    body += &format!(
                        "<g opacity=\"{:.2}\"><title>{}</title>{}</g>",
                        0.35 + lines as f64 / max * 0.65,
                        escape_xml(&format!(
                            "{deleting} removed {lines} lines originally authored by {original}"
                        )),
                        rect(x as f64, y as f64, 78.0, 36.0, accent)
                    );
                    body += &text(x + 8, y + 24, 15, fg, &lines.to_string());
                }
            }
        }
        if people.is_empty() {
            body += &text(64, 420, 23, fg, "No measured removed lines");
        }
        body += &text(
            64,
            825,
            16,
            muted,
            &format!(
                "Top {} of {total_people} contributors by lines removed or authored · unit: removed nonblank lines · merges excluded",
                people.len()
            ),
        );
        body += &text(
            64,
            855,
            16,
            muted,
            &format!(
                "{} deletion-bearing commits examined · {} skipped{}",
                deep.coverage.interaction_commits_examined,
                deep.coverage.interaction_commits_skipped,
                if deep.coverage.interaction_commits_skipped > 0 {
                    " · partial coverage"
                } else {
                    ""
                }
            ),
        );
        let contents = svg(900, bg, &body);
        write_artifact(output, "contributor-interactions.svg", contents.as_bytes())?;
        current_cards.insert("contributor-interactions.svg".to_owned(), contents);

        let mut body = heading("Files that change together");
        body += &text(
            64,
            165,
            18,
            muted,
            "Distinct selected commits changing both paths; co-change, not causation",
        );
        let names: BTreeMap<&str, &str> = data
            .files
            .iter()
            .map(|file| (file.path_id.as_str(), file.display_path.as_str()))
            .collect();
        let name = |id: &str| names.get(id).copied().unwrap_or(id).to_owned();
        for (i, pair) in deep.coupling.iter().take(10).enumerate() {
            let y = 212 + i as u32 * 58;
            let suffix = format!(" · {} commits", pair.cochange_commits);
            body += &text(
                64,
                y,
                16,
                fg,
                &format!(
                    "{}. {}",
                    i + 1,
                    fit_text(&name(&pair.first_path_id), 1000, 16)
                ),
            );
            let room = 1000_u32.saturating_sub(suffix.chars().count() as u32 * 16);
            body += &text(
                100,
                y + 24,
                16,
                fg,
                &format!(
                    "+ {}{suffix}",
                    fit_text(&name(&pair.second_path_id), room, 16)
                ),
            );
        }
        if deep.coupling.is_empty() {
            body += &text(64, 245, 20, muted, "No file pairs changed together");
        }
        body += &text(
            64,
            795,
            16,
            muted,
            &format!(
                "Top {} of {} pairs · commits with 2–50 changed paths · top 200 files by revisions",
                deep.coupling.len().min(10),
                deep.coupling.len()
            ),
        );
        body += &text(
            64,
            825,
            16,
            muted,
            &format!(
                "{} commits counted · {} commits with more than 50 paths skipped",
                deep.coverage.coupling_commits_examined, deep.coverage.coupling_commits_skipped
            ),
        );
        let contents = svg(860, bg, &body);
        write_artifact(output, "file-coupling.svg", contents.as_bytes())?;
        current_cards.insert("file-coupling.svg".to_owned(), contents);
    }

    let mut body = heading("The people behind the commits");
    let max = data
        .contributors
        .iter()
        .map(|c| c.commits)
        .max()
        .unwrap_or(1)
        .max(1) as f64;
    for (i, c) in data.contributors.iter().take(10).enumerate() {
        let y = 195 + i as u32 * 54;
        body += &text(64, y + 22, 19, fg, &fit_text(&c.name, 265, 19));
        body += &rect(
            345.0,
            y as f64,
            c.commits as f64 / max * 650.0,
            30.0,
            accent,
        );
        body += &text(1020, y + 22, 18, fg, &c.commits.to_string());
    }
    body += &text(
        64,
        765,
        16,
        muted,
        if data.contributors.is_empty() {
            "No contributors"
        } else {
            "Top 10 contributors · bar length = commits"
        },
    );
    write("contributors.svg", 800, body)?;

    let mut people: Vec<_> = data.contributors.iter().collect();
    people.sort_by(|a, b| b.commits.cmp(&a.commits).then_with(|| a.id.cmp(&b.id)));
    let mut body = heading("Contributor mix");
    body += &text(
        64,
        175,
        18,
        muted,
        "Share of selected commits · after mailmap",
    );
    let total = people.iter().map(|c| c.commits).sum::<u64>().max(1) as f64;
    let colors = [
        palette.accent,
        palette.secondary,
        palette.positive,
        palette.negative,
    ];
    let mut x = 64.0;
    for (i, c) in people.iter().take(10).enumerate() {
        let width = c.commits as f64 / total * 1072.0;
        body += &rect(x, 215.0, width, 44.0, colors[i % colors.len()]);
        x += width;
        let row_y = 307 + i as u32 * 40;
        body += &rect(
            64.0,
            (row_y - 17) as f64,
            17.0,
            17.0,
            colors[i % colors.len()],
        );
        body += &text(
            94,
            row_y,
            18,
            fg,
            &format!(
                "{} · {} commits ({:.1}%)",
                fit_text(
                    &c.name,
                    1042_u32.saturating_sub(
                        format!(
                            " · {} commits ({:.1}%)",
                            c.commits,
                            c.commits as f64 / total * 100.0
                        )
                        .chars()
                        .count() as u32
                            * 18,
                    ),
                    18,
                ),
                c.commits,
                c.commits as f64 / total * 100.0
            ),
        );
    }
    if x < 1136.0 {
        body += &rect(x, 215.0, 1136.0 - x, 44.0, muted);
        if people.len() > 10 {
            body += &text(64, 737, 16, muted, "Muted segment = all other contributors");
        }
    }
    body += &text(
        64,
        775,
        16,
        muted,
        &format!(
            "Top {} of {} contributors · bar width = commit share",
            people.len().min(10),
            people.len()
        ),
    );
    write("contributor-mix.svg", 800, body)?;

    let mut files: Vec<_> = data.files.iter().collect();
    files.sort_by(|a, b| {
        b.churn
            .cmp(&a.churn)
            .then_with(|| a.path_id.cmp(&b.path_id))
    });
    let mut body = heading("Files by historical churn");
    body += &text(
        64,
        176,
        18,
        muted,
        "Lines added + deleted across selected history",
    );
    for (i, file) in files.iter().take(10).enumerate() {
        let status = if file.exists_at_head {
            "current"
        } else {
            "historical path"
        };
        body += &text(
            64,
            222 + i as u32 * 48,
            18,
            fg,
            &format!(
                "{}. {} · {} lines · {status}",
                i + 1,
                fit_text(
                    &file.display_path,
                    1072_u32.saturating_sub(
                        (format!("{}. ", i + 1).chars().count()
                            + format!(" · {} lines · {status}", file.churn)
                                .chars()
                                .count()) as u32
                            * 18,
                    ),
                    18,
                ),
                file.churn
            ),
        );
    }
    if files.is_empty() {
        body += &text(64, 245, 20, muted, "No file changes in selected history");
    }
    body += &text(
        64,
        765,
        16,
        muted,
        &format!(
            "Top {} of {} files · historical path = absent at HEAD",
            files.len().min(10),
            files.len()
        ),
    );
    write("file-churn.svg", 800, body)?;

    let mut directories: Vec<_> = data.directories.iter().collect();
    directories.sort_by(|a, b| {
        b.churn
            .cmp(&a.churn)
            .then_with(|| a.path_id.cmp(&b.path_id))
    });
    let mut body = heading("Directory activity");
    body += &text(
        64,
        176,
        18,
        muted,
        "Historical line churn in each changed file's immediate parent",
    );
    for (i, dir) in directories.iter().take(10).enumerate() {
        body += &text(
            64,
            222 + i as u32 * 48,
            18,
            fg,
            &format!(
                "{}. {} · {} lines · {} commits · {} current files",
                i + 1,
                fit_text(
                    &dir.display_path,
                    1072_u32.saturating_sub(
                        (format!("{}. ", i + 1).chars().count()
                            + format!(
                                " · {} lines · {} commits · {} current files",
                                dir.churn, dir.commits, dir.current_file_count
                            )
                            .chars()
                            .count()) as u32
                            * 18,
                    ),
                    18,
                ),
                dir.churn,
                dir.commits,
                dir.current_file_count
            ),
        );
    }
    if directories.is_empty() {
        body += &text(
            64,
            245,
            20,
            muted,
            "No directory activity in selected history",
        );
    }
    body += &text(
        64,
        765,
        16,
        muted,
        &format!(
            "Top {} of {} directories · current files counted recursively at HEAD",
            directories.len().min(10),
            directories.len()
        ),
    );
    write("directories.svg", 800, body)?;

    let mut top_people: Vec<_> = people.into_iter().take(20).collect();
    top_people.sort_by(|a, b| a.id.cmp(&b.id));
    let card_names = contributor_card_names(
        data.contributors
            .iter()
            .map(|c| (c.id.clone(), c.commits))
            .collect(),
    );
    for c in top_people {
        let name = &card_names[&c.id];
        let mut body = heading(&fit_text(&c.name, 1072, 42));
        body += &text(64, 174, 18, muted, &fit_text(&c.id, 1072, 18));
        for (i, (label, value)) in [
            ("Commits", c.commits.to_string()),
            ("Lines added", c.additions.to_string()),
            ("Lines deleted", c.deletions.to_string()),
            ("Active days", c.active_days.to_string()),
        ]
        .iter()
        .enumerate()
        {
            let x = 64 + i as u32 % 2 * 540;
            let y = 275 + i as u32 / 2 * 110;
            body += &text(x, y, 37, fg, value);
            body += &text(x, y + 28, 17, muted, label);
        }
        body += &text(64, 535, 18, accent, "Author local hour · commits");
        let max = c.commits_by_hour.iter().copied().max().unwrap_or(0).max(1) as f64;
        for (hour, count) in c.commits_by_hour.iter().enumerate() {
            let height = *count as f64 / max * 126.0;
            body += &rect(
                72.0 + hour as f64 * 44.0,
                691.0 - height,
                28.0,
                height,
                palette.secondary,
            );
        }
        for hour in [0, 6, 12, 18, 23] {
            body += &text(72 + hour * 44, 721, 16, muted, &format!("{hour:02}"));
        }
        body += &text(
            64,
            770,
            16,
            muted,
            &format!(
                "{} → {} · author-local dates",
                c.first_contribution.split('T').next().unwrap_or(""),
                c.latest_contribution.split('T').next().unwrap_or("")
            ),
        );
        let path = format!("contributors/{name}.svg");
        let contents = svg(800, bg, &body);
        write_artifact(output, &path, contents.as_bytes())?;
        current_cards.insert(path, contents);
    }

    let mut body = heading("Every month tells a story");
    let max = months.iter().map(|a| a.1).max().unwrap_or(1).max(1) as f64;
    let (plot_top, plot_bottom) = (200, 670);
    body += &format!(
        "<path d=\"M 100 {plot_top} V {plot_bottom} H 1136\" fill=\"none\" stroke=\"{muted}\"/>"
    );
    body += &text(64, plot_top, 16, muted, &format!("{max:.0}"));
    body += &text(64, plot_bottom, 16, muted, "0");
    let step = 1010.0 / months.len().max(1) as f64;
    for (i, a) in months.iter().enumerate() {
        let h = a.1 as f64 / max * (plot_bottom - plot_top) as f64;
        body += &format!(
            "<g><title>{}</title>{}</g>",
            escape_xml(&format!("{}: {} commits", a.0, a.1)),
            rect(
                110.0 + i as f64 * step,
                plot_bottom as f64 - h,
                step * 0.8,
                h,
                accent
            )
        );
        if i % months.len().div_ceil(8).max(1) == 0 {
            body += &text((110.0 + i as f64 * step) as u32, 704, 15, muted, &a.0);
        }
    }
    body += &text(
        100,
        757,
        18,
        muted,
        if months.is_empty() {
            "No activity"
        } else {
            "Monthly commits · includes merge and empty commits"
        },
    );
    write("activity.svg", 800, body)?;

    let mut body = heading("Small steps. Lasting history.");
    let cells: Vec<_> = data
        .activity_heatmap
        .iter()
        .filter_map(|c| {
            NaiveDate::parse_from_str(&c.date, "%Y-%m-%d")
                .ok()
                .map(|d| (d, c.commits))
        })
        .collect();
    if let Some(last) = cells.iter().map(|c| c.0).max() {
        let first = last
            .checked_sub_days(chrono::Days::new(364))
            .ok_or("calendar window out of range")?;
        let offset = first.weekday().num_days_from_monday() as i64;
        let visible: std::collections::BTreeMap<_, _> = cells
            .iter()
            .copied()
            .filter(|(date, _)| *date >= first && *date <= last)
            .collect();
        let max = visible.values().copied().max().unwrap_or(1).max(1) as f64;
        body += &text(
            64,
            185,
            20,
            muted,
            &format!("Trailing 365 days · {first} — {last}"),
        );
        for (row, day) in ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
            .iter()
            .enumerate()
        {
            body += &text(64, 289 + row as u32 * 24, 16, muted, day);
        }
        let mut date = first;
        loop {
            let week = ((date - first).num_days() + offset) / 7;
            let x = 112 + week as u32 * 19;
            let y = 275 + date.weekday().num_days_from_monday() * 24;
            // A very short opening month has no room for a label before the next month.
            if date.day() == 1 || (date == first && date.day() <= 15) {
                body += &text(x, 248, 16, muted, &date.format("%b").to_string());
            }
            let count = visible.get(&date).copied().unwrap_or(0);
            if count == 0 {
                body += &rect(x as f64, y as f64, 16.0, 20.0, empty);
            } else {
                body += &format!(
                    "<g opacity=\"{:.2}\"><title>{date}: {count} commits</title>{}</g>",
                    0.4 + count as f64 / max * 0.6,
                    rect(x as f64, y as f64, 16.0, 20.0, accent),
                );
            }
            if date == last {
                break;
            }
            date = date.succ_opt().ok_or("calendar date out of range")?;
        }
    }
    body += &text(
        64,
        765,
        16,
        muted,
        if cells.is_empty() {
            "No activity"
        } else {
            "Monday → Sunday · brighter = more commits · author calendar dates"
        },
    );
    write("activity-heatmap.svg", 800, body)?;
    let mut award_cards: std::collections::BTreeMap<&str, &str> = [
        ("commit-machine", "Commit Machine"),
        ("code-creator", "Code Creator"),
        ("code-destroyer", "Code Destroyer"),
        ("night-owl", "Night Owl"),
    ]
    .into_iter()
    .collect();
    for award in &data.awards {
        if valid_award_slug(&award.slug) {
            award_cards.insert(&award.slug, &award.title);
        } else {
            return Err(format!("invalid award slug: {}", award.slug));
        }
    }
    for (slug, title) in award_cards {
        let mut body = heading(&fit_text(title, 1072, 42));
        body += &text(64, 185, 20, muted, &fit_text(&r.name, 1072, 20));
        if let Some(a) = data.awards.iter().find(|a| a.slug == slug) {
            body += &text(64, 310, 52, fg, &fit_text(&a.winner, 1072, 52));
            body += &text(
                64,
                391,
                32,
                accent,
                &fit_text(&format!("{} · {}", a.value, a.metric), 1072, 32),
            );
            for (i, line) in a
                .explanation
                .chars()
                .collect::<Vec<_>>()
                .chunks(1072 / 21)
                .take(3)
                .enumerate()
            {
                body += &text(
                    64,
                    485 + i as u32 * 30,
                    21,
                    muted,
                    &line.iter().collect::<String>(),
                );
            }
        } else {
            body += &text(64, 310, 44, fg, "No eligible winner");
        }
        let path = format!("awards/{slug}.svg");
        let contents = svg(675, bg, &body);
        write_artifact(output, &path, contents.as_bytes())?;
        current_cards.insert(path, contents);
    }
    let mut json =
        serde_json::to_vec_pretty(data).map_err(|e| format!("serialize data.json: {e}"))?;
    json.push(b'\n');
    write_artifact(output, "data.json", &json)?;
    for (name, expected) in &previous_cards {
        if !current_cards.contains_key(name) {
            remove_stale_card(output, name, expected)?;
        }
    }
    let mut manifest = serde_json::to_vec(&current_cards)
        .map_err(|e| format!("serialize card-manifest.json: {e}"))?;
    manifest.push(b'\n');
    write_artifact(output, "card-manifest.json", &manifest)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oversized_stale_card_is_preserved_without_reading_it() {
        use std::time::{Duration, SystemTime};

        let temp_root = fs::canonicalize(std::env::temp_dir()).unwrap();
        let out = tempfile::Builder::new().tempdir_in(temp_root).unwrap();
        let cards = out.path().join("contributors");
        fs::create_dir(&cards).unwrap();
        let path = cards.join("abcdef.svg");
        let file = fs::File::create(&path).unwrap();
        file.set_len(4 * 1024 * 1024).unwrap();
        let old_access = SystemTime::UNIX_EPOCH + Duration::from_secs(946_684_800);
        file.set_times(fs::FileTimes::new().set_accessed(old_access))
            .unwrap();
        drop(file);
        assert_eq!(fs::metadata(&path).unwrap().accessed().unwrap(), old_access);

        remove_stale_card(out.path(), "contributors/abcdef.svg", "<svg/>").unwrap();
        let meta = fs::metadata(path).unwrap();
        assert_eq!(meta.len(), 4 * 1024 * 1024);
        assert_eq!(meta.accessed().unwrap(), old_access);
    }

    #[test]
    fn render_escapes_controls_and_xml() {
        assert_eq!(
            escape_xml("<&>\"'\n\t\u{0}\u{85}"),
            "&lt;&amp;&gt;&quot;&apos;    "
        );
    }
}
