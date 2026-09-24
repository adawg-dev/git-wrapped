use clap::{error::ErrorKind, CommandFactory, Parser, Subcommand, ValueEnum};
use git_wrapped::{
    analysis::{
        activity_by_day, activity_by_month, analyze_with_options_and_cancel, AnalysisOptions,
        TimezoneChoice,
    },
    cache,
    config::Config,
    deep::{analyze_deep_with_cancel, DeepLimits},
    git::discover,
    model::RepositoryAnalytics,
    motion::{write_gif, write_mp4_with_cancel, GourceOptions},
    progress::{CancelFlag, Progress},
    render::{render_report_with_options, Theme},
};
use std::{
    collections::BTreeMap,
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
};

#[derive(Parser)]
#[command(
    name = "git-wrapped",
    version,
    about = "Turn local Git history into a shareable report"
)]
struct Cli {
    repository: Option<PathBuf>,
    #[arg(long, value_parser = parse_date, global = true)]
    since: Option<chrono::NaiveDate>,
    #[arg(long, value_parser = parse_date, global = true)]
    until: Option<chrono::NaiveDate>,
    #[arg(long, global = true)]
    author: Vec<String>,
    #[arg(long, global = true)]
    exclude: Vec<String>,
    #[arg(long, global = true)]
    timezone: Option<TimezoneChoice>,
    #[arg(long, global = true)]
    no_merges: bool,
    /// Report directory (default git-wrapped-report); for `animate`, the output file
    #[arg(long, global = true)]
    output: Option<PathBuf>,
    #[arg(long, value_enum, default_value_t = ThemeArg::Dark, global = true)]
    theme: ThemeArg,
    #[arg(long, global = true)]
    no_png: bool,
    #[arg(long, global = true)]
    no_cache: bool,
    #[arg(long, global = true)]
    verbose: bool,
    #[command(subcommand)]
    command: Option<CommandArg>,
}

#[derive(Clone, Copy, ValueEnum)]
enum ThemeArg {
    Dark,
    Light,
}

#[derive(Subcommand)]
enum CommandArg {
    Report {
        #[arg(long)]
        deep: bool,
        repository: Option<PathBuf>,
    },
    Ownership {
        #[arg(long, required = true)]
        deep: bool,
        repository: Option<PathBuf>,
    },
    Export {
        #[arg(long, value_enum, default_value_t = ExportFormat::Json)]
        format: ExportFormat,
        repository: Option<PathBuf>,
    },
    Contributors {
        #[arg(long, value_enum, default_value_t = Metric::Commits)]
        by: Metric,
        repository: Option<PathBuf>,
    },
    Contributor {
        id_or_name: String,
        repository: Option<PathBuf>,
    },
    Activity {
        #[arg(long, value_enum, default_value_t = Bucket::Month)]
        bucket: Bucket,
        repository: Option<PathBuf>,
    },
    Archaeology {
        repository: Option<PathBuf>,
    },
    Awards {
        repository: Option<PathBuf>,
    },
    Top {
        metric: TopMetric,
        repository: Option<PathBuf>,
    },
    /// Write an animated GIF recap or, with Gource and FFmpeg installed, an MP4 history
    Animate {
        #[arg(long, value_enum, default_value_t = AnimateFormat::Gif)]
        format: AnimateFormat,
        /// MP4 only: Gource simulation seconds per day of history
        #[arg(long, default_value_t = 0.1)]
        seconds_per_day: f32,
        /// MP4 only: hide file names in the Gource view
        #[arg(long)]
        hide_filenames: bool,
        repository: Option<PathBuf>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum AnimateFormat {
    Gif,
    Mp4,
}

#[derive(Clone, Copy, ValueEnum)]
enum ExportFormat {
    Json,
}

#[derive(Clone, Copy, ValueEnum)]
enum Metric {
    Commits,
    Additions,
    Deletions,
    Churn,
    Net,
    Files,
    ActiveDays,
}

impl Metric {
    fn name(self) -> &'static str {
        match self {
            Self::Commits => "commits",
            Self::Additions => "additions",
            Self::Deletions => "deletions",
            Self::Churn => "churn",
            Self::Net => "net",
            Self::Files => "files",
            Self::ActiveDays => "active-days",
        }
    }

    fn value(self, c: &git_wrapped::model::ContributorAnalytics) -> i128 {
        match self {
            Self::Commits => i128::from(c.commits),
            Self::Additions => i128::from(c.additions),
            Self::Deletions => i128::from(c.deletions),
            Self::Churn => i128::from(c.churn),
            Self::Net => i128::from(c.net),
            Self::Files => c.files_touched as i128,
            Self::ActiveDays => c.active_days as i128,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum TopMetric {
    Commits,
    Additions,
    Deletions,
    Churn,
    Files,
}

impl From<TopMetric> for Metric {
    fn from(value: TopMetric) -> Self {
        match value {
            TopMetric::Commits => Self::Commits,
            TopMetric::Additions => Self::Additions,
            TopMetric::Deletions => Self::Deletions,
            TopMetric::Churn => Self::Churn,
            TopMetric::Files => Self::Files,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum Bucket {
    Day,
    Week,
    Month,
    Quarter,
    Year,
}

enum View {
    Contributors(Metric),
    Contributor(String),
    Activity(Bucket),
    Archaeology,
    Awards,
    Ownership,
}

fn safe(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn parse_date(value: &str) -> Result<chrono::NaiveDate, String> {
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| format!("invalid date '{value}'; use YYYY-MM-DD"))
}

fn parse() -> Result<Cli, clap::Error> {
    let cli = Cli::try_parse()?;
    if cli.repository.is_some() && cli.command.is_some() {
        return Err(Cli::command().error(
            ErrorKind::ArgumentConflict,
            "repository path cannot precede a subcommand",
        ));
    }
    Ok(cli)
}

fn print_view(data: &RepositoryAnalytics, view: View, out: &mut impl Write) -> Result<(), String> {
    match view {
        View::Contributors(metric) => {
            let mut rows: Vec<_> = data.contributors.iter().collect();
            rows.sort_by(|a, b| {
                metric
                    .value(b)
                    .cmp(&metric.value(a))
                    .then_with(|| a.id.cmp(&b.id))
            });
            writeln!(out, "name\tid\t{}", metric.name()).map_err(|e| e.to_string())?;
            for c in rows.iter().take(20) {
                writeln!(
                    out,
                    "{}\t{}\t{}",
                    safe(&c.name),
                    safe(&c.id),
                    metric.value(c)
                )
                .map_err(|e| e.to_string())?;
            }
            writeln!(out, "showing {} of {}", rows.len().min(20), rows.len())
                .map_err(|e| e.to_string())?;
        }
        View::Contributor(query) => {
            let found = data.contributors.iter().find(|c| c.id == query);
            let c = match found {
                Some(c) => c,
                None => {
                    let matches: Vec<_> = data
                        .contributors
                        .iter()
                        .filter(|c| c.name.to_lowercase() == query.to_lowercase())
                        .collect();
                    match matches.as_slice() {
                        [c] => *c,
                        [] => return Err(format!("contributor '{}' not found", safe(&query))),
                        _ => {
                            return Err(format!(
                                "contributor '{}' is ambiguous; use an ID: {}",
                                safe(&query),
                                matches
                                    .iter()
                                    .map(|c| safe(&c.id))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ))
                        }
                    }
                }
            };
            writeln!(out, "{}\t{}", safe(&c.name), safe(&c.id)).map_err(|e| e.to_string())?;
            writeln!(out, "commits\t{}", c.commits).map_err(|e| e.to_string())?;
            writeln!(out, "additions\t{}", c.additions).map_err(|e| e.to_string())?;
            writeln!(out, "deletions\t{}", c.deletions).map_err(|e| e.to_string())?;
            writeln!(out, "churn\t{}", c.churn).map_err(|e| e.to_string())?;
            writeln!(out, "net historical lines\t{}", c.net).map_err(|e| e.to_string())?;
            writeln!(out, "files touched\t{}", c.files_touched).map_err(|e| e.to_string())?;
            writeln!(out, "active days\t{}", c.active_days).map_err(|e| e.to_string())?;
            writeln!(out, "first contribution\t{}", safe(&c.first_contribution))
                .map_err(|e| e.to_string())?;
            writeln!(out, "latest contribution\t{}", safe(&c.latest_contribution))
                .map_err(|e| e.to_string())?;
        }
        View::Activity(bucket) => {
            let rows: Vec<(String, u64)> = match bucket {
                Bucket::Day => activity_by_day(data)?
                    .into_iter()
                    .map(|c| (c.date, c.commits))
                    .collect(),
                Bucket::Week => data
                    .activity_by_week
                    .iter()
                    .map(|c| (c.date.clone(), c.commits))
                    .collect(),
                Bucket::Month => activity_by_month(data)?
                    .into_iter()
                    .map(|c| (c.month, c.commits))
                    .collect(),
                Bucket::Quarter | Bucket::Year => {
                    let width = if matches!(bucket, Bucket::Quarter) {
                        4
                    } else {
                        1
                    };
                    let mut counts = BTreeMap::<i32, u64>::new();
                    for month in &data.activity {
                        let date = chrono::NaiveDate::parse_from_str(
                            &format!("{}-01", month.month),
                            "%Y-%m-%d",
                        )
                        .map_err(|e| e.to_string())?;
                        use chrono::Datelike;
                        let index = date.year() * width + (date.month0() as i32 / (12 / width));
                        let count = counts.entry(index).or_default();
                        *count = count
                            .checked_add(month.commits)
                            .ok_or("activity count overflow")?;
                    }
                    let (Some(&first), Some(&last)) =
                        (counts.keys().next(), counts.keys().next_back())
                    else {
                        return Ok(());
                    };
                    if i64::from(last) - i64::from(first) + 1 > 20_000 {
                        return Err("activity exceeds 20,000 buckets; use a coarser period".into());
                    }
                    (first..=last)
                        .map(|index| {
                            let label = if width == 4 {
                                format!("{}-Q{}", index.div_euclid(4), index.rem_euclid(4) + 1)
                            } else {
                                index.to_string()
                            };
                            (label, counts.get(&index).copied().unwrap_or(0))
                        })
                        .collect()
                }
            };
            writeln!(out, "period\tcommits").map_err(|e| e.to_string())?;
            let start = rows.len().saturating_sub(20);
            for (label, commits) in &rows[start..] {
                writeln!(out, "{}\t{}", safe(label), commits).map_err(|e| e.to_string())?;
            }
            writeln!(out, "showing {} of {}", rows.len().min(20), rows.len())
                .map_err(|e| e.to_string())?;
        }
        View::Archaeology => {
            let mut rows: Vec<_> = data.files.iter().collect();
            rows.sort_by(|a, b| {
                b.churn
                    .cmp(&a.churn)
                    .then_with(|| a.path_id.cmp(&b.path_id))
            });
            writeln!(
                out,
                "path\tid\trevisions\tchurn\tstatus\tfirst change\tlatest change"
            )
            .map_err(|e| e.to_string())?;
            for file in rows.iter().take(20) {
                writeln!(
                    out,
                    "{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    safe(&file.display_path),
                    safe(&file.path_id),
                    file.revisions,
                    file.churn,
                    if file.exists_at_head {
                        "current"
                    } else {
                        "historical"
                    },
                    safe(&file.first_change),
                    safe(&file.latest_change)
                )
                .map_err(|e| e.to_string())?;
            }
            writeln!(out, "showing {} of {}", rows.len().min(20), rows.len())
                .map_err(|e| e.to_string())?;
        }
        View::Awards => {
            writeln!(out, "award\twinner\tvalue\texplanation").map_err(|e| e.to_string())?;
            for award in data.awards.iter().take(20) {
                writeln!(
                    out,
                    "{}\t{}\t{}\t{}",
                    safe(&award.title),
                    safe(&award.winner),
                    safe(&award.value),
                    safe(&award.explanation)
                )
                .map_err(|e| e.to_string())?;
            }
            writeln!(
                out,
                "showing {} of {}",
                data.awards.len().min(20),
                data.awards.len()
            )
            .map_err(|e| e.to_string())?;
        }
        View::Ownership => {
            let deep = data.deep.as_ref().ok_or("ownership requires --deep")?;
            writeln!(
                out,
                "Current HEAD nonblank text lines by author (date/author filters do not apply)"
            )
            .map_err(|e| e.to_string())?;
            writeln!(out, "author id\tlines\tpercent").map_err(|e| e.to_string())?;
            for row in &deep.ownership {
                writeln!(
                    out,
                    "{}\t{}\t{:.1}%",
                    safe(&row.author_id),
                    row.lines,
                    row.percent
                )
                .map_err(|e| e.to_string())?;
            }
            writeln!(out, "coverage\t{} attributed, {} unknown; {} of {} eligible files analyzed; {} binary and {} submodules skipped{}",
                deep.coverage.attributed_lines, deep.coverage.unknown_lines,
                deep.coverage.analyzed_files, deep.coverage.eligible_files,
                deep.coverage.skipped_binary, deep.coverage.skipped_submodules,
                if deep.coverage.truncated { "; truncated" } else { "" })
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn run(cli: Cli) -> Result<(), String> {
    let cancel = CancelFlag::default();
    cancel.install_ctrlc()?;
    let progress = Progress::new(cli.verbose);
    let theme = match cli.theme {
        ThemeArg::Dark => Theme::Dark,
        ThemeArg::Light => Theme::Light,
    };
    let animate = match &cli.command {
        Some(CommandArg::Animate {
            format,
            seconds_per_day,
            hide_filenames,
            ..
        }) => Some((
            *format,
            GourceOptions {
                seconds_per_day: *seconds_per_day,
                hide_filenames: *hide_filenames,
                ..GourceOptions::default()
            },
        )),
        _ => None,
    };
    let (repository, export, view, deep) = match cli.command {
        None => (
            cli.repository.unwrap_or_else(|| PathBuf::from(".")),
            false,
            None,
            false,
        ),
        Some(CommandArg::Report { repository, deep }) => (
            repository.unwrap_or_else(|| PathBuf::from(".")),
            false,
            None,
            deep,
        ),
        Some(CommandArg::Ownership { repository, deep }) => (
            repository.unwrap_or_else(|| PathBuf::from(".")),
            false,
            Some(View::Ownership),
            deep,
        ),
        Some(CommandArg::Export {
            format: _,
            repository,
        }) => (
            repository.unwrap_or_else(|| PathBuf::from(".")),
            true,
            None,
            false,
        ),
        Some(CommandArg::Contributors { by, repository }) => (
            repository.unwrap_or_else(|| PathBuf::from(".")),
            false,
            Some(View::Contributors(by)),
            false,
        ),
        Some(CommandArg::Contributor {
            id_or_name,
            repository,
        }) => (
            repository.unwrap_or_else(|| PathBuf::from(".")),
            false,
            Some(View::Contributor(id_or_name)),
            false,
        ),
        Some(CommandArg::Activity { bucket, repository }) => (
            repository.unwrap_or_else(|| PathBuf::from(".")),
            false,
            Some(View::Activity(bucket)),
            false,
        ),
        Some(CommandArg::Archaeology { repository }) => (
            repository.unwrap_or_else(|| PathBuf::from(".")),
            false,
            Some(View::Archaeology),
            false,
        ),
        Some(CommandArg::Awards { repository }) => (
            repository.unwrap_or_else(|| PathBuf::from(".")),
            false,
            Some(View::Awards),
            false,
        ),
        Some(CommandArg::Top { metric, repository }) => (
            repository.unwrap_or_else(|| PathBuf::from(".")),
            false,
            Some(View::Contributors(metric.into())),
            false,
        ),
        Some(CommandArg::Animate { repository, .. }) => (
            repository.unwrap_or_else(|| PathBuf::from(".")),
            false,
            None,
            false,
        ),
    };
    let repo = discover(&repository)?;
    let config = Config::load(&repo.root)?;
    let timezone = match cli.timezone {
        Some(zone) => zone,
        None => config
            .timezone
            .as_deref()
            .map(str::parse::<TimezoneChoice>)
            .transpose()
            .map_err(|error| format!("{}: {error}", repo.root.join(".git-wrapped.json").display()))?
            .unwrap_or_default(),
    };
    let options = AnalysisOptions {
        since: cli.since,
        until: cli.until,
        author_ids: cli.author,
        exclusions: config
            .exclude
            .iter()
            .chain(cli.exclude.iter())
            .cloned()
            .collect(),
        timezone,
        include_merges: !cli.no_merges,
    };
    if let Some((format, gource)) = animate {
        let output = cli.output.unwrap_or_else(|| {
            PathBuf::from("git-wrapped-report").join(match format {
                AnimateFormat::Gif => "story.gif",
                AnimateFormat::Mp4 => "repo-history.mp4",
            })
        });
        match format {
            AnimateFormat::Gif => {
                progress.phase("Scanning Git history", None, None);
                let data = analyze_with_options_and_cancel(&repo, &config, &options, &cancel)?;
                progress.phase("Rendering GIF story", None, None);
                write_gif(&data, &output, theme)?;
            }
            AnimateFormat::Mp4 => {
                progress.phase("Rendering Gource history", None, None);
                write_mp4_with_cancel(&repo, &config, &options, &output, gource, &cancel)?;
            }
        }
        cancel.check()?;
        println!("Animation written to: {}", safe(&output.to_string_lossy()));
        return Ok(());
    }
    let output = cli
        .output
        .unwrap_or_else(|| PathBuf::from("git-wrapped-report"));
    let cache_path = output.join(".git-wrapped-cache.json");
    let key = if !export && (view.is_none() || deep) && !cli.no_cache {
        match cache::key(&repo, &config, &options, deep) {
            Ok(key) => Some(key),
            Err(error) => {
                eprintln!("Warning: could not key analysis cache: {}", safe(&error));
                None
            }
        }
    } else {
        None
    };
    let cached = key
        .as_ref()
        .and_then(|expected| cache::load(&cache_path, expected).ok().flatten())
        .filter(|data| !deep || data.deep.is_some());
    let cache_hit = cached.is_some();
    let mut data = if let Some(data) = cached {
        eprintln!("Using cached analysis");
        data
    } else {
        progress.phase("Scanning Git history", None, None);
        analyze_with_options_and_cancel(&repo, &config, &options, &cancel)?
    };
    if deep && !cache_hit {
        progress.phase("Measuring current ownership", None, None);
        analyze_deep_with_cancel(&repo, &config, &mut data, DeepLimits::default(), &cancel)?;
    }
    cancel.check()?;
    if repo.shallow {
        eprintln!("Warning: shallow repository; historical totals cover available history only.");
    }
    if export {
        let stdout = io::stdout();
        let mut output = stdout.lock();
        serde_json::to_writer_pretty(&mut output, &data).map_err(|e| format!("write JSON: {e}"))?;
        output
            .write_all(b"\n")
            .map_err(|e| format!("write JSON: {e}"))?;
    } else if let Some(view) = view {
        let stdout = io::stdout();
        print_view(&data, view, &mut stdout.lock())?;
        if let Some(key) = key.as_ref().filter(|_| !cache_hit) {
            if let Err(error) = cache::save(&cache_path, key, &data) {
                eprintln!("Warning: could not save analysis cache: {}", safe(&error));
            }
        }
    } else {
        progress.phase("Writing report", None, None);
        render_report_with_options(&data, &output, theme, !cli.no_png)?;
        if let Some(key) = key.as_ref().filter(|_| !cache_hit) {
            if let Err(error) = cache::save(&cache_path, key, &data) {
                eprintln!("Warning: could not save analysis cache: {}", safe(&error));
            }
        }
        println!("Git Wrapped: {}", safe(&data.repository.name));
        let selection = data.repository.selection_labels();
        let selected = !selection.is_empty();
        if selected {
            println!("Selection: {}", safe(&selection.join(" · ")));
        }
        println!(
            "{}{} commit{} · {} contributor{}",
            if selected { "Selected " } else { "" },
            data.repository.total_commits,
            if data.repository.total_commits == 1 {
                ""
            } else {
                "s"
            },
            data.repository.total_contributors,
            if data.repository.total_contributors == 1 {
                ""
            } else {
                "s"
            }
        );
        println!(
            "{} {} additions · {} {} deletions · {} net {} lines",
            data.repository.additions,
            if selected { "selected" } else { "lifetime" },
            data.repository.deletions,
            if selected { "selected" } else { "lifetime" },
            data.repository.net_historical_lines,
            if selected { "selected" } else { "historical" }
        );
        let mut contributors: Vec<_> = data.contributors.iter().collect();
        contributors.sort_by(|a, b| b.commits.cmp(&a.commits).then_with(|| a.id.cmp(&b.id)));
        println!("Top contributors:");
        for c in contributors.iter().take(3) {
            println!(
                "  {} ({}): {} commit{}",
                safe(&c.name),
                safe(&c.id),
                c.commits,
                if c.commits == 1 { "" } else { "s" }
            );
        }
        if let Some(peak) = &data.insights.busiest_day {
            println!("Peak day: {} ({} commits)", safe(&peak.label), peak.count);
        }
        println!("Awards:");
        for award in data.awards.iter().take(3) {
            println!("  {}: {}", safe(&award.title), safe(&award.winner));
        }
        println!("Report written to: {}", safe(&output.to_string_lossy()));
    }
    Ok(())
}

fn main() -> ExitCode {
    let cli = match parse() {
        Ok(cli) => cli,
        Err(error) => {
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) {
                print!("{error}");
            } else {
                eprintln!("{}", safe(&error.to_string()));
            }
            return ExitCode::from(error.exit_code() as u8);
        }
    };
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {}", safe(&error));
            if error == "cancelled" {
                ExitCode::from(130)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}
