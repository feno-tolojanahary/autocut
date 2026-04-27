use std::path::Path;

use anyhow::{bail, Context, Result};

/// Validate that the input file exists and is an MP4.
pub fn validate_input(path: &Path) -> Result<()> {
    if !path.exists() {
        bail!("input file does not exist: {}", path.display());
    }
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("mp4") => Ok(()),
        _ => bail!(
            "only MP4 files are supported, got: {}",
            path.display()
        ),
    }
}

/// Get the duration of a video file in seconds using ffmpeg.
pub fn get_video_duration(path: &Path) -> Result<f64> {
    validate_input(path)?;
    let ctx = ffmpeg_next::format::input(path)
        .with_context(|| format!("failed to open video: {}", path.display()))?;
    let duration_ts = ctx.duration();
    if duration_ts <= 0 {
        bail!("could not determine video duration for: {}", path.display());
    }
    // ffmpeg duration is in AV_TIME_BASE units (microseconds)
    Ok(duration_ts as f64 / f64::from(ffmpeg_next::ffi::AV_TIME_BASE))
}
