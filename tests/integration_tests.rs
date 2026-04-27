use std::path::Path;
use std::process::Command;

fn clip_cli() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_clip-cli"));
    // Ensure ffmpeg DLLs are findable at runtime.
    if let Ok(ffmpeg_dir) = std::env::var("FFMPEG_DIR") {
        let bin_dir = Path::new(&ffmpeg_dir).join("bin");
        let current_path = std::env::var("PATH").unwrap_or_default();
        cmd.env("PATH", format!("{};{}", bin_dir.display(), current_path));
    }
    cmd
}

fn fixture_path() -> &'static str {
    "tests/fixtures/test_30s.mp4"
}

fn assert_file_exists_and_nonzero(path: &Path) {
    assert!(path.exists(), "output file should exist: {}", path.display());
    let meta = std::fs::metadata(path).unwrap();
    assert!(meta.len() > 0, "output file should not be empty");
}

#[test]
fn test_trim() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("trimmed.mp4");

    let status = clip_cli()
        .args([
            "trim",
            "--input", fixture_path(),
            "--output", output.to_str().unwrap(),
            "--start", "00:00:05",
            "--end", "00:00:15",
        ])
        .status()
        .expect("failed to run clip-cli");

    assert!(status.success(), "trim should succeed");
    assert_file_exists_and_nonzero(&output);
}

#[test]
fn test_split() {
    let dir = tempfile::tempdir().unwrap();
    let output_dir = dir.path().join("parts");
    std::fs::create_dir_all(&output_dir).unwrap();

    let status = clip_cli()
        .args([
            "split",
            "--input", fixture_path(),
            "--output-dir", output_dir.to_str().unwrap(),
            "--at", "00:00:10",
            "--at", "00:00:20",
        ])
        .status()
        .expect("failed to run clip-cli");

    assert!(status.success(), "split should succeed");
    assert_file_exists_and_nonzero(&output_dir.join("part_001.mp4"));
    assert_file_exists_and_nonzero(&output_dir.join("part_002.mp4"));
    assert_file_exists_and_nonzero(&output_dir.join("part_003.mp4"));
}

#[test]
fn test_remove() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("removed.mp4");

    let status = clip_cli()
        .args([
            "remove",
            "--input", fixture_path(),
            "--output", output.to_str().unwrap(),
            "--start", "00:00:10",
            "--end", "00:00:20",
        ])
        .status()
        .expect("failed to run clip-cli");

    assert!(status.success(), "remove should succeed");
    assert_file_exists_and_nonzero(&output);
}

#[test]
fn test_trim_invalid_timestamps() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("out.mp4");

    // start >= end
    let status = clip_cli()
        .args([
            "trim",
            "--input", fixture_path(),
            "--output", output.to_str().unwrap(),
            "--start", "15",
            "--end", "5",
        ])
        .status()
        .expect("failed to run clip-cli");

    assert!(!status.success(), "should fail when start >= end");
}

#[test]
fn test_trim_nonexistent_input() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("out.mp4");

    let status = clip_cli()
        .args([
            "trim",
            "--input", "nonexistent.mp4",
            "--output", output.to_str().unwrap(),
            "--start", "0",
            "--end", "5",
        ])
        .status()
        .expect("failed to run clip-cli");

    assert!(!status.success(), "should fail with nonexistent input");
}
