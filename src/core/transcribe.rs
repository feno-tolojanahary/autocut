use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use ffmpeg_next as ffmpeg;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use super::validation::validate_input;

const WHISPER_SAMPLE_RATE: u32 = 16000;

pub struct Segment {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

/// Entry point for the transcribe subcommand.
pub fn run(
    input: &Path,
    output: &Path,
    model: &Path,
    language: Option<&str>,
    start: Option<f64>,
    end: Option<f64>,
) -> Result<()> {
    validate_input(input)?;

    if !model.exists() {
        bail!("model file does not exist: {}", model.display());
    }

    if let (Some(s), Some(e)) = (start, end) {
        if s >= e {
            bail!("start time ({s:.3}s) must be before end time ({e:.3}s)");
        }
    }

    println!("[clip-cli] Extracting audio from {}...", input.display());
    let samples = extract_audio_pcm(input, start, end)?;
    let duration_secs = samples.len() as f64 / WHISPER_SAMPLE_RATE as f64;
    println!(
        "[clip-cli] Extracted {duration_secs:.1}s of audio ({} samples)",
        samples.len()
    );

    println!("[clip-cli] Loading whisper model...");
    let mut segments = transcribe_pcm(model, &samples, language)?;
    println!("[clip-cli] Transcribed {} segments", segments.len());

    // Shift timestamps to match original video time if --start was used.
    if let Some(offset) = start {
        let offset_ms = (offset * 1000.0) as i64;
        for seg in &mut segments {
            seg.start_ms += offset_ms;
            seg.end_ms += offset_ms;
        }
    }

    let txt_path = output.with_extension("txt");
    let srt_path = output.with_extension("srt");

    fs::write(&txt_path, format_txt(&segments))
        .with_context(|| format!("failed to write {}", txt_path.display()))?;
    fs::write(&srt_path, format_srt(&segments))
        .with_context(|| format!("failed to write {}", srt_path.display()))?;

    println!(
        "[clip-cli] Transcription complete: {} and {}",
        txt_path.display(),
        srt_path.display()
    );

    Ok(())
}

/// Extract audio from a video file as 16kHz mono f32 PCM samples.
pub(crate) fn extract_audio_pcm(path: &Path, start: Option<f64>, end: Option<f64>) -> Result<Vec<f32>> {
    let mut input_ctx =
        ffmpeg::format::input(path).with_context(|| format!("failed to open: {}", path.display()))?;

    let audio_stream = input_ctx
        .streams()
        .best(ffmpeg::media::Type::Audio)
        .context("no audio stream found in input")?;

    let audio_stream_idx = audio_stream.index();
    let audio_time_base = audio_stream.time_base();

    // Set up decoder.
    let codec_ctx = ffmpeg::codec::context::Context::from_parameters(audio_stream.parameters())?;
    let mut decoder = codec_ctx.decoder().audio()?;

    // Set up resampler: source format -> 16kHz mono f32.
    let mut resampler = ffmpeg::software::resampling::Context::get(
        decoder.format(),
        decoder.channel_layout(),
        decoder.rate(),
        ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Packed),
        ffmpeg::ChannelLayout::MONO,
        WHISPER_SAMPLE_RATE,
    )?;

    // Seek if start is specified.
    if let Some(s) = start {
        let ts = (s * f64::from(ffmpeg::ffi::AV_TIME_BASE)) as i64;
        input_ctx.seek(ts, ..ts)?;
    }

    let mut samples: Vec<f32> = Vec::new();

    let receive_decoded_frames =
        |decoder: &mut ffmpeg::decoder::Audio, resampler: &mut ffmpeg::software::resampling::Context, samples: &mut Vec<f32>, end: Option<f64>, audio_time_base: ffmpeg::Rational| -> Result<bool> {
            let mut decoded = ffmpeg::frame::Audio::empty();
            while decoder.receive_frame(&mut decoded).is_ok() {
                // Check if we've passed the end time.
                if let Some(e) = end {
                    let pts = decoded.pts().unwrap_or(0);
                    let frame_time =
                        pts as f64 * f64::from(audio_time_base.0) / f64::from(audio_time_base.1);
                    if frame_time >= e {
                        return Ok(true); // done
                    }
                }

                let mut resampled = ffmpeg::frame::Audio::empty();
                resampler.run(&decoded, &mut resampled)?;

                // Extract f32 samples from the resampled frame.
                let data = resampled.data(0);
                let float_samples: &[f32] = unsafe {
                    std::slice::from_raw_parts(
                        data.as_ptr() as *const f32,
                        data.len() / std::mem::size_of::<f32>(),
                    )
                };
                // Only take as many samples as there are in this frame.
                let n = resampled.samples();
                samples.extend_from_slice(&float_samples[..n]);
            }
            Ok(false)
        };

    for (stream, packet) in input_ctx.packets() {
        if stream.index() != audio_stream_idx {
            continue;
        }

        decoder.send_packet(&packet)?;
        let done = receive_decoded_frames(&mut decoder, &mut resampler, &mut samples, end, audio_time_base)?;
        if done {
            break;
        }
    }

    // Flush the decoder.
    decoder.send_eof()?;
    let _ = receive_decoded_frames(&mut decoder, &mut resampler, &mut samples, end, audio_time_base);

    // Flush the resampler (may have buffered samples).
    let mut flushed = ffmpeg::frame::Audio::empty();
    if resampler.flush(&mut flushed).is_ok() && flushed.samples() > 0 {
        let data = flushed.data(0);
        let float_samples: &[f32] = unsafe {
            std::slice::from_raw_parts(
                data.as_ptr() as *const f32,
                data.len() / std::mem::size_of::<f32>(),
            )
        };
        samples.extend_from_slice(&float_samples[..flushed.samples()]);
    }

    Ok(samples)
}

/// Run whisper inference on PCM samples and return timed segments.
pub(crate) fn transcribe_pcm(
    model_path: &Path,
    samples: &[f32],
    language: Option<&str>,
) -> Result<Vec<Segment>> {
    let ctx = WhisperContext::new_with_params(
        model_path.to_str().context("invalid model path")?,
        WhisperContextParameters::default(),
    )
    .context("failed to load whisper model")?;

    let mut state = ctx.create_state().context("failed to create whisper state")?;

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_special(false);
    params.set_print_timestamps(false);

    if let Some(lang) = language {
        params.set_language(Some(lang));
    }

    state
        .full(params, samples)
        .context("whisper transcription failed")?;

    let n_segments = state.full_n_segments()?;
    let mut segments = Vec::with_capacity(n_segments as usize);

    for i in 0..n_segments {
        let text = state.full_get_segment_text(i)?;
        let t0 = state.full_get_segment_t0(i)?;
        let t1 = state.full_get_segment_t1(i)?;
        // whisper-rs returns timestamps in centiseconds (10ms units).
        segments.push(Segment {
            start_ms: t0 as i64 * 10,
            end_ms: t1 as i64 * 10,
            text: text.trim().to_string(),
        });
    }

    Ok(segments)
}

/// Format segments as plain text.
pub(crate) fn format_txt(segments: &[Segment]) -> String {
    segments
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Format segments as SRT subtitle format.
pub(crate) fn format_srt(segments: &[Segment]) -> String {
    let mut out = String::new();
    for (i, seg) in segments.iter().enumerate() {
        out.push_str(&format!(
            "{}\n{} --> {}\n{}\n\n",
            i + 1,
            ms_to_srt_time(seg.start_ms),
            ms_to_srt_time(seg.end_ms),
            seg.text,
        ));
    }
    out
}

fn ms_to_srt_time(ms: i64) -> String {
    let total_secs = ms / 1000;
    let millis = ms % 1000;
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02},{millis:03}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ms_to_srt_time() {
        assert_eq!(ms_to_srt_time(0), "00:00:00,000");
        assert_eq!(ms_to_srt_time(1500), "00:00:01,500");
        assert_eq!(ms_to_srt_time(90500), "00:01:30,500");
        assert_eq!(ms_to_srt_time(3661000), "01:01:01,000");
    }

    #[test]
    fn test_format_srt() {
        let segments = vec![
            Segment { start_ms: 0, end_ms: 2000, text: "Hello".to_string() },
            Segment { start_ms: 2500, end_ms: 5000, text: "World".to_string() },
        ];
        let srt = format_srt(&segments);
        assert!(srt.contains("1\n00:00:00,000 --> 00:00:02,000\nHello"));
        assert!(srt.contains("2\n00:00:02,500 --> 00:00:05,000\nWorld"));
    }

    #[test]
    fn test_format_txt() {
        let segments = vec![
            Segment { start_ms: 0, end_ms: 2000, text: "Hello".to_string() },
            Segment { start_ms: 2500, end_ms: 5000, text: "World".to_string() },
        ];
        assert_eq!(format_txt(&segments), "Hello\nWorld");
    }
}
