use std::path::PathBuf;

/// A time range within a source video file.
#[derive(Debug, Clone)]
pub struct Clip {
    pub source: PathBuf,
    pub start_secs: f64,
    pub end_secs: f64,
}

/// An ordered sequence of clips that together form the output video.
/// For trim: one clip. For remove: two clips (before + after the cut).
/// For split: produces multiple single-clip timelines.
#[derive(Debug, Clone)]
pub struct Timeline {
    pub clips: Vec<Clip>,
}

impl Timeline {
    pub fn new(clips: Vec<Clip>) -> Self {
        Self { clips }
    }

    pub fn single(source: PathBuf, start: f64, end: f64) -> Self {
        Self {
            clips: vec![Clip {
                source,
                start_secs: start,
                end_secs: end,
            }],
        }
    }
}

/// Trait for all video operations. Each operation builds one or more timelines
/// from user arguments, then the engine renders them to output files.
pub trait VideoOperation {
    /// Returns (output_path, timeline) pairs to render.
    fn plan(&self) -> anyhow::Result<Vec<(PathBuf, Timeline)>>;
}
