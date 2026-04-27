use anyhow::{bail, Result};

/// Parse a timestamp string into seconds.
/// Supported formats:
///   - Raw seconds: "15.5", "90"
///   - HH:MM:SS:    "00:01:30"
///   - HH:MM:SS.mmm: "00:01:30.500"
pub fn parse_timestamp(input: &str) -> Result<f64> {
    let input = input.trim();

    if let Some((time_part, millis_str)) = input.rsplit_once('.') {
        if time_part.contains(':') {
            // HH:MM:SS.mmm
            let base = parse_hms(time_part)?;
            let frac: f64 = format!("0.{millis_str}").parse().map_err(|_| {
                anyhow::anyhow!("invalid fractional seconds in timestamp: {input}")
            })?;
            return Ok(base + frac);
        }
    }

    if input.contains(':') {
        // HH:MM:SS
        return parse_hms(input);
    }

    // Raw seconds
    input
        .parse::<f64>()
        .map_err(|_| anyhow::anyhow!("invalid timestamp: {input}"))
}

fn parse_hms(s: &str) -> Result<f64> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 3 {
        bail!("expected HH:MM:SS format, got: {s}");
    }
    let h: f64 = parts[0]
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid hours in: {s}"))?;
    let m: f64 = parts[1]
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid minutes in: {s}"))?;
    let sec: f64 = parts[2]
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid seconds in: {s}"))?;
    Ok(h * 3600.0 + m * 60.0 + sec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_raw_seconds() {
        assert!((parse_timestamp("15.5").unwrap() - 15.5).abs() < 1e-6);
        assert!((parse_timestamp("90").unwrap() - 90.0).abs() < 1e-6);
    }

    #[test]
    fn test_hms() {
        assert!((parse_timestamp("00:01:30").unwrap() - 90.0).abs() < 1e-6);
        assert!((parse_timestamp("01:00:00").unwrap() - 3600.0).abs() < 1e-6);
    }

    #[test]
    fn test_hms_millis() {
        assert!((parse_timestamp("00:01:30.500").unwrap() - 90.5).abs() < 1e-6);
        assert!((parse_timestamp("00:00:10.250").unwrap() - 10.25).abs() < 1e-6);
    }

    #[test]
    fn test_invalid() {
        assert!(parse_timestamp("abc").is_err());
        assert!(parse_timestamp("00:01").is_err());
    }
}
