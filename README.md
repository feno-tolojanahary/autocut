# clip-cli

A Rust CLI tool for cutting MP4 video files. First prototype of a larger video editor project.

## Prerequisites

**FFmpeg development libraries** and **CMake** must be installed on your system before building. The `whisper-rs` crate compiles whisper.cpp from source, which requires CMake and a C++ compiler.

### Windows

Option A: Download pre-built shared libraries from [gyan.dev](https://www.gyan.dev/ffmpeg/builds/) and set `FFMPEG_DIR` to the extracted folder. You also need LLVM/Clang for bindgen — set `LIBCLANG_PATH` to the directory containing `libclang.dll`.

```
set FFMPEG_DIR=C:\path\to\ffmpeg-shared
set LIBCLANG_PATH=C:\path\to\llvm\bin
```

Option B: Install via vcpkg:
```
vcpkg install ffmpeg:x64-windows
```

### Linux (Debian/Ubuntu)

```bash
sudo apt install libavcodec-dev libavformat-dev libavutil-dev libavdevice-dev libavfilter-dev libswresample-dev libswscale-dev libclang-dev pkg-config
```

### macOS

```bash
brew install ffmpeg
```

## Build

```bash
cargo build --release
```

The binary will be at `target/release/clip-cli` (or `clip-cli.exe` on Windows).

**Note on Windows:** The FFmpeg shared DLLs (e.g., `avcodec-62.dll`, `swscale-9.dll`) must be in your PATH or next to the executable at runtime.

## Usage

### trim — Keep one section

```bash
clip-cli trim --input in.mp4 --output out.mp4 --start 00:00:10 --end 00:00:30
```

### split — Cut into multiple pieces

```bash
clip-cli split --input in.mp4 --output-dir ./parts --at 00:00:10 --at 00:00:25
```

Output files are numbered: `part_001.mp4`, `part_002.mp4`, etc.

### remove — Cut out a section

```bash
clip-cli remove --input in.mp4 --output out.mp4 --start 00:00:10 --end 00:00:20
```

### transcribe — Speech to text

```bash
clip-cli transcribe --input in.mp4 --output transcript --model ggml-base.en.bin
```

Produces `transcript.txt` (plain text) and `transcript.srt` (subtitles with timestamps).

Options:
- `--language en` — Set language (omit for auto-detection)
- `--start 00:01:00 --end 00:05:00` — Transcribe only a portion

**Whisper model required:** Download a GGML model from [huggingface.co/ggerganov/whisper.cpp](https://huggingface.co/ggerganov/whisper.cpp/tree/main). Recommended models:
- `ggml-tiny.en.bin` (75 MB) — fastest, English only
- `ggml-base.en.bin` (142 MB) — good balance
- `ggml-medium.bin` (1.5 GB) — best accuracy, multilingual

### summarize — Auto-extract important sections

```bash
clip-cli summarize --input in.mp4 --output summary.mp4 \
  --whisper-model models/ggml-tiny.en.bin \
  --llm-model mistral
```

Transcribes the video, uses a local LLM to identify the most important sections, and stitches them into a summary video.

Options:
- `--llm-model mistral` — Ollama model name (default: `mistral`)
- `--max-segments 5` — Maximum number of sections to extract (default: 5)
- `--language en` — Set language for transcription

**Requires [Ollama](https://ollama.com) running locally.** Install and pull a model:
```bash
ollama pull mistral
ollama serve
```

### Timestamp formats

All commands accept timestamps in these formats:

- `HH:MM:SS` — e.g., `00:01:30`
- `HH:MM:SS.mmm` — e.g., `00:01:30.500`
- Raw seconds — e.g., `90`, `15.5`

## Running tests

A 30-second test fixture must exist at `tests/fixtures/test_30s.mp4`. Generate one with:

```bash
ffmpeg -y -f lavfi -i "testsrc=duration=30:size=320x240:rate=25" \
  -f lavfi -i "sine=frequency=440:duration=30" \
  -c:v libx264 -c:a aac -shortest tests/fixtures/test_30s.mp4
```

Then run:

```bash
cargo test
```

## Architecture

```
src/
  main.rs          — entry point, wires CLI to core
  cli.rs           — clap argument definitions
  core/
    mod.rs         — module exports
    types.rs       — domain types: Clip, Timeline, VideoOperation trait
    timestamp.rs   — timestamp parsing helper
    operations.rs  — TrimOp, SplitOp, RemoveOp implementations
    validation.rs  — input file and duration validation
    engine.rs      — ffmpeg-next rendering engine (stream copy)
    transcribe.rs  — audio extraction + whisper-rs speech-to-text
    llm.rs         — local LLM inference via Ollama + response parsing
    summarize.rs   — orchestrator: transcribe → LLM analysis → render
```

The `VideoOperation` trait returns `Vec<(PathBuf, Timeline)>` from its `plan()` method. Each `Timeline` is a sequence of `Clip`s (time ranges within a source file). The engine renders each timeline to its output path using packet-level stream copy (no re-encoding) for speed. New operations (concat, overlay, effects) can be added by implementing `VideoOperation`.

The `transcribe` command uses a separate code path: it decodes audio via ffmpeg-next, resamples to 16kHz mono f32, and feeds the samples to whisper-rs for inference.

The `summarize` command chains transcription with local LLM analysis (via Ollama's HTTP API) to identify important sections, then builds a multi-clip `Timeline` and renders it through the standard engine.
