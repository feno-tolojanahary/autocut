mod cli;
mod core;

use std::fs;

use anyhow::{Context, Result};
use clap::Parser;

use crate::cli::{Cli, Command};
use crate::core::engine;
use crate::core::operations::{RemoveOp, SplitOp, TrimOp};
use crate::core::timestamp::parse_timestamp;
use crate::core::types::VideoOperation;

fn main() -> Result<()> {
    ffmpeg_next::init().context("failed to initialize ffmpeg")?;

    let cli = Cli::parse();

    match cli.command {
        Command::Trim {
            input,
            output,
            start,
            end,
        } => {
            let start = parse_timestamp(&start)?;
            let end = parse_timestamp(&end)?;
            let op = TrimOp {
                input,
                output,
                start,
                end,
            };
            run_operation(&op, "Trim")?;
        }
        Command::Split {
            input,
            output_dir,
            at,
        } => {
            fs::create_dir_all(&output_dir).with_context(|| {
                format!("failed to create output directory: {}", output_dir.display())
            })?;
            let timestamps: Vec<f64> = at
                .iter()
                .map(|t| parse_timestamp(t))
                .collect::<Result<_>>()?;
            let op = SplitOp {
                input,
                output_dir,
                at: timestamps,
            };
            run_operation(&op, "Split")?;
        }
        Command::Remove {
            input,
            output,
            start,
            end,
        } => {
            let start = parse_timestamp(&start)?;
            let end = parse_timestamp(&end)?;
            let op = RemoveOp {
                input,
                output,
                start,
                end,
            };
            run_operation(&op, "Remove")?;
        }
        Command::Summarize {
            input,
            output,
            whisper_model,
            llm_model,
            language,
            max_segments,
            max_duration,
            crop_mobile,
            face_model,
        } => {
            crate::core::summarize::run(
                &input,
                &output,
                &whisper_model,
                &llm_model,
                language.as_deref(),
                max_segments,
                max_duration,
                crop_mobile,
                face_model.as_deref(),
            )?;

        }
        Command::Transcribe {
            input,
            output,
            model,
            language,
            start,
            end,
        } => {
            let start = start.map(|s| parse_timestamp(&s)).transpose()?;
            let end = end.map(|s| parse_timestamp(&s)).transpose()?;
            crate::core::transcribe::run(
                &input,
                &output,
                &model,
                language.as_deref(),
                start,
                end,
            )?;
        }
    }

    Ok(())
}

fn run_operation(op: &dyn VideoOperation, name: &str) -> Result<()> {
    println!("[clip-cli] Planning {name} operation...");
    let outputs = op.plan()?;
    for (i, (path, timeline)) in outputs.iter().enumerate() {
        println!(
            "[clip-cli] Rendering output {} of {}: {}",
            i + 1,
            outputs.len(),
            path.display()
        );
        engine::render(timeline, path)?;
    }
    println!("[clip-cli] {name} complete.");
    Ok(())
}
