use std::path::PathBuf;

use anyhow::{bail, Result};

use super::types::{Clip, Timeline, VideoOperation};
use super::validation::get_video_duration;

/// Trim: keep one section of the video.
pub struct TrimOp {
    pub input: PathBuf,
    pub output: PathBuf,
    pub start: f64,
    pub end: f64,
}

impl VideoOperation for TrimOp {
    fn plan(&self) -> Result<Vec<(PathBuf, Timeline)>> {
        let duration = get_video_duration(&self.input)?;
        validate_range(self.start, self.end, duration)?;
        Ok(vec![(
            self.output.clone(),
            Timeline::single(self.input.clone(), self.start, self.end),
        )])
    }
}

/// Split: cut a video into multiple pieces at given timestamps.
pub struct SplitOp {
    pub input: PathBuf,
    pub output_dir: PathBuf,
    pub at: Vec<f64>,
}

impl VideoOperation for SplitOp {
    fn plan(&self) -> Result<Vec<(PathBuf, Timeline)>> {
        let duration = get_video_duration(&self.input)?;

        let mut points = self.at.clone();
        points.sort_by(|a, b| a.partial_cmp(b).unwrap());
        points.dedup();

        for &t in &points {
            if t <= 0.0 || t >= duration {
                bail!(
                    "split point {t:.3}s is outside video duration (0 .. {duration:.3}s)"
                );
            }
        }

        // Build segments: [0, p1], [p1, p2], ..., [pN, duration]
        let mut boundaries = vec![0.0];
        boundaries.extend(&points);
        boundaries.push(duration);

        let mut result = Vec::new();
        for i in 0..boundaries.len() - 1 {
            let start = boundaries[i];
            let end = boundaries[i + 1];
            let filename = format!("part_{:03}.mp4", i + 1);
            let out_path = self.output_dir.join(filename);
            result.push((
                out_path,
                Timeline::single(self.input.clone(), start, end),
            ));
        }
        Ok(result)
    }
}

/// Remove: cut out a section and stitch the remaining parts together.
pub struct RemoveOp {
    pub input: PathBuf,
    pub output: PathBuf,
    pub start: f64,
    pub end: f64,
}

impl VideoOperation for RemoveOp {
    fn plan(&self) -> Result<Vec<(PathBuf, Timeline)>> {
        let duration = get_video_duration(&self.input)?;
        validate_range(self.start, self.end, duration)?;

        let mut clips = Vec::new();

        // Part before the cut
        if self.start > 0.0 {
            clips.push(Clip {
                source: self.input.clone(),
                start_secs: 0.0,
                end_secs: self.start,
            });
        }

        // Part after the cut
        if self.end < duration {
            clips.push(Clip {
                source: self.input.clone(),
                start_secs: self.end,
                end_secs: duration,
            });
        }

        if clips.is_empty() {
            bail!("removing the entire video leaves no content");
        }

        Ok(vec![(self.output.clone(), Timeline::new(clips))])
    }
}

fn validate_range(start: f64, end: f64, duration: f64) -> Result<()> {
    if start < 0.0 {
        bail!("start time cannot be negative");
    }
    if end > duration {
        bail!("end time ({end:.3}s) exceeds video duration ({duration:.3}s)");
    }
    if start >= end {
        bail!("start time ({start:.3}s) must be before end time ({end:.3}s)");
    }
    Ok(())
}
