use crate::{
    model::RepositoryAnalytics,
    render::{monthly_counts, rect, text, Palette, Theme},
};

pub const SIZE: u32 = 720;
pub const FRAMES_PER_SCENE: u32 = 7;
const TRANSITION_MS: u32 = 100;
const HOLD_MS: u32 = 1800;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoryPhase {
    Intro,
    Growth,
    People,
    Rhythm,
    Paths,
    Awards,
    Outro,
}

const PHASES: [StoryPhase; 7] = [
    StoryPhase::Intro,
    StoryPhase::Growth,
    StoryPhase::People,
    StoryPhase::Rhythm,
    StoryPhase::Paths,
    StoryPhase::Awards,
    StoryPhase::Outro,
];

pub struct Scene<'a> {
    pub phase: StoryPhase,
    pub data: &'a RepositoryAnalytics,
    pub theme: Theme,
    pub delay_ms: u32,
}

/// The fixed seven-scene story; order, timing, and colors never depend on data.
pub fn scenes(data: &RepositoryAnalytics, theme: Theme) -> Vec<Scene<'_>> {
    PHASES
        .iter()
        .map(|&phase| Scene {
            phase,
            data,
            theme,
            delay_ms: TRANSITION_MS,
        })
        .collect()
}

// Bundled Lato ASCII glyphs stay under 0.6 em; other scalars count as a full em.
fn fits(value: &str, width: u32, size: u32) -> bool {
    let em: f64 = value
        .chars()
        .map(|c| if c.is_ascii() { 0.6 } else { 1.0 })
        .sum();
    em * f64::from(size) <= f64::from(width)
}

fn fit_text(value: &str, width: u32, size: u32) -> String {
    if fits(value, width, size) {
        return value.to_owned();
    }
    let mut shown: Vec<char> = value.chars().collect();
    while !shown.is_empty() && !fits(&format!("{}…", String::from_iter(&shown)), width, size) {
        shown.pop();
    }
    format!("{}…", String::from_iter(shown))
}

/// Paths keep their file name: the head is elided instead of the tail.
fn fit_path(value: &str, width: u32, size: u32) -> String {
    if fits(value, width, size) {
        return value.to_owned();
    }
    let mut shown: std::collections::VecDeque<char> = value.chars().collect();
    while !shown.is_empty() && !fits(&format!("…{}", String::from_iter(&shown)), width, size) {
        shown.pop_front();
    }
    format!("…{}", String::from_iter(shown))
}

fn scaled(value: u64, t: f64) -> u64 {
    (value as f64 * t).round() as u64
}

fn share(value: u64, max: u64) -> f64 {
    if max == 0 {
        0.0
    } else {
        value as f64 / max as f64
    }
}

impl Scene<'_> {
    /// Transition frames advance quickly; the settled last frame holds.
    pub fn frame_delay_ms(&self, frame_index: u32, frame_count: u32) -> u32 {
        if frame_index + 1 >= frame_count {
            HOLD_MS
        } else {
            self.delay_ms
        }
    }

    /// A complete SVG for one frame; progress eases from first to last frame.
    pub fn at(&self, frame_index: u32, frame_count: u32) -> String {
        let linear = if frame_count == 0 {
            1.0
        } else {
            (f64::from(frame_index) + 1.0) / f64::from(frame_count)
        }
        .clamp(0.0, 1.0);
        let t = 1.0 - (1.0 - linear).powi(3);
        let p = self.theme.palette();
        let mut body = text(48, 64, 18, p.accent, "GIT WRAPPED");
        body += &match self.phase {
            StoryPhase::Intro => self.intro(p, t),
            StoryPhase::Growth => self.growth(p, t),
            StoryPhase::People => self.people(p, t),
            StoryPhase::Rhythm => self.rhythm(p, t),
            StoryPhase::Paths => self.paths(p, t),
            StoryPhase::Awards => self.awards(p, t),
            StoryPhase::Outro => self.outro(p, t),
        };
        let current = PHASES.iter().position(|&phase| phase == self.phase);
        for index in 0..PHASES.len() {
            let color = if Some(index) == current {
                p.accent
            } else {
                p.muted
            };
            body += &rect(302.0 + index as f64 * 18.0, 676.0, 10.0, 10.0, color);
        }
        format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{SIZE}\" height=\"{SIZE}\" viewBox=\"0 0 {SIZE} {SIZE}\"><rect width=\"100%\" height=\"100%\" fill=\"{}\"/>{body}</svg>\n", p.bg)
    }

    fn heading(&self, p: Palette, title: &str, subtitle: &str) -> String {
        text(48, 116, 38, p.fg, title) + &text(48, 150, 17, p.muted, &fit_text(subtitle, 624, 17))
    }

    fn intro(&self, p: Palette, t: f64) -> String {
        let r = &self.data.repository;
        let mut body = text(48, 150, 44, p.fg, &fit_text(&r.name, 624, 44));
        body += &text(
            48,
            190,
            18,
            p.muted,
            &format!(
                "{} — {} · {} days",
                r.first_commit.split('T').next().unwrap_or(""),
                r.latest_commit.split('T').next().unwrap_or(""),
                r.age_days
            ),
        );
        let selection = r.selection_labels();
        if !selection.is_empty() {
            body += &text(
                48,
                216,
                16,
                p.muted,
                &fit_text(&selection.join(" · "), 624, 16),
            );
        }
        body += &text(48, 380, 110, p.fg, &scaled(r.total_commits, t).to_string());
        body += &text(
            48,
            420,
            24,
            p.muted,
            if r.total_commits == 1 {
                "commit"
            } else {
                "commits"
            },
        );
        body += &text(
            48,
            520,
            48,
            p.secondary,
            &scaled(r.total_contributors as u64, t).to_string(),
        );
        body += &text(
            48,
            552,
            20,
            p.muted,
            if r.total_contributors == 1 {
                "contributor"
            } else {
                "contributors"
            },
        );
        body
    }

    fn growth(&self, p: Palette, t: f64) -> String {
        let months = monthly_counts(self.data);
        // Equal-width buckets keep bar heights comparable; only the last may be shorter.
        let months_per_bar = months.len().div_ceil(12).max(1);
        let subtitle = if months_per_bar == 1 {
            "Commits per calendar month".to_owned()
        } else {
            format!("Commits per {months_per_bar} calendar months · last bar may be partial")
        };
        let mut body = self.heading(p, "Growth", &subtitle);
        if months.is_empty() {
            return body + &text(48, 360, 22, p.muted, "No monthly activity to chart");
        }
        let buckets: Vec<u64> = months
            .chunks(months_per_bar)
            .map(|chunk| {
                chunk
                    .iter()
                    .fold(0_u64, |sum, (_, c)| sum.saturating_add(*c))
            })
            .collect();
        let count = buckets.len();
        let max = buckets.iter().copied().max().unwrap_or(0);
        let step = 624.0 / count as f64;
        let width = (step * 0.7).min(72.0);
        for (index, commits) in buckets.iter().enumerate() {
            let h = share(*commits, max) * 360.0 * t;
            let x = 48.0 + index as f64 * step + (step - width) / 2.0;
            body += &rect(x, 580.0 - h, width, h, p.accent);
            body += &text(
                x as u32,
                (570.0 - h) as u32,
                16,
                p.muted,
                &scaled(*commits, t).to_string(),
            );
        }
        let (first, last) = (&months[0].0, &months[months.len() - 1].0);
        let span = if first == last {
            first.clone()
        } else {
            format!("{first} → {last}")
        };
        body += &text(48, 616, 17, p.muted, &span);
        body
    }

    fn people(&self, p: Palette, t: f64) -> String {
        let mut body = self.heading(p, "People", "Top contributors by commits · after mailmap");
        let mut people: Vec<_> = self.data.contributors.iter().collect();
        people.sort_by(|a, b| b.commits.cmp(&a.commits).then_with(|| a.id.cmp(&b.id)));
        if people.is_empty() {
            return body + &text(48, 360, 22, p.muted, "No contributors to chart");
        }
        let max = people[0].commits;
        let colors = [p.accent, p.secondary, p.positive, p.negative, p.muted];
        for (index, person) in people.iter().take(5).enumerate() {
            let y = 210 + index as u32 * 84;
            body += &text(48, y, 22, p.fg, &fit_text(&person.name, 624, 22));
            let width = share(person.commits, max) * 520.0 * t;
            body += &rect(48.0, f64::from(y) + 14.0, width, 26.0, colors[index]);
            body += &text(
                (48.0 + width) as u32 + 12,
                y + 35,
                18,
                p.muted,
                &scaled(person.commits, t).to_string(),
            );
        }
        if people.len() > 5 {
            body += &text(
                48,
                640,
                16,
                p.muted,
                &format!("Top 5 of {} shown", people.len()),
            );
        }
        body
    }

    fn rhythm(&self, p: Palette, t: f64) -> String {
        let mut body = self.heading(p, "Rhythm", "Commits by author local hour · 00–23");
        let mut hours = [0_u64; 24];
        for person in &self.data.contributors {
            for (hour, value) in person.commits_by_hour.iter().enumerate() {
                hours[hour] = hours[hour].saturating_add(*value);
            }
        }
        let max = hours.iter().copied().max().unwrap_or(0);
        if max == 0 {
            return body + &text(48, 360, 22, p.muted, "No hourly activity to chart");
        }
        for (hour, value) in hours.iter().enumerate() {
            let h = share(*value, max) * 340.0 * t;
            body += &rect(48.0 + hour as f64 * 26.0, 560.0 - h, 18.0, h, p.secondary);
        }
        for hour in [0_u32, 6, 12, 18, 23] {
            body += &text(48 + hour * 26, 588, 16, p.muted, &format!("{hour:02}"));
        }
        if let Some(peak) = &self.data.insights.peak_hour {
            body += &text(
                48,
                636,
                20,
                p.fg,
                &fit_text(&format!("Peak hour: {}:00", peak.label), 624, 20),
            );
        }
        body
    }

    fn paths(&self, p: Palette, t: f64) -> String {
        let mut body = self.heading(p, "Paths", "Most changed files · historical line churn");
        let mut files: Vec<_> = self.data.files.iter().collect();
        files.sort_by(|a, b| {
            b.churn
                .cmp(&a.churn)
                .then_with(|| a.path_id.cmp(&b.path_id))
        });
        if files.is_empty() {
            return body + &text(48, 360, 22, p.muted, "No file changes to chart");
        }
        let max = files[0].churn;
        for (index, file) in files.iter().take(5).enumerate() {
            let y = 210 + index as u32 * 84;
            body += &text(48, y, 19, p.fg, &fit_path(&file.display_path, 624, 19));
            body += &rect(
                48.0,
                f64::from(y) + 12.0,
                share(file.churn, max) * 624.0 * t,
                16.0,
                p.accent,
            );
            body += &text(
                48,
                y + 50,
                16,
                p.muted,
                &format!(
                    "{} revisions · {} churn",
                    scaled(file.revisions, t),
                    scaled(file.churn, t)
                ),
            );
        }
        body
    }

    fn awards(&self, p: Palette, t: f64) -> String {
        let mut body = self.heading(p, "Awards", "Deterministic winners from selected history");
        if self.data.awards.is_empty() {
            return body + &text(48, 360, 22, p.muted, "No eligible winners");
        }
        for (index, award) in self.data.awards.iter().take(3).enumerate() {
            let opacity = (t * 3.0 - index as f64).clamp(0.0, 1.0);
            let y = 220 + index as u32 * 140;
            body += &format!("<g opacity=\"{opacity:.3}\">");
            body += &text(48, y, 18, p.muted, &fit_text(&award.title, 624, 18));
            body += &text(48, y + 40, 32, p.fg, &fit_text(&award.winner, 624, 32));
            body += &text(
                48,
                y + 72,
                18,
                p.secondary,
                &fit_text(&format!("{} · {}", award.value, award.metric), 624, 18),
            );
            body += "</g>";
        }
        body
    }

    fn outro(&self, p: Palette, t: f64) -> String {
        let r = &self.data.repository;
        let mut body = text(48, 150, 44, p.fg, "That's a wrap");
        body += &text(48, 190, 18, p.muted, &fit_text(&r.name, 624, 18));
        for (index, (value, label, color)) in [
            (
                format!("+{}", scaled(r.additions, t)),
                "lines added",
                p.positive,
            ),
            (
                format!("−{}", scaled(r.deletions, t)),
                "lines deleted",
                p.negative,
            ),
            (
                scaled(r.tracked_files as u64, t).to_string(),
                "tracked files at HEAD",
                p.secondary,
            ),
        ]
        .iter()
        .enumerate()
        {
            let y = 300 + index as u32 * 110;
            body += &text(48, y, 56, color, value);
            body += &text(48, y + 32, 20, p.muted, label);
        }
        body + &text(
            48,
            640,
            16,
            p.muted,
            "Made with git-wrapped · static poster: summary.svg",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fitting_elides_the_right_end() {
        assert_eq!(fit_text("short", 624, 20), "short");
        let long = "x".repeat(100);
        let fitted = fit_text(&long, 120, 20);
        assert!(fitted.ends_with('…') && fits(&fitted, 120, 20));
        let path = format!("{}/report.rs", "dir/".repeat(40));
        let fitted = fit_path(&path, 300, 20);
        assert!(fitted.starts_with('…') && fitted.ends_with("/report.rs"));
        assert!(fits(&fitted, 300, 20));
    }
}
