use crate::model::RepositoryAnalytics;
use chrono::{Datelike, NaiveDate};
use std::{
    fs,
    io::ErrorKind,
    path::{Component, Path},
};

#[derive(Clone, Copy, Debug)]
pub enum Theme {
    Dark,
    Light,
}

#[derive(Clone, Copy)]
struct Palette {
    bg: &'static str,
    fg: &'static str,
    muted: &'static str,
    accent: &'static str,
    secondary: &'static str,
    positive: &'static str,
    negative: &'static str,
}

impl Theme {
    fn palette(self) -> Palette {
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

fn text(x: u32, y: u32, size: u32, color: &str, value: &str) -> String {
    format!("<text x=\"{x}\" y=\"{y}\" font-family=\"system-ui,sans-serif\" font-size=\"{size}\" fill=\"{color}\">{}</text>", escape_xml(value))
}

fn short(value: &str, limit: usize) -> String {
    let mut s: String = value.chars().take(limit).collect();
    if value.chars().count() > limit {
        s.push('…');
    }
    s
}

fn svg(height: u32, background: &str, body: &str) -> String {
    format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1200\" height=\"{height}\" viewBox=\"0 0 1200 {height}\"><rect width=\"100%\" height=\"100%\" fill=\"{background}\"/>{body}</svg>\n")
}

fn rect(x: f64, y: f64, w: f64, h: f64, color: &str) -> String {
    format!("<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{h:.2}\" rx=\"3\" fill=\"{color}\"/>")
}

// Preserve calendar gaps so bars and sparkline positions represent elapsed time.
fn monthly_counts(data: &RepositoryAnalytics) -> Vec<(String, u64)> {
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

fn checked_directory(path: &Path) -> Result<(), String> {
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
    let mut body = text(64, 70, 20, p.accent, "GIT WRAPPED");
    body += &text(64, 138, 48, p.fg, &short(&r.name, 38));
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
    body += &text(
        64,
        244,
        18,
        p.muted,
        "GROWTH  ·  PEOPLE  ·  RHYTHM  ·  AWARDS",
    );

    for (i, (value, label)) in [
        (r.total_commits.to_string(), "Commits"),
        (r.total_contributors.to_string(), "Contributors"),
        (r.tracked_files.to_string(), "Tracked files at HEAD"),
        (r.age_days.to_string(), "Days of history"),
        (format!("+{}", r.additions), "Lifetime additions"),
        (format!("−{}", r.deletions), "Lifetime deletions"),
    ]
    .iter()
    .enumerate()
    {
        let x = 64 + (i % 3) as u32 * 365;
        let y = 322 + (i / 3) as u32 * 103;
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
        "Lifetime additions/deletions count historical changed lines, not current file size.",
    );
    svg(1600, p.bg, &body)
}

/// Write the fixed report artifacts. Repository data never determines output paths.
pub fn render_report(
    data: &RepositoryAnalytics,
    output: &Path,
    theme: Theme,
) -> Result<(), String> {
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
    write_artifact(output, "summary.svg", poster(data, palette).as_bytes())?;

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
        body += &text(64, y + 22, 19, fg, &short(&c.name, 23));
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
    for (slug, title) in [
        ("commit-machine", "Commit Machine"),
        ("code-creator", "Code Creator"),
        ("code-destroyer", "Code Destroyer"),
        ("night-owl", "Night Owl"),
    ] {
        let mut body = heading(title);
        body += &text(64, 185, 20, muted, &short(&r.name, 60));
        if let Some(a) = data.awards.iter().find(|a| a.slug == slug) {
            body += &text(64, 310, 52, fg, &short(&a.winner, 32));
            body += &text(
                64,
                391,
                32,
                accent,
                &short(&format!("{} · {}", a.value, a.metric), 58),
            );
            for (i, line) in a
                .explanation
                .chars()
                .collect::<Vec<_>>()
                .chunks(80)
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
        write(&format!("awards/{slug}.svg"), 675, body)?;
    }
    let mut json =
        serde_json::to_vec_pretty(data).map_err(|e| format!("serialize data.json: {e}"))?;
    json.push(b'\n');
    write_artifact(output, "data.json", &json)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn render_escapes_controls_and_xml() {
        assert_eq!(
            escape_xml("<&>\"'\n\t\u{0}\u{85}"),
            "&lt;&amp;&gt;&quot;&apos;    "
        );
    }
}
