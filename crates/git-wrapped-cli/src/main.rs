use clap::{error::ErrorKind, CommandFactory, Parser, Subcommand, ValueEnum};
use git_wrapped::{
    analysis::{
        activity_by_day, activity_by_month, analyze_with_options, AnalysisOptions, TimezoneChoice,
    },
    config::Config,
    git::discover,
    model::RepositoryAnalytics,
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
    timezone: Option<TimezoneChoice>,
    #[arg(long, global = true)]
    no_merges: bool,
    #[arg(long, default_value = "git-wrapped-report", global = true)]
    output: PathBuf,
    #[arg(long, value_enum, default_value_t = ThemeArg::Dark, global = true)]
    theme: ThemeArg,
    #[arg(long, global = true)]
    no_png: bool,
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
    }
    Ok(())
}

fn run(cli: Cli) -> Result<(), String> {
    let (repository, export, view) = match cli.command {
        None => (
            cli.repository.unwrap_or_else(|| PathBuf::from(".")),
            false,
            None,
        ),
        Some(CommandArg::Report { repository }) => (
            repository.unwrap_or_else(|| PathBuf::from(".")),
            false,
            None,
        ),
        Some(CommandArg::Export {
            format: _,
            repository,
        }) => (repository.unwrap_or_else(|| PathBuf::from(".")), true, None),
        Some(CommandArg::Contributors { by, repository }) => (
            repository.unwrap_or_else(|| PathBuf::from(".")),
            false,
            Some(View::Contributors(by)),
        ),
        Some(CommandArg::Contributor {
            id_or_name,
            repository,
        }) => (
            repository.unwrap_or_else(|| PathBuf::from(".")),
            false,
            Some(View::Contributor(id_or_name)),
        ),
        Some(CommandArg::Activity { bucket, repository }) => (
            repository.unwrap_or_else(|| PathBuf::from(".")),
            false,
            Some(View::Activity(bucket)),
        ),
        Some(CommandArg::Archaeology { repository }) => (
            repository.unwrap_or_else(|| PathBuf::from(".")),
            false,
            Some(View::Archaeology),
        ),
        Some(CommandArg::Awards { repository }) => (
            repository.unwrap_or_else(|| PathBuf::from(".")),
            false,
            Some(View::Awards),
        ),
        Some(CommandArg::Top { metric, repository }) => (
            repository.unwrap_or_else(|| PathBuf::from(".")),
            false,
            Some(View::Contributors(metric.into())),
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
        timezone,
        include_merges: !cli.no_merges,
        ..Default::default()
    };
    if !export {
        eprintln!("Analyzing Git history...");
    }
    let data = analyze_with_options(&repo, &config, &options)?;
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
    } else {
        let theme = match cli.theme {
            ThemeArg::Dark => Theme::Dark,
            ThemeArg::Light => Theme::Light,
        };
        render_report_with_options(&data, &cli.output, theme, !cli.no_png)?;
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
        println!("Report written to: {}", safe(&cli.output.to_string_lossy()));
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
            ExitCode::FAILURE
        }
    }
}
