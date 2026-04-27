use anyhow::{bail, Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};

use super::transcribe::Segment;

const DEFAULT_OLLAMA_URL: &str = "http://localhost:11434";

/// A section of the video identified as important by the LLM.
#[derive(Debug, Clone)]
pub struct ImportantSection {
    pub start_secs: f64,
    pub end_secs: f64,
    pub reason: String,
}

#[derive(Serialize)]
struct OllamaRequest {
    model: String,
    prompt: String,
    stream: bool,
    options: OllamaOptions,
}

#[derive(Serialize)]
struct OllamaOptions {
    temperature: f64,
    top_p: f64,
    num_predict: i32,
}

#[derive(Deserialize)]
struct OllamaResponse {
    response: String,
}

/// Analyze transcript segments with a local LLM (via Ollama) and return the most important sections.
pub fn extract_important_sections(
    model_name: &str,
    segments: &[Segment],
    max_segments: usize,
    video_duration: f64,
) -> Result<Vec<ImportantSection>> {
    if segments.is_empty() {
        bail!("no speech detected in video — cannot summarize");
    }

    let transcript = condense_transcript(segments);
    let prompt = build_prompt(&transcript, max_segments, video_duration);

    println!("[clip-cli] Running LLM inference via Ollama (this may take a while)...");
    let response = run_inference(model_name, &prompt)?;

    println!("model response: {}", response);

    let sections = parse_sections(&response, video_duration)?;

    if sections.is_empty() {
        bail!(
            "LLM did not return any valid sections. Raw response:\n{}",
            response
        );
    }

    Ok(sections)
}

/// Condense transcript segments into a compact format for the LLM prompt.
/// Uses `[MM:SS] text` format. For long transcripts, subsamples to stay
/// within the LLM's context window.
fn condense_transcript(segments: &[Segment]) -> String {
    let max_entries = 400;
    let stride = if segments.len() > max_entries {
        segments.len() / max_entries
    } else {
        1
    };

    let mut out = String::new();
    for (i, seg) in segments.iter().enumerate() {
        if i % stride != 0 {
            continue;
        }
        let secs = seg.start_ms as f64 / 1000.0;
        out.push_str(&format!("[{secs:.1}] {}\n", seg.text));
    }
    out
}

/// Build the prompt for the LLM.
fn build_prompt(transcript: &str, max_segments: usize, video_duration: f64) -> String {
    format!(
        r#"You are a video editor assistant. You will receive a transcript with timestamps. Your job is to identify the {max_segments} most important and interesting sections of the video.

IMPORTANT: Only select sections where a person is clearly speaking meaningful phrases or sentences. Exclude any section that contains only background sounds, music, noise, laughter, applause, silence, or non-verbal audio (e.g. "[music]", "[applause]", "[silence]", "[noise]"). Every selected section MUST contain actual spoken words from a person.

Reply with ONLY a list of time ranges, one per line, in this exact format:
START_SECONDS - END_SECONDS | reason

Example:
12.5 - 45.0 | Speaker introduces the main thesis
120.0 - 185.5 | Key demonstration of the technique

Rules:
- Use decimal seconds (not HH:MM:SS)
- Each section must be at least 5 seconds long
- Sections must not overlap
- Order sections by start time
- Do not exceed {max_segments} sections
- The video is {video_duration:.1} seconds long; do not exceed this
- Only include sections with clear human speech — skip sounds, music, and noise
- Output NOTHING else — no preamble, no summary, just the lines

Here is the transcript:

{transcript}"#
    )
}

/// Call Ollama's local API to run inference.
fn run_inference(model_name: &str, prompt: &str) -> Result<String> {
    let url = format!("{DEFAULT_OLLAMA_URL}/api/generate");

    let request_body = OllamaRequest {
        model: model_name.to_string(),
        prompt: prompt.to_string(),
        stream: false,
        options: OllamaOptions {
            temperature: 0.1,
            top_p: 0.9,
            num_predict: 1024,
        },
    };

    let response: OllamaResponse = ureq::post(&url)
        .send_json(&request_body)
        .context("failed to connect to Ollama — is it running? Start it with: ollama serve")?
        .body_mut()
        .read_json()
        .context("failed to parse Ollama response")?;

    Ok(response.response.trim().to_string())
}

/// Check that Ollama is reachable and the model is available.
pub fn check_ollama(model_name: &str) -> Result<()> {
    // Check connectivity
    let url = format!("{DEFAULT_OLLAMA_URL}/api/tags");
    let resp: serde_json::Value = ureq::get(&url)
        .call()
        .context("cannot reach Ollama at localhost:11434 — is it running? Start it with: ollama serve")?
        .body_mut()
        .read_json()
        .context("unexpected Ollama response")?;

    // Check if model is available
    if let Some(models) = resp.get("models").and_then(|m| m.as_array()) {
        let available = models.iter().any(|m| {
            m.get("name")
                .and_then(|n| n.as_str())
                .map(|n| n.starts_with(model_name))
                .unwrap_or(false)
        });
        if !available {
            let names: Vec<&str> = models
                .iter()
                .filter_map(|m| m.get("name").and_then(|n| n.as_str()))
                .collect();
            bail!(
                "model '{}' not found in Ollama. Available: {:?}\nPull it with: ollama pull {}",
                model_name,
                names,
                model_name
            );
        }
    }

    Ok(())
}

/// Parse the LLM response into validated ImportantSection entries.
fn parse_sections(response: &str, video_duration: f64) -> Result<Vec<ImportantSection>> {
    // Match lines like: "12.5 - 45.0 | reason" or "1. 12.5 - 45.0 | reason"
    let re = Regex::new(r"^\s*(?:\d+\.\s+)?(\d+\.?\d*)\s*-\s*(\d+\.?\d*)\s*\|\s*(.+)$")
        .context("failed to compile regex")?;

    let mut sections: Vec<ImportantSection> = Vec::new();

    for line in response.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(caps) = re.captures(line) {
            let start: f64 = caps[1].parse().unwrap_or(-1.0);
            let end: f64 = caps[2].parse().unwrap_or(-1.0);
            let reason = caps[3].trim().to_string();

            if start < 0.0 || end < 0.0 {
                continue;
            }
            if start >= end {
                continue;
            }
            if end - start < 1.0 {
                continue;
            }

            // Clamp to video bounds
            let start = start.max(0.0);
            let end = end.min(video_duration);

            if start >= end {
                continue;
            }

            sections.push(ImportantSection {
                start_secs: start,
                end_secs: end,
                reason,
            });
        }
    }

    // Sort by start time
    sections.sort_by(|a, b| a.start_secs.partial_cmp(&b.start_secs).unwrap());

    // Remove overlapping sections (keep earlier one)
    let mut filtered:
    
    Vec<ImportantSection> = Vec::new();
    for section in sections {
        if let Some(last) = filtered.last() {
            if section.start_secs < last.end_secs {
                continue;
            }
        }
        filtered.push(section);
    }

    Ok(filtered)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_sections_valid() {
        let response = "12.5 - 45.0 | Main introduction\n120.0 - 185.5 | Key demo\n";
        let sections = parse_sections(response, 300.0).unwrap();
        assert_eq!(sections.len(), 2);
        assert!((sections[0].start_secs - 12.5).abs() < 1e-6);
        assert!((sections[0].end_secs - 45.0).abs() < 1e-6);
        assert_eq!(sections[0].reason, "Main introduction");
        assert!((sections[1].start_secs - 120.0).abs() < 1e-6);
    }

    #[test]
    fn test_parse_sections_with_preamble() {
        let response = "Here are the sections:\n\n12.5 - 45.0 | Introduction\nSome garbage\n50.0 - 80.0 | Conclusion\n";
        let sections = parse_sections(response, 300.0).unwrap();
        assert_eq!(sections.len(), 2);
    }

    #[test]
    fn test_parse_sections_clamps_to_duration() {
        let response = "0.0 - 500.0 | Everything\n";
        let sections = parse_sections(response, 200.0).unwrap();
        assert_eq!(sections.len(), 1);
        assert!((sections[0].end_secs - 200.0).abs() < 1e-6);
    }

    #[test]
    fn test_parse_sections_removes_overlaps() {
        let response = "10.0 - 30.0 | First\n20.0 - 40.0 | Overlapping\n50.0 - 60.0 | Third\n";
        let sections = parse_sections(response, 300.0).unwrap();
        assert_eq!(sections.len(), 2);
        assert!((sections[0].start_secs - 10.0).abs() < 1e-6);
        assert!((sections[1].start_secs - 50.0).abs() < 1e-6);
    }

    #[test]
    fn test_parse_sections_rejects_invalid() {
        let response = "50.0 - 30.0 | Reversed\n0.5 - 1.0 | Too short\n";
        let sections = parse_sections(response, 300.0).unwrap();
        assert_eq!(sections.len(), 0);
    }

    #[test]
    fn test_condense_transcript() {
        let segments = vec![
            Segment { start_ms: 0, end_ms: 5000, text: "Hello".to_string() },
            Segment { start_ms: 5000, end_ms: 10000, text: "World".to_string() },
            Segment { start_ms: 65000, end_ms: 70000, text: "Later".to_string() },
        ];
        let condensed = condense_transcript(&segments);
        assert!(condensed.contains("[0.0] Hello"));
        assert!(condensed.contains("[5.0] World"));
        assert!(condensed.contains("[65.0] Later"));
    }
}
