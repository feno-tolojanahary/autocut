use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};

use super::crop;
use super::engine;
use super::llm;
use super::transcribe;
use super::types::{Clip, Timeline};
use super::validation::{get_video_duration, validate_input};

/// Entry point for the summarize subcommand.
pub fn run(
    input: &Path,
    output: &Path,
    whisper_model: &Path,
    llm_model: &str,
    language: Option<&str>,
    max_segments: usize,
    max_duration: f64,
    crop_mobile: bool,
    face_model: Option<&Path>,
) -> Result<()> {
    validate_input(input)?;

    if !whisper_model.exists() {
        bail!(
            "whisper model file does not exist: {}",
            whisper_model.display()
        );
    }

    if crop_mobile {
        let fm = face_model.context("--face-model is required when --crop-mobile is set")?;
        if !fm.exists() {
            bail!("face model file does not exist: {}", fm.display());
        }
    }

    // Check Ollama connectivity and model availability early.
    println!("[clip-cli] Checking Ollama connection and model '{llm_model}'...");
    llm::check_ollama(llm_model)?;

    let duration = get_video_duration(input)?;

    // Step 1: Extract audio
    println!("[clip-cli] Step 1/4: Extracting audio...");
    let samples = transcribe::extract_audio_pcm(input, None, None)?;
    let audio_duration = samples.len() as f64 / 16000.0;
    println!(
        "[clip-cli] Extracted {audio_duration:.1}s of audio ({} samples)",
        samples.len()
    );

    // Step 2: Transcribe with Whisper
    println!("[clip-cli] Step 2/4: Transcribing with Whisper...");
    let segments = transcribe::transcribe_pcm(whisper_model, &samples, language)?;
    println!("[clip-cli] Transcribed {} segments", segments.len());

    if segments.is_empty() {
        bail!("no speech detected in video — cannot summarize");
    }

    // Save full transcript alongside the output video.
    let txt_path = output.with_extension("txt");
    let srt_path = output.with_extension("srt");
    fs::write(&txt_path, transcribe::format_txt(&segments))
        .with_context(|| format!("failed to write {}", txt_path.display()))?;
    fs::write(&srt_path, transcribe::format_srt(&segments))
        .with_context(|| format!("failed to write {}", srt_path.display()))?;
    println!(
        "[clip-cli] Transcript saved: {} and {}",
        txt_path.display(),
        srt_path.display()
    );

    // Step 3: Analyze transcript with LLM
    println!("[clip-cli] Step 3/4: Analyzing transcript with LLM ({llm_model})...");
    let sections =
        llm::extract_important_sections(llm_model, &segments, max_segments, duration)?;

    println!(
        "[clip-cli] Identified {} important section(s):",
        sections.len()
    );
    for s in &sections {
        println!("  {:.1}s - {:.1}s: {}", s.start_secs, s.end_secs, s.reason);
    }

    // Clamp each section to max_duration, keeping the start and trimming the end.
    let sections: Vec<_> = sections
        .into_iter()
        .map(|mut s| {
            if s.end_secs - s.start_secs > max_duration {
                s.end_secs = s.start_secs + max_duration;
            }
            s
        })
        .collect();

    // Step 4: Render each segment as a separate file
    println!("[clip-cli] Step 4/4: Rendering {} summary segment(s)...", sections.len());

    let stem = output
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("summary");
    let ext = output
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("mp4");
    let parent = output.parent().unwrap_or_else(|| Path::new("."));

    for (i, s) in sections.iter().enumerate() {
        let seg_path = parent.join(format!("{stem}_{}.{ext}", i + 1));
        let clip = Clip {
            source: input.to_path_buf(),
            start_secs: s.start_secs,
            end_secs: s.end_secs,
        };
        let timeline = Timeline::new(vec![clip]);
        engine::render(&timeline, &seg_path)?;

        let seg_duration = s.end_secs - s.start_secs;
        println!(
            "[clip-cli] Segment {}/{}: {:.1}s - {:.1}s ({:.1}s) → {}",
            i + 1,
            sections.len(),
            s.start_secs,
            s.end_secs,
            seg_duration,
            seg_path.display()
        );
    }

    // Render combined video with all segments concatenated
    println!("[clip-cli] Rendering combined summary video...");
    let all_clips: Vec<Clip> = sections
        .iter()
        .map(|s| Clip {
            source: input.to_path_buf(),
            start_secs: s.start_secs,
            end_secs: s.end_secs,
        })
        .collect();
    let combined_timeline = Timeline::new(all_clips);
    engine::render(&combined_timeline, output)?;

    let total_duration: f64 = sections.iter().map(|s| s.end_secs - s.start_secs).sum();
    println!(
        "[clip-cli] Summary complete: {} segment(s), {:.1}s from {:.1}s original ({:.0}% of video)",
        sections.len(),
        total_duration,
        duration,
        (total_duration / duration) * 100.0,
    );
    println!("[clip-cli] Combined video → {}", output.display());

    if crop_mobile {
        let mobile_path = parent.join(format!("{stem}_mobile.{ext}"));
        crop::crop_mobile(output, &mobile_path, face_model.unwrap())?;
        println!("[clip-cli] Combined mobile crop → {}", mobile_path.display());
    }

    Ok(())
}
