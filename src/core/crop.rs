use std::path::Path;

use anyhow::{bail, Context, Result};
use ffmpeg_next as ffmpeg;

const MOBILE_WIDTH: u32 = 1080;
const MOBILE_HEIGHT: u32 = 1920;
const FACE_SAMPLE_INTERVAL_SECS: f64 = 0.5;

/// A face position detected at a specific time in the video.
#[derive(Clone, Copy)]
struct FacePosition {
    time_secs: f64,
    /// Horizontal center of the face as a ratio of frame width (0.0 = left, 1.0 = right).
    x_ratio: f64,
}

/// Crop a video to mobile portrait format (1080x1920, 9:16) with face tracking.
pub fn crop_mobile(input: &Path, output: &Path, face_model: &Path) -> Result<()> {
    println!("[clip-cli] Crop: pass 1/2 -- detecting face positions...");
    let positions = detect_face_positions(input, face_model)?;
    let smoothed = smooth_positions(&positions);
    println!(
        "[clip-cli] Detected {} face position sample(s)",
        smoothed.len()
    );

    println!("[clip-cli] Crop: pass 2/2 -- re-encoding with face-centered crop...");
    reencode_cropped(input, output, &smoothed)?;

    Ok(())
}

// -- Pass 1: face detection ------------------------------------------------

fn detect_face_positions(input: &Path, face_model: &Path) -> Result<Vec<FacePosition>> {
    let model_path = face_model
        .to_str()
        .context("face model path is not valid UTF-8")?;
    let mut detector =
        rustface::create_detector(model_path).context("failed to load face detection model")?;
    detector.set_min_face_size(60);
    detector.set_score_thresh(2.0);
    detector.set_pyramid_scale_factor(0.8);
    detector.set_slide_window_step(4, 4);

    let mut input_ctx = ffmpeg::format::input(input)
        .with_context(|| format!("failed to open: {}", input.display()))?;

    let video_stream = input_ctx
        .streams()
        .best(ffmpeg::media::Type::Video)
        .context("no video stream found")?;
    let video_idx = video_stream.index();
    let time_base = video_stream.time_base();

    let codec_ctx = ffmpeg::codec::context::Context::from_parameters(video_stream.parameters())?;
    let mut decoder = codec_ctx.decoder().video()?;
    let src_w = decoder.width();
    let src_h = decoder.height();

    // Scale to grayscale for face detection.
    let mut gray_scaler = ffmpeg::software::scaling::Context::get(
        decoder.format(),
        src_w,
        src_h,
        ffmpeg::format::Pixel::GRAY8,
        src_w,
        src_h,
        ffmpeg::software::scaling::Flags::BILINEAR,
    )?;

    let mut positions = Vec::new();
    let mut last_sample = -FACE_SAMPLE_INTERVAL_SECS;

    for (stream, packet) in input_ctx.packets() {
        if stream.index() != video_idx {
            continue;
        }
        decoder.send_packet(&packet)?;

        let mut frame = ffmpeg::frame::Video::empty();
        while decoder.receive_frame(&mut frame).is_ok() {
            let pts = frame.pts().unwrap_or(0);
            let time = pts as f64 * f64::from(time_base.0) / f64::from(time_base.1);

            if time - last_sample < FACE_SAMPLE_INTERVAL_SECS {
                continue;
            }
            last_sample = time;

            // Convert to grayscale.
            let mut gray = ffmpeg::frame::Video::new(ffmpeg::format::Pixel::GRAY8, src_w, src_h);
            gray_scaler.run(&frame, &mut gray)?;

            // Build contiguous buffer (stride may exceed width).
            let stride = gray.stride(0);
            let w = src_w as usize;
            let h = src_h as usize;
            let raw = gray.data(0);
            let buf = if stride == w {
                raw[..w * h].to_vec()
            } else {
                let mut v = Vec::with_capacity(w * h);
                for row in 0..h {
                    v.extend_from_slice(&raw[row * stride..row * stride + w]);
                }
                v
            };

            let mut image = rustface::ImageData::new(&buf, src_w, src_h);
            let faces = detector.detect(&mut image);

            // Pick the largest face by area.
            let x_ratio = faces
                .iter()
                .max_by_key(|f| f.bbox().width() * f.bbox().height())
                .map(|f| {
                    let b = f.bbox();
                    let cx = b.x() as f64 + b.width() as f64 / 2.0;
                    (cx / src_w as f64).clamp(0.0, 1.0)
                })
                .unwrap_or(0.5);

            positions.push(FacePosition {
                time_secs: time,
                x_ratio,
            });
        }
    }

    // Flush decoder.
    decoder.send_eof()?;
    let mut frame = ffmpeg::frame::Video::empty();
    while decoder.receive_frame(&mut frame).is_ok() {}

    if positions.is_empty() {
        println!("[clip-cli] Warning: no frames sampled, defaulting to center crop");
        positions.push(FacePosition {
            time_secs: 0.0,
            x_ratio: 0.5,
        });
    }

    Ok(positions)
}

// -- Smoothing --------------------------------------------------------------

fn smooth_positions(raw: &[FacePosition]) -> Vec<FacePosition> {
    if raw.len() <= 1 {
        return raw.to_vec();
    }
    let alpha = 0.3_f64;
    let mut out = Vec::with_capacity(raw.len());
    let mut prev = raw[0].x_ratio;
    for p in raw {
        let s = alpha * p.x_ratio + (1.0 - alpha) * prev;
        out.push(FacePosition {
            time_secs: p.time_secs,
            x_ratio: s,
        });
        prev = s;
    }
    out
}

/// Linearly interpolate the face X ratio for a given time.
fn x_ratio_at(positions: &[FacePosition], t: f64) -> f64 {
    if positions.is_empty() {
        return 0.5;
    }
    if t <= positions[0].time_secs {
        return positions[0].x_ratio;
    }
    if t >= positions.last().unwrap().time_secs {
        return positions.last().unwrap().x_ratio;
    }
    for w in positions.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        if t >= a.time_secs && t <= b.time_secs {
            let span = b.time_secs - a.time_secs;
            if span < 1e-9 {
                return a.x_ratio;
            }
            let frac = (t - a.time_secs) / span;
            return a.x_ratio + frac * (b.x_ratio - a.x_ratio);
        }
    }
    0.5
}

// -- Pass 2: re-encode with crop -------------------------------------------

fn reencode_cropped(
    input: &Path,
    output: &Path,
    positions: &[FacePosition],
) -> Result<()> {
    let mut input_ctx = ffmpeg::format::input(input)?;

    // -- Identify streams --
    let video_stream = input_ctx
        .streams()
        .best(ffmpeg::media::Type::Video)
        .context("no video stream")?;
    let video_idx = video_stream.index();
    let video_tb = video_stream.time_base();

    let codec_ctx = ffmpeg::codec::context::Context::from_parameters(video_stream.parameters())?;
    let mut decoder = codec_ctx.decoder().video()?;
    let src_w = decoder.width();
    let src_h = decoder.height();
    let src_fmt = decoder.format();

    let audio_idx = input_ctx
        .streams()
        .best(ffmpeg::media::Type::Audio)
        .map(|s| s.index());

    // -- Compute crop geometry --
    let crop_w = (((src_h as f64 * 9.0 / 16.0).round() as u32) & !1).max(2);
    let crop_h = src_h;

    if crop_w >= src_w {
        bail!(
            "source video ({}x{}) is too narrow to crop to 9:16 — need at least {}px width",
            src_w,
            src_h,
            crop_w + 2
        );
    }

    // -- Output context --
    let mut output_ctx = ffmpeg::format::output(output)?;

    // Video encoder (H.264).
    let h264 = ffmpeg::encoder::find(ffmpeg::codec::Id::H264)
        .context("H264 encoder not found — ensure ffmpeg was built with libx264")?;
    let out_video_idx;
    {
        let ost = output_ctx.add_stream(h264)?;
        out_video_idx = ost.index();
    }

    let enc_ctx = ffmpeg::codec::context::Context::new_with_codec(h264);
    let mut video_enc_cfg = enc_ctx.encoder().video()?;
    video_enc_cfg.set_width(MOBILE_WIDTH);
    video_enc_cfg.set_height(MOBILE_HEIGHT);
    video_enc_cfg.set_format(ffmpeg::format::Pixel::YUV420P);
    video_enc_cfg.set_time_base(video_tb);
    video_enc_cfg.set_frame_rate(decoder.frame_rate());
    video_enc_cfg.set_bit_rate(4_000_000);

    // Disable B-frames to avoid DTS/PTS reordering issues with the MP4 muxer.
    unsafe {
        (*video_enc_cfg.as_mut_ptr()).max_b_frames = 0;
    }

    if output_ctx
        .format()
        .flags()
        .contains(ffmpeg::format::Flags::GLOBAL_HEADER)
    {
        unsafe {
            (*video_enc_cfg.as_mut_ptr()).flags |=
                ffmpeg::ffi::AV_CODEC_FLAG_GLOBAL_HEADER as i32;
        }
    }

    let mut video_enc = video_enc_cfg.open_as(h264)?;

    // Copy encoder parameters to the output stream.
    unsafe {
        let stream_ptr = output_ctx.stream(out_video_idx).unwrap().as_ptr() as *mut ffmpeg::ffi::AVStream;
        ffmpeg::ffi::avcodec_parameters_from_context(
            (*stream_ptr).codecpar,
            video_enc.as_ptr(),
        );
    }

    // Audio stream (copy).
    let mut out_audio_idx = None;
    if let Some(a_idx) = audio_idx {
        let a_stream = input_ctx.stream(a_idx).unwrap();
        let mut out_audio =
            output_ctx.add_stream(ffmpeg::encoder::find(ffmpeg::codec::Id::None))?;
        out_audio.set_parameters(a_stream.parameters());
        unsafe {
            (*out_audio.parameters().as_mut_ptr()).codec_tag = 0;
        }
        out_audio_idx = Some(out_audio.index());
    }

    output_ctx
        .write_header()
        .context("failed to write output header")?;

    // Get time bases after write_header (muxer may adjust them).
    let video_out_tb = output_ctx.stream(out_video_idx).unwrap().time_base();
    let audio_out_tb = out_audio_idx.map(|idx| output_ctx.stream(idx).unwrap().time_base());
    let enc_tb = video_enc.time_base();

    // Frame counter for monotonic PTS.
    let mut frame_count: i64 = 0;
    // Compute the PTS increment per frame in the encoder's time base.
    let frame_rate = decoder.frame_rate();
    let pts_increment = if frame_rate.is_some() && frame_rate.unwrap().0 > 0 {
        let fr = frame_rate.unwrap();
        // enc time base is video_tb; increment = time_base_den * fr_den / (time_base_num * fr_num)
        // but simpler: just use 1 frame = 1/fps in encoder time base ticks
        let fps = fr.0 as f64 / fr.1 as f64;
        (enc_tb.1 as f64 / (enc_tb.0 as f64 * fps)).round() as i64
    } else {
        1
    };

    // -- Scalers --
    // 1. Source format -> YUV420P at original resolution.
    let mut yuv_scaler = ffmpeg::software::scaling::Context::get(
        src_fmt,
        src_w,
        src_h,
        ffmpeg::format::Pixel::YUV420P,
        src_w,
        src_h,
        ffmpeg::software::scaling::Flags::BILINEAR,
    )?;

    // 2. Cropped YUV420P -> 1080x1920.
    let mut resize_scaler = ffmpeg::software::scaling::Context::get(
        ffmpeg::format::Pixel::YUV420P,
        crop_w,
        crop_h,
        ffmpeg::format::Pixel::YUV420P,
        MOBILE_WIDTH,
        MOBILE_HEIGHT,
        ffmpeg::software::scaling::Flags::BILINEAR,
    )?;

    // -- Process packets --
    for (stream, packet) in input_ctx.packets() {
        let idx = stream.index();

        if idx == video_idx {
            decoder.send_packet(&packet)?;
            let mut frame = ffmpeg::frame::Video::empty();
            while decoder.receive_frame(&mut frame).is_ok() {
                let pts = frame.pts().unwrap_or(0);
                let time = pts as f64 * f64::from(video_tb.0) / f64::from(video_tb.1);

                // Convert to YUV420P.
                let mut yuv =
                    ffmpeg::frame::Video::new(ffmpeg::format::Pixel::YUV420P, src_w, src_h);
                yuv_scaler.run(&frame, &mut yuv)?;

                // Compute crop X from face position.
                let ratio = x_ratio_at(positions, time);
                let center_x = (ratio * src_w as f64) as i32;
                let half_crop = crop_w as i32 / 2;
                let crop_x =
                    ((center_x - half_crop).clamp(0, src_w as i32 - crop_w as i32) as u32) & !1;

                // Crop.
                let cropped = crop_yuv420p(&yuv, crop_x, crop_w, crop_h);

                // Scale to mobile resolution.
                let mut scaled = ffmpeg::frame::Video::new(
                    ffmpeg::format::Pixel::YUV420P,
                    MOBILE_WIDTH,
                    MOBILE_HEIGHT,
                );
                resize_scaler.run(&cropped, &mut scaled)?;

                scaled.set_pts(Some(frame_count * pts_increment));
                scaled.set_kind(ffmpeg::picture::Type::None);
                frame_count += 1;

                // Encode.
                video_enc.send_frame(&scaled)?;
                receive_encoded_packets(
                    &mut video_enc,
                    &mut output_ctx,
                    out_video_idx,
                    enc_tb,
                    video_out_tb,
                )?;
            }
        } else if Some(idx) == audio_idx {
            if let Some(a_out) = out_audio_idx {
                let a_in_tb = stream.time_base();
                let a_out_tb = audio_out_tb.unwrap_or(a_in_tb);
                let mut pkt = packet.clone();
                pkt.set_stream(a_out);
                pkt.rescale_ts(a_in_tb, a_out_tb);
                pkt.set_position(-1);
                pkt.write_interleaved(&mut output_ctx)
                    .context("failed to write audio packet")?;
            }
        }
    }

    // Flush decoder.
    decoder.send_eof()?;
    {
        let mut frame = ffmpeg::frame::Video::empty();
        while decoder.receive_frame(&mut frame).is_ok() {
            let pts = frame.pts().unwrap_or(0);
            let time = pts as f64 * f64::from(video_tb.0) / f64::from(video_tb.1);

            let mut yuv = ffmpeg::frame::Video::new(ffmpeg::format::Pixel::YUV420P, src_w, src_h);
            yuv_scaler.run(&frame, &mut yuv)?;

            let ratio = x_ratio_at(positions, time);
            let center_x = (ratio * src_w as f64) as i32;
            let half_crop = crop_w as i32 / 2;
            let crop_x =
                ((center_x - half_crop).clamp(0, src_w as i32 - crop_w as i32) as u32) & !1;

            let cropped = crop_yuv420p(&yuv, crop_x, crop_w, crop_h);

            let mut scaled = ffmpeg::frame::Video::new(
                ffmpeg::format::Pixel::YUV420P,
                MOBILE_WIDTH,
                MOBILE_HEIGHT,
            );
            resize_scaler.run(&cropped, &mut scaled)?;

            scaled.set_pts(Some(frame_count * pts_increment));
            scaled.set_kind(ffmpeg::picture::Type::None);
            frame_count += 1;

            video_enc.send_frame(&scaled)?;
            receive_encoded_packets(
                &mut video_enc,
                &mut output_ctx,
                out_video_idx,
                enc_tb,
                video_out_tb,
            )?;
        }
    }

    // Flush encoder.
    video_enc.send_eof()?;
    receive_encoded_packets(
        &mut video_enc,
        &mut output_ctx,
        out_video_idx,
        enc_tb,
        video_out_tb,
    )?;

    output_ctx
        .write_trailer()
        .context("failed to write output trailer")?;

    Ok(())
}

/// Drain encoded packets from the encoder and write them to the output.
fn receive_encoded_packets(
    encoder: &mut ffmpeg::encoder::video::Encoder,
    output: &mut ffmpeg::format::context::Output,
    stream_idx: usize,
    enc_tb: ffmpeg::Rational,
    stream_tb: ffmpeg::Rational,
) -> Result<()> {
    let mut pkt = ffmpeg::Packet::empty();
    while encoder.receive_packet(&mut pkt).is_ok() {
        pkt.set_stream(stream_idx);
        pkt.rescale_ts(enc_tb, stream_tb);
        pkt.write_interleaved(output)
            .context("failed to write encoded packet")?;
    }
    Ok(())
}

/// Crop a YUV420P frame horizontally. Returns a new frame with dimensions crop_w x crop_h.
fn crop_yuv420p(
    src: &ffmpeg::frame::Video,
    crop_x: u32,
    crop_w: u32,
    crop_h: u32,
) -> ffmpeg::frame::Video {
    let mut dst = ffmpeg::frame::Video::new(ffmpeg::format::Pixel::YUV420P, crop_w, crop_h);
    let cx = crop_x as usize;
    let cw = crop_w as usize;
    let ch = crop_h as usize;

    // Y plane (full resolution).
    {
        let src_stride = src.stride(0);
        let dst_stride = dst.stride(0);
        let src_data = src.data(0);
        let dst_data = dst.data_mut(0);
        for row in 0..ch {
            let s = row * src_stride + cx;
            let d = row * dst_stride;
            dst_data[d..d + cw].copy_from_slice(&src_data[s..s + cw]);
        }
    }

    // U and V planes (half resolution for YUV420P).
    for plane in 1..=2 {
        let src_stride = src.stride(plane);
        let dst_stride = dst.stride(plane);
        let src_data = src.data(plane);
        let dst_data = dst.data_mut(plane);
        let half_cx = cx / 2;
        let half_cw = cw / 2;
        let half_ch = ch / 2;
        for row in 0..half_ch {
            let s = row * src_stride + half_cx;
            let d = row * dst_stride;
            dst_data[d..d + half_cw].copy_from_slice(&src_data[s..s + half_cw]);
        }
    }

    dst
}
