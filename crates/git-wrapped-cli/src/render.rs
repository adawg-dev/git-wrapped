use crate::model::RepositoryAnalytics;
use chrono::{Datelike, NaiveDate};
use std::{fs, io::Write, path::Path};

#[derive(Clone, Copy, Debug)]
pub enum Theme {
    Dark,
    Light,
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

/// Write the fixed report artifacts. Repository data never determines output paths.
pub fn render_report(
    data: &RepositoryAnalytics,
    output: &Path,
    theme: Theme,
) -> Result<(), String> {
    let (bg, fg, accent, muted) = match theme {
        Theme::Dark => ("#10131f", "#f4f5fb", "#8f7cff", "#a7aec6"),
        Theme::Light => ("#f7f7fc", "#172033", "#5b43c9", "#535e76"),
    };
    fs::create_dir_all(output.join("awards"))
        .map_err(|e| format!("create {}: {e}", output.display()))?;
    let write = |name: &str, height, body: String| -> Result<(), String> {
        let path = output.join(name);
        // Refuse pre-existing symlinks instead of following them outside the report.
        if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(format!("refusing symlink output: {}", path.display()));
        }
        fs::write(&path, svg(height, bg, &body))
            .map_err(|e| format!("write {}: {e}", path.display()))
    };
    if fs::symlink_metadata(output.join("awards")).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err("refusing symlink awards directory".into());
    }
    let empty = match theme {
        Theme::Dark => "#282e43",
        Theme::Light => "#e0e2ec",
    };
    let months = monthly_counts(data);
    let r = &data.repository;
    let heading =
        |title: &str| text(64, 66, 18, accent, "GIT WRAPPED") + &text(64, 125, 42, fg, title);
    let mut body = heading(&short(&r.name, 42));
    body += &text(
        64,
        163,
        18,
        muted,
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
    for (i, (value, label)) in [
        (r.total_commits.to_string(), "commits"),
        (r.total_contributors.to_string(), "contributors"),
        (format!("+{}", r.additions), "lines ever added"),
        (format!("−{}", r.deletions), "lines ever deleted"),
    ]
    .iter()
    .enumerate()
    {
        let x = 64 + i as u32 * 280;
        body += &text(x, 252, 40, fg, value);
        body += &text(x, 284, 18, muted, label);
    }
    body += &text(64, 348, 18, accent, "TOP CONTRIBUTORS");
    body += &text(625, 348, 18, accent, "AWARD HIGHLIGHTS");
    for (i, c) in data.contributors.iter().take(3).enumerate() {
        body += &text(
            64,
            389 + i as u32 * 38,
            22,
            fg,
            &format!("{}  ·  {} commits", short(&c.name, 25), c.commits),
        );
    }
    for (i, a) in data.awards.iter().take(3).enumerate() {
        body += &text(
            625,
            389 + i as u32 * 38,
            20,
            fg,
            &format!("{} · {}", short(&a.title, 22), short(&a.winner, 19)),
        );
    }
    body += &text(
        64,
        530,
        16,
        muted,
        "ACTIVITY · 12 BUCKETS ACROSS ANALYZED MONTHS",
    );
    let mut buckets = [0_u64; 12];
    for (i, (_, count)) in months.iter().enumerate() {
        buckets[i * 12 / months.len()] += count;
    }
    let max = buckets.iter().copied().max().unwrap_or(1).max(1) as f64;
    let points = buckets
        .iter()
        .enumerate()
        .map(|(i, n)| {
            format!(
                "{:.2},{:.2}",
                64.0 + i as f64 * 97.0,
                623.0 - *n as f64 / max * 60.0
            )
        })
        .collect::<Vec<_>>()
        .join(" ");
    body += &format!(
        "<polyline points=\"{points}\" fill=\"none\" stroke=\"{accent}\" stroke-width=\"4\"/>"
    );
    write("summary.svg", 675, body)?;

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
    body += &format!("<path d=\"M 100 200 V 670 H 1136\" fill=\"none\" stroke=\"{muted}\"/>");
    body += &text(64, 200, 16, muted, &format!("{max:.0}"));
    body += &text(64, 670, 16, muted, "0");
    let step = 1010.0 / months.len().max(1) as f64;
    for (i, a) in months.iter().enumerate() {
        let h = a.1 as f64 / max * 440.0;
        body += &format!(
            "<g><title>{}</title>{}</g>",
            escape_xml(&format!("{}: {} commits", a.0, a.1)),
            rect(110.0 + i as f64 * step, 670.0 - h, step * 0.8, h, accent)
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
    if let (Some(first), Some(last)) = (
        cells.iter().map(|c| c.0).min(),
        cells.iter().map(|c| c.0).max(),
    ) {
        let years = (last.year() - first.year() + 1) as f64;
        let band = (520.0 / years).min(180.0);
        let size = (band / 10.0).min(17.0);
        let max = cells.iter().map(|c| c.1).max().unwrap_or(1).max(1) as f64;
        for year in first.year()..=last.year() {
            let y = 210.0 + (year - first.year()) as f64 * band;
            body += &text(64, y as u32, size as u32, muted, &year.to_string());
            let jan = NaiveDate::from_ymd_opt(year, 1, 1).ok_or("invalid calendar year")?;
            for month in 1..=12 {
                let d = NaiveDate::from_ymd_opt(year, month, 1).ok_or("invalid calendar month")?;
                let week = (d.ordinal0() + jan.weekday().num_days_from_monday()) / 7;
                body += &text(
                    140 + week * 18,
                    y as u32,
                    size as u32,
                    muted,
                    &d.format("%b").to_string(),
                );
            }
            let mut date = jan;
            while date.year() == year {
                let week = (date.ordinal0() + jan.weekday().num_days_from_monday()) / 7;
                body += &rect(
                    (140 + week * 18) as f64,
                    y + 14.0 + date.weekday().num_days_from_monday() as f64 * size,
                    15.0,
                    (size - 2.0).max(0.5),
                    empty,
                );
                let Some(next) = date.succ_opt() else { break };
                date = next;
            }
        }
        for (date, count) in cells.iter().filter(|c| c.1 > 0) {
            let jan = NaiveDate::from_ymd_opt(date.year(), 1, 1).ok_or("invalid calendar year")?;
            let week = (date.ordinal0() + jan.weekday().num_days_from_monday()) / 7;
            let y = 224.0
                + (date.year() - first.year()) as f64 * band
                + date.weekday().num_days_from_monday() as f64 * size;
            body += &format!(
                "<g opacity=\"{:.2}\"><title>{date}: {count} commits</title>{}</g>",
                0.4 + *count as f64 / max * 0.6,
                rect(
                    (140 + week * 18) as f64,
                    y,
                    15.0,
                    (size - 2.0).max(0.5),
                    accent
                )
            );
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
    let path = output.join("data.json");
    if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err("refusing symlink data.json".into());
    }
    let mut file =
        fs::File::create(&path).map_err(|e| format!("create {}: {e}", path.display()))?;
    serde_json::to_writer_pretty(&mut file, data)
        .map_err(|e| format!("serialize {}: {e}", path.display()))?;
    file.write_all(b"\n")
        .map_err(|e| format!("write {}: {e}", path.display()))
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
