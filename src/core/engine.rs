use std::path::Path;

use anyhow::{Context, Result};
use ffmpeg_next as ffmpeg;

use super::types::Timeline;

/// Render a timeline to an output MP4 file using stream copy (no re-encoding).
pub fn render(timeline: &Timeline, output: &Path) -> Result<()> {
    if timeline.clips.is_empty() {
        anyhow::bail!("timeline has no clips to render");
    }

    // Use the first clip's source to set up output format and streams.
    let first_source = &timeline.clips[0].source;
    let input_ctx = ffmpeg::format::input(first_source)
        .with_context(|| format!("failed to open: {}", first_source.display()))?;

    let mut output_ctx = ffmpeg::format::output(output)
        .with_context(|| format!("failed to create output: {}", output.display()))?;

    let nb_streams = input_ctx.nb_streams() as usize;

    // Map all streams from input to output.
    let mut stream_mapping: Vec<usize> = Vec::new();
    let mut out_idx = 0;
    for in_stream in input_ctx.streams() {
        let codec_params = in_stream.parameters();
        let mut out_stream = output_ctx.add_stream(ffmpeg::encoder::find(ffmpeg::codec::Id::None))?;
        out_stream.set_parameters(codec_params);
        unsafe {
            (*out_stream.parameters().as_mut_ptr()).codec_tag = 0;
        }
        stream_mapping.push(out_idx);
        out_idx += 1;
    }
    drop(input_ctx);

    output_ctx
        .write_header()
        .context("failed to write output header")?;

    // Track cumulative duration offsets across clips for multi-clip timelines.
    let mut cumulative_offsets: Vec<i64> = vec![0; nb_streams];
    // Track the maximum DTS written per stream across all clips to guarantee
    // monotonically increasing DTS even across clip boundaries with B-frames.
    let mut max_dts_written: Vec<i64> = vec![-1; nb_streams];

    for clip in &timeline.clips {
        let last_dts = write_clip(
            &mut output_ctx,
            clip,
            &stream_mapping,
            &cumulative_offsets,
            &mut max_dts_written,
        )?;
        // Advance cumulative offsets by the last DTS seen in each stream + 1.
        for i in 0..nb_streams {
            if last_dts[i] > 0 {
                cumulative_offsets[i] = last_dts[i] + 1;
            }
        }
    }

    output_ctx
        .write_trailer()
        .context("failed to write output trailer")?;

    Ok(())
}

fn write_clip(
    output_ctx: &mut ffmpeg::format::context::Output,
    clip: &super::types::Clip,
    stream_mapping: &[usize],
    cumulative_offsets: &[i64],
    max_dts_written: &mut [i64],
) -> Result<Vec<i64>> {
    let mut input_ctx = ffmpeg::format::input(&clip.source)
        .with_context(|| format!("failed to open: {}", clip.source.display()))?;

    let nb_streams = input_ctx.nb_streams() as usize;
    let start_ts = (clip.start_secs * f64::from(ffmpeg::ffi::AV_TIME_BASE)) as i64;

    // Seek to the start position. Use backward seek to land on a keyframe before start.
    input_ctx.seek(start_ts, ..start_ts)?;

    // Per-stream: the last DTS written (returned to caller for multi-clip offset tracking).
    let mut last_dts: Vec<i64> = vec![0; nb_streams];
    // Per-stream: whether we've seen a packet past end_secs.
    let mut stream_done: Vec<bool> = vec![false; nb_streams];
    // Whether we've seen a video keyframe at or before start_secs (needed for clean decoding).
    let mut seen_video_keyframe = false;
    // The real-world time (seconds) used as a common reference for all streams.
    // Set to the first video keyframe time so video starts at DTS=0 and audio
    // is offset by the same amount, keeping them in sync.
    let mut reference_secs: Option<f64> = None;

    for (stream, packet) in input_ctx.packets() {
        let stream_idx = stream.index();
        if stream_idx >= stream_mapping.len() || stream_done[stream_idx] {
            continue;
        }

        let time_base = stream.time_base();

        // Convert PTS to seconds to check if within clip range.
        let pts = packet.pts().unwrap_or(packet.dts().unwrap_or(0));
        let pkt_time = pts as f64 * f64::from(time_base.0) / f64::from(time_base.1);

        let is_video = stream.parameters().medium() == ffmpeg::media::Type::Video;

        // For the video stream, we must start from a keyframe to avoid corruption.
        // Skip all video packets until we find a keyframe, then include it even if
        // it's slightly before start_secs.
        if is_video && !seen_video_keyframe {
            if packet.is_key() && pkt_time <= clip.start_secs {
                seen_video_keyframe = true;
                // Use this keyframe's time as the sync reference for all streams.
                reference_secs = Some(pkt_time);
            } else if packet.is_key() && pkt_time > clip.start_secs {
                // Keyframe is past start — use it anyway (no earlier keyframe available).
                seen_video_keyframe = true;
                reference_secs = Some(pkt_time);
            } else {
                // Non-keyframe before we found our keyframe — skip it.
                continue;
            }
        }

        // For non-video streams (audio), skip packets before the reference time.
        // This ensures audio starts at the same real-world time as the video keyframe.
        let ref_secs = reference_secs.unwrap_or(clip.start_secs);
        if !is_video && pkt_time < ref_secs - 0.05 {
            continue;
        }

        // Mark this stream as done when we pass end time.
        if pkt_time >= clip.end_secs {
            stream_done[stream_idx] = true;
            if stream_done.iter().all(|&done| done) {
                break;
            }
            continue;
        }

        let out_stream_idx = stream_mapping[stream_idx];
        let out_time_base = output_ctx.stream(out_stream_idx).unwrap().time_base();

        let mut out_packet = packet.clone();
        out_packet.set_stream(out_stream_idx);

        // Compute the base DTS for this stream from the common reference time.
        // Converting the same real-world time to each stream's output time base
        // ensures audio and video are normalized by the same offset.
        let base_dts = (ref_secs * f64::from(out_time_base.1) / f64::from(out_time_base.0)) as i64;

        // Rescale timestamps from input to output time base.
        out_packet.rescale_ts(time_base, out_time_base);

        // Shift timestamps: normalize relative to clip start then add cumulative offset.
        // Enforce strictly increasing DTS to avoid muxer errors with B-frame reordering.
        if let Some(dts) = out_packet.dts() {
            let mut new_dts = (dts - base_dts + cumulative_offsets[stream_idx]).max(0);
            if new_dts <= max_dts_written[stream_idx] {
                new_dts = max_dts_written[stream_idx] + 1;
            }
            let dts_shift = new_dts - (dts - base_dts + cumulative_offsets[stream_idx]);
            out_packet.set_dts(Some(new_dts));
            max_dts_written[stream_idx] = new_dts;
            last_dts[stream_idx] = new_dts;

            // Apply the same shift to PTS to preserve the PTS-DTS delta (B-frame ordering).
            if let Some(pts) = out_packet.pts() {
                let new_pts = (pts - base_dts + cumulative_offsets[stream_idx] + dts_shift).max(new_dts);
                out_packet.set_pts(Some(new_pts));
            }
        } else if let Some(pts) = out_packet.pts() {
            let new_pts = (pts - base_dts + cumulative_offsets[stream_idx]).max(0);
            out_packet.set_pts(Some(new_pts));
        }

        out_packet.set_position(-1);
        out_packet
            .write_interleaved(output_ctx)
            .context("failed to write packet")?;
    }

    Ok(last_dts)
}
