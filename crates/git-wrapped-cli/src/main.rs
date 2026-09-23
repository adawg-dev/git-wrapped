use clap::{Parser, Subcommand, ValueEnum};
use git_wrapped::{
    analysis::analyze,
    config::Config,
    git::discover,
    render::{render_report, Theme},
};
use std::{
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
    #[arg(long, default_value = "git-wrapped-report", global = true)]
    output: PathBuf,
    #[arg(long, value_enum, default_value_t = ThemeArg::Dark, global = true)]
    theme: ThemeArg,
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
}

#[derive(Clone, Copy, ValueEnum)]
enum ExportFormat {
    Json,
}

fn safe(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn run() -> Result<(), String> {
    let cli = Cli::parse();
    let (repository, export) = match cli.command {
        None => (cli.repository.unwrap_or_else(|| PathBuf::from(".")), false),
        Some(CommandArg::Report { repository }) => {
            if cli.repository.is_some() {
                return Err("repository path cannot precede a subcommand".into());
            }
            (repository.unwrap_or_else(|| PathBuf::from(".")), false)
        }
        Some(CommandArg::Export {
            format: _,
            repository,
        }) => {
            if cli.repository.is_some() {
                return Err("repository path cannot precede a subcommand".into());
            }
            (repository.unwrap_or_else(|| PathBuf::from(".")), true)
        }
    };
    let repo = discover(&repository)?;
    let config = Config::load(&repo.root)?;
    if !export {
        eprintln!("Analyzing Git history...");
    }
    let data = analyze(&repo, &config)?;
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
    } else {
        let theme = match cli.theme {
            ThemeArg::Dark => Theme::Dark,
            ThemeArg::Light => Theme::Light,
        };
        render_report(&data, &cli.output, theme)?;
        println!("Git Wrapped: {}", safe(&data.repository.name));
        println!(
            "{} commit{} · {} contributor{}",
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
            "{} lifetime additions · {} lifetime deletions · {} net historical lines",
            data.repository.additions,
            data.repository.deletions,
            data.repository.net_historical_lines
        );
        println!("Report written to: {}", safe(&cli.output.to_string_lossy()));
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {}", safe(&error));
            ExitCode::FAILURE
        }
    }
}
