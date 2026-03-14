//! Video keyframe extraction via ffmpeg
//!
//! Extracts I-frames from video data for vision analysis.
//! Gracefully degrades if ffmpeg is not available.

use crate::{Error, Result};

/// Extract keyframes from video data using ffmpeg
///
/// Spawns ffmpeg to extract up to `max_frames` I-frames as JPEG images.
/// Returns raw JPEG bytes for each extracted frame.
///
/// # Errors
///
/// Returns error if ffmpeg execution fails or output is invalid
pub async fn extract_keyframes(data: &[u8], max_frames: u32) -> Result<Vec<Vec<u8>>> {
    if which::which("ffmpeg").is_err() {
        tracing::warn!("ffmpeg not found, skipping keyframe extraction");
        return Ok(Vec::new());
    }

    // Write video data to a temp file (ffmpeg needs seekable input for I-frame selection)
    let tmp_dir = tempfile::tempdir()
        .map_err(|e| Error::Attachment(format!("failed to create temp dir: {e}")))?;
    let input_path = tmp_dir.path().join("input");
    tokio::fs::write(&input_path, data)
        .await
        .map_err(|e| Error::Attachment(format!("failed to write temp video: {e}")))?;

    // Extract I-frames as individual JPEG files
    let output_pattern = tmp_dir.path().join("frame_%03d.jpg");
    let mut cmd = tokio::process::Command::new("ffmpeg");
    cmd.args(["-i", &input_path.display().to_string()]);
    cmd.args([
        "-vf",
        r"select=eq(pict_type\,I)",
        "-frames:v",
        &max_frames.to_string(),
        "-vsync",
        "vfr",
        "-q:v",
        "2",
    ]);
    cmd.arg(output_pattern.display().to_string());
    cmd.args(["-y", "-loglevel", "error"]);
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::piped());

    let output = tokio::time::timeout(std::time::Duration::from_secs(30), cmd.output())
        .await
        .map_err(|_| Error::Attachment("ffmpeg timed out (30s)".to_string()))?
        .map_err(|e| Error::Attachment(format!("ffmpeg execution failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::warn!(stderr = %stderr, "ffmpeg keyframe extraction failed");
        return Ok(Vec::new());
    }

    // Read extracted frames
    let mut frames = Vec::new();
    for i in 1..=max_frames {
        let frame_path = tmp_dir.path().join(format!("frame_{i:03}.jpg"));
        match tokio::fs::read(&frame_path).await {
            Ok(data) if !data.is_empty() => frames.push(data),
            _ => break,
        }
    }

    tracing::debug!(count = frames.len(), "extracted keyframes from video");
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn empty_data_returns_no_frames() {
        let frames = extract_keyframes(b"", 3).await.unwrap();
        assert!(frames.is_empty());
    }

    #[tokio::test]
    async fn missing_ffmpeg_returns_empty() {
        // If ffmpeg is not installed, should gracefully return empty
        // If it is installed, the empty input will produce no frames
        let frames = extract_keyframes(b"not-a-video", 3).await.unwrap();
        assert!(frames.is_empty());
    }
}
