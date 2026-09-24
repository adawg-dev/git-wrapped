use crate::{
    analysis::{select_commit, AnalysisOptions, Selection},
    config::Config,
    git::{scan_with_cancel, Repository},
    progress::CancelFlag,
};
use std::{
    ffi::OsString,
    fs,
    io::{BufWriter, ErrorKind, Write},
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::Duration,
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
    write_custom_log_with_cancel(repo, config, filters, path, &CancelFlag::default())
}

pub(crate) fn write_custom_log_with_cancel(
    repo: &Repository,
    config: &Config,
    filters: &AnalysisOptions,
    path: &Path,
    cancel: &CancelFlag,
) -> Result<LogStats, String> {
    let selection = Selection::new(repo, config, filters)?;
    let mut events = Vec::new();
    let mut commits = 0;
    scan_with_cancel(repo, cancel, |raw| {
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

#[derive(Clone, Copy, Debug)]
pub struct GourceOptions {
    pub seconds_per_day: f32,
    pub hide_filenames: bool,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

impl Default for GourceOptions {
    fn default() -> Self {
        Self {
            seconds_per_day: 0.1,
            hide_filenames: false,
            width: 1280,
            height: 720,
            fps: 30,
        }
    }
}

impl GourceOptions {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if !(320..=3840).contains(&self.width) || !(320..=3840).contains(&self.height) {
            return Err("video width and height must be between 320 and 3840".into());
        }
        if !(1..=60).contains(&self.fps) {
            return Err("video frame rate must be between 1 and 60".into());
        }
        if !(self.seconds_per_day > 0.0 && self.seconds_per_day <= 1000.0) {
            return Err("--seconds-per-day must be greater than 0 and at most 1000".into());
        }
        Ok(())
    }
}

/// Confirm an optional tool runs before any output is produced.
pub(crate) fn require_tool(program: &str, version_flag: &str, name: &str) -> Result<(), String> {
    let status = Command::new(program)
        .arg(version_flag)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match status {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("{name} is not usable: `{program} {version_flag}` exited with {status}")),
        Err(error) if error.kind() == ErrorKind::NotFound => Err(format!(
            "{name} is not installed; --format mp4 needs both Gource (https://gource.io) and FFmpeg (https://ffmpeg.org) on PATH"
        )),
        Err(error) => Err(format!("cannot run {name}: {error}")),
    }
}

/// A child that is killed and reaped whenever it goes out of scope unfinished.
struct Reaped {
    child: Child,
    name: &'static str,
    status: Option<ExitStatus>,
}

impl Reaped {
    fn poll(&mut self) -> Result<Option<ExitStatus>, String> {
        if self.status.is_none() {
            self.status = self
                .child
                .try_wait()
                .map_err(|e| format!("wait for {}: {e}", self.name))?;
        }
        Ok(self.status)
    }
}

impl Drop for Reaped {
    fn drop(&mut self) {
        if self.status.is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// Pipe Gource's PPM stream into FFmpeg without a shell. Any failure or cancellation
/// kills and reaps both children; the caller owns cleanup of `video`.
pub(crate) fn run_pipeline(
    log: &Path,
    video: &Path,
    options: GourceOptions,
    cancel: &CancelFlag,
) -> Result<(), String> {
    let mut gource_args: Vec<OsString> = vec![
        "--log-format".into(),
        "custom".into(),
        format!("-{}x{}", options.width, options.height).into(),
        "--output-ppm-stream".into(),
        "-".into(),
        "--output-framerate".into(),
        options.fps.to_string().into(),
        "--seconds-per-day".into(),
        options.seconds_per_day.to_string().into(),
        "--stop-at-end".into(),
    ];
    if options.hide_filenames {
        gource_args.extend(["--hide".into(), "filenames".into()]);
    }
    gource_args.push(log.into());
    let mut gource = Reaped {
        child: Command::new("gource")
            .args(&gource_args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| format!("start Gource: {e}"))?,
        name: "Gource",
        status: None,
    };
    let frames = gource.child.stdout.take().ok_or("missing Gource stdout")?;
    let mut ffmpeg = Reaped {
        child: Command::new("ffmpeg")
            .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-r"])
            .arg(options.fps.to_string())
            .args([
                "-f",
                "image2pipe",
                "-vcodec",
                "ppm",
                "-i",
                "-",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(video)
            .stdin(frames)
            .stdout(Stdio::null())
            .spawn()
            .map_err(|e| format!("start FFmpeg: {e}"))?,
        name: "FFmpeg",
        status: None,
    };
    loop {
        cancel.check()?;
        let first = gource.poll()?;
        let second = ffmpeg.poll()?;
        // FFmpeg first: a failed encoder usually makes Gource die of a broken pipe.
        for (child, status) in [(&ffmpeg, second), (&gource, first)] {
            if let Some(status) = status.filter(|status| !status.success()) {
                return Err(format!("{} exited with {status}", child.name));
            }
        }
        if first.is_some() && second.is_some() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(20));
    }
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
