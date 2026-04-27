# clip-cli

Rust CLI tool for cutting MP4 video files. First prototype of a larger video editor project — prioritize clean, extensible architecture over feature breadth.

## Tech stack

- **Language:** Rust (edition 2021)
- **Video processing:** `ffmpeg-next` (Rust bindings to FFmpeg) — do NOT shell out to the ffmpeg binary
- **Speech-to-text:** `whisper-rs` (Rust bindings to whisper.cpp)
- **LLM inference:** Ollama local HTTP API via `ureq` (avoids GGML symbol conflicts with whisper-rs)
- **CLI parsing:** `clap` with derive feature
- **Error handling:** `anyhow`
- **Target format:** MP4 input/output only

## Architecture

```
src/
  main.rs          — entry point, wires CLI to core via run_operation()
  cli.rs           — clap argument definitions (Cli struct, Command enum)
  core/
    mod.rs         — module exports
    types.rs       — domain types: Clip, Timeline, VideoOperation trait
    timestamp.rs   — reusable timestamp parsing (HH:MM:SS, HH:MM:SS.mmm, raw seconds)
    operations.rs  — TrimOp, SplitOp, RemoveOp (implement VideoOperation)
    validation.rs  — input file validation and duration querying
    engine.rs      — ffmpeg-next rendering engine (packet-level stream copy, no re-encoding)
    transcribe.rs  — audio extraction + whisper-rs inference, outputs .txt and .srt
    llm.rs         — local LLM inference via Ollama HTTP API + response parsing
    summarize.rs   — orchestrator: transcribe → LLM analysis → render summary video
```

Key design pattern: the `VideoOperation` trait's `plan()` method returns `Vec<(PathBuf, Timeline)>`. Each `Timeline` is a sequence of `Clip`s (time ranges within a source file). The engine renders each timeline via stream copy. New operations (concat, overlay, effects) should be added by implementing `VideoOperation`.

The `transcribe` and `summarize` commands use separate code paths (not `VideoOperation`). `transcribe` decodes audio, resamples to 16kHz mono f32, and feeds samples to whisper-rs. `summarize` chains transcription with LLM analysis via Ollama, then builds a multi-clip Timeline and renders through the standard engine.

## Implemented subcommands

- **trim** — Keep one section, discard the rest (`--input`, `--output`, `--start`, `--end`)
- **split** — Cut into numbered parts at given timestamps (`--input`, `--output-dir`, `--at` repeatable)
- **remove** — Cut out a section and stitch remaining parts (`--input`, `--output`, `--start`, `--end`)
- **transcribe** — Extract speech to .txt and .srt files (`--input`, `--output`, `--model`, optional `--language`, `--start`, `--end`)
- **summarize** — Auto-extract important sections into a summary video (`--input`, `--output`, `--whisper-model`, `--llm-model` [default: mistral], optional `--language`, `--max-segments` [default: 5]). Requires Ollama running locally.

## Build requirements

- FFmpeg dev libraries must be installed (see README.md for platform-specific instructions)
- LLVM/Clang for bindgen (`LIBCLANG_PATH` on Windows)
- CMake + C++ compiler (for whisper.cpp compilation via whisper-rs)
- On Windows: `FFMPEG_DIR` env var pointing to FFmpeg shared libs; DLLs must be in PATH at runtime
- For `summarize`: Ollama must be installed and running (`ollama serve`), with the desired model pulled (`ollama pull mistral`)

## Testing

- Integration tests in `tests/integration_tests.rs` — one test per subcommand plus error cases
- Unit tests in `timestamp.rs`, `transcribe.rs`, and `llm.rs`
- Test fixture: `tests/fixtures/test_30s.mp4` (generate with ffmpeg testsrc command in README)
- Run with `cargo test`

## Conventions

- Keep subcommand handlers thin: parse args, build a domain object, call core logic
- Validate inputs (file exists, MP4 extension, timestamps within duration, start < end)
- Print progress messages prefixed with `[clip-cli]`
- Use `anyhow` for error propagation with `.context()` for actionable messages
