use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "clip-cli", version, about = "A CLI tool for cutting MP4 video files")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Keep one section of the video, discard the rest
    Trim {
        /// Input MP4 file
        #[arg(short, long)]
        input: PathBuf,

        /// Output MP4 file
        #[arg(short, long)]
        output: PathBuf,

        /// Start timestamp (HH:MM:SS, HH:MM:SS.mmm, or seconds)
        #[arg(short, long)]
        start: String,

        /// End timestamp (HH:MM:SS, HH:MM:SS.mmm, or seconds)
        #[arg(short, long)]
        end: String,
    },

    /// Cut a video into multiple pieces at the given timestamps
    Split {
        /// Input MP4 file
        #[arg(short, long)]
        input: PathBuf,

        /// Output directory for numbered parts
        #[arg(short, long)]
        output_dir: PathBuf,

        /// Timestamps at which to split (can be repeated)
        #[arg(long)]
        at: Vec<String>,
    },

    /// Cut out a section from the middle and stitch the remaining parts together
    Remove {
        /// Input MP4 file
        #[arg(short, long)]
        input: PathBuf,

        /// Output MP4 file
        #[arg(short, long)]
        output: PathBuf,

        /// Start of section to remove (HH:MM:SS, HH:MM:SS.mmm, or seconds)
        #[arg(short, long)]
        start: String,

        /// End of section to remove (HH:MM:SS, HH:MM:SS.mmm, or seconds)
        #[arg(short, long)]
        end: String,
    },

    /// Automatically find important sections and combine them into a summary video
    Summarize {
        /// Input MP4 file
        #[arg(short, long)]
        input: PathBuf,

        /// Output MP4 file
        #[arg(short, long)]
        output: PathBuf,

        /// Path to whisper GGML model file (e.g. ggml-base.en.bin)
        #[arg(short = 'w', long)]
        whisper_model: PathBuf,

        /// Ollama model name (e.g. "mistral"). Requires Ollama running locally.
        #[arg(short = 'l', long, default_value = "mistral")]
        llm_model: String,

        /// Language code (e.g. "en"). Omit for auto-detection.
        #[arg(long)]
        language: Option<String>,

        /// Maximum number of important sections to extract
        #[arg(long, default_value = "5")]
        max_segments: usize,

        /// Maximum duration in seconds for each segment
        #[arg(long, default_value = "10")]
        max_duration: f64,

        /// Crop output to 9:16 portrait (1080x1920) with face tracking
        #[arg(long, default_value_t = false)]
        crop_mobile: bool,

        /// Path to rustface model file (seeta_fd_frontal_v1.0.bin). Required when --crop-mobile is set.
        #[arg(long)]
        face_model: Option<PathBuf>,
    },

    /// Extract speech from video and produce a text transcript and SRT subtitles
    Transcribe {
        /// Input MP4 file
        #[arg(short, long)]
        input: PathBuf,

        /// Output file base name (produces .txt and .srt)
        #[arg(short, long)]
        output: PathBuf,

        /// Path to whisper GGML model file (e.g. ggml-base.en.bin)
        #[arg(short, long)]
        model: PathBuf,

        /// Language code (e.g. "en"). Omit for auto-detection.
        #[arg(short, long)]
        language: Option<String>,

        /// Start timestamp to begin transcription (optional)
        #[arg(long)]
        start: Option<String>,

        /// End timestamp to stop transcription (optional)
        #[arg(long)]
        end: Option<String>,
    },
}
