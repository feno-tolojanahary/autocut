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

    for clip in &timeline.clips {
        let last_dts = write_clip(&mut output_ctx, clip, &stream_mapping, &cumulative_offsets)?;
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
) -> Result<Vec<i64>> {
    let mut input_ctx = ffmpeg::format::input(&clip.source)
        .with_context(|| format!("failed to open: {}", clip.source.display()))?;

    let nb_streams = input_ctx.nb_streams() as usize;
    let start_ts = (clip.start_secs * f64::from(ffmpeg::ffi::AV_TIME_BASE)) as i64;

    // Seek to the start position. Use backward seek to land on a keyframe before start.
    input_ctx.seek(start_ts, ..start_ts)?;

    // Per-stream: the first DTS seen (used to normalize timestamps to 0).
    let mut first_dts: Vec<Option<i64>> = vec![None; nb_streams];
    // Per-stream: the last DTS written (returned to caller for multi-clip offset tracking).
    let mut last_dts: Vec<i64> = vec![0; nb_streams];
    // Per-stream: whether we've seen a packet past end_secs.
    let mut stream_done: Vec<bool> = vec![false; nb_streams];

    for (stream, packet) in input_ctx.packets() {
        let stream_idx = stream.index();
        if stream_idx >= stream_mapping.len() || stream_done[stream_idx] {
            continue;
        }

        let time_base = stream.time_base();

        // Convert PTS to seconds to check if within clip range.
        let pts = packet.pts().unwrap_or(packet.dts().unwrap_or(0));
        let pkt_time = pts as f64 * f64::from(time_base.0) / f64::from(time_base.1);

        // Skip packets before start (from keyframe seek overshoot).
        if pkt_time < clip.start_secs - 0.5 {
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

        // Rescale timestamps from input to output time base.
        out_packet.rescale_ts(time_base, out_time_base);

        // Record the first DTS for this stream to normalize timestamps.
        if first_dts[stream_idx].is_none() {
            first_dts[stream_idx] = out_packet.dts();
        }

        // Shift timestamps: normalize to 0 then add cumulative offset.
        if let Some(base_dts) = first_dts[stream_idx] {
            if let Some(dts) = out_packet.dts() {
                let new_dts = dts - base_dts + cumulative_offsets[stream_idx];
                out_packet.set_dts(Some(new_dts));
                last_dts[stream_idx] = new_dts;
            }
            if let Some(pts) = out_packet.pts() {
                out_packet.set_pts(Some(pts - base_dts + cumulative_offsets[stream_idx]));
            }
        }

        out_packet.set_position(-1);
        out_packet
            .write_interleaved(output_ctx)
            .context("failed to write packet")?;
    }

    Ok(last_dts)
}
