pub mod gource;
pub mod story;

pub use gource::GourceOptions;

use crate::{
    analysis::AnalysisOptions,
    config::Config,
    git::Repository,
    model::RepositoryAnalytics,
    progress::CancelFlag,
    render::{checked_directory, raster, Theme},
};
use std::{
    fs,
    io::{BufWriter, ErrorKind, Write},
    path::{Path, PathBuf},
};

const MAX_GIF_BYTES: u64 = 20_000_000;

/// A sibling temporary file that is removed unless published by rename.
struct TempOutput {
    temp: PathBuf,
    target: PathBuf,
    published: bool,
}

impl TempOutput {
    /// Refuses symlink or non-file targets; `suffix` keeps tools' extension inference.
    fn create(target: &Path, suffix: &str) -> Result<(Self, fs::File), String> {
        let name = target
            .file_name()
            .ok_or_else(|| format!("output needs a file name: {}", target.display()))?;
        let parent = target
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        checked_directory(parent)?;
        // A "./" prefix keeps relative temporary paths from reading as child options.
        let parent = if parent.is_relative() {
            Path::new(".").join(parent)
        } else {
            parent.to_path_buf()
        };
        match fs::symlink_metadata(target) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(format!("refusing symlink output: {}", target.display()))
            }
            Ok(meta) if !meta.is_file() => {
                return Err(format!("output is not a file: {}", target.display()))
            }
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(format!("inspect {}: {error}", target.display())),
        }
        let temp = parent.join(format!(
            ".{}.{}.tmp{suffix}",
            name.to_string_lossy(),
            std::process::id()
        ));
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|e| format!("create {}: {e}", temp.display()))?;
        Ok((
            Self {
                temp,
                target: target.to_path_buf(),
                published: false,
            },
            file,
        ))
    }

    fn publish(mut self) -> Result<(), String> {
        fs::rename(&self.temp, &self.target)
            .map_err(|e| format!("write {}: {e}", self.target.display()))?;
        self.published = true;
        Ok(())
    }
}

impl Drop for TempOutput {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_file(&self.temp);
        }
    }
}

/// A helper file removed when dropped.
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Write the fixed seven-scene 720×720 GIF recap; the static poster is the reduced-motion alternative.
pub fn write_gif(data: &RepositoryAnalytics, output: &Path, theme: Theme) -> Result<(), String> {
    write_gif_with_cancel(data, output, theme, &CancelFlag::default())
}

pub fn write_gif_with_cancel(
    data: &RepositoryAnalytics,
    output: &Path,
    theme: Theme,
    cancel: &CancelFlag,
) -> Result<(), String> {
    use image::{codecs::gif, Delay, Frame, RgbaImage};
    let (temp, file) = TempOutput::create(output, ".gif")?;
    {
        let mut writer = BufWriter::new(&file);
        // The GIF trailer is written when the encoder drops; the flush below reports I/O errors.
        let mut encoder = gif::GifEncoder::new_with_speed(&mut writer, 10);
        encoder
            .set_repeat(gif::Repeat::Infinite)
            .map_err(|e| format!("encode GIF: {e}"))?;
        for scene in story::scenes(data, theme) {
            for index in 0..story::FRAMES_PER_SCENE {
                cancel.check()?;
                let svg = scene.at(index, story::FRAMES_PER_SCENE);
                let rgba = raster::render_rgba(&svg, story::SIZE, story::SIZE)?;
                let image = RgbaImage::from_raw(story::SIZE, story::SIZE, rgba)
                    .ok_or("invalid RGBA frame")?;
                let delay = scene.frame_delay_ms(index, story::FRAMES_PER_SCENE);
                encoder
                    .encode_frame(Frame::from_parts(
                        image,
                        0,
                        0,
                        Delay::from_numer_denom_ms(delay, 1),
                    ))
                    .map_err(|e| format!("encode GIF: {e}"))?;
            }
        }
        drop(encoder);
        writer.flush().map_err(|e| format!("write GIF: {e}"))?;
    }
    let size = file
        .metadata()
        .map_err(|e| format!("inspect GIF: {e}"))?
        .len();
    if size > MAX_GIF_BYTES {
        return Err(format!("GIF would exceed the 20 MB limit ({size} bytes)"));
    }
    file.sync_all().map_err(|e| format!("write GIF: {e}"))?;
    drop(file);
    cancel.check()?;
    temp.publish()
}

/// Render repository history through optional Gource and FFmpeg into an MP4.
pub fn write_mp4(
    repo: &Repository,
    config: &Config,
    filters: &AnalysisOptions,
    output: &Path,
    options: GourceOptions,
) -> Result<(), String> {
    write_mp4_with_cancel(
        repo,
        config,
        filters,
        output,
        options,
        &CancelFlag::default(),
    )
}

pub fn write_mp4_with_cancel(
    repo: &Repository,
    config: &Config,
    filters: &AnalysisOptions,
    output: &Path,
    options: GourceOptions,
    cancel: &CancelFlag,
) -> Result<(), String> {
    options.validate()?;
    gource::require_tool("gource", "--version", "Gource")?;
    gource::require_tool("ffmpeg", "-version", "FFmpeg")?;
    let (temp, file) = TempOutput::create(output, ".mp4")?;
    drop(file);
    let log = TempFile(temp.temp.with_extension("gource.log"));
    let stats = gource::write_custom_log_with_cancel(repo, config, filters, &log.0, cancel)?;
    if stats.events == 0 {
        return Err("no selected file changes to animate".into());
    }
    gource::run_pipeline(&log.0, &temp.temp, options, cancel)?;
    cancel.check()?;
    temp.publish()
}
