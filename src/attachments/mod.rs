//! Attachment processing for multimodal messages
//!
//! Processes images, audio, and other attachments to augment message context

mod pdf;
mod video;
mod vision;

use std::sync::Arc;

use synapse_client::SynapseClient;

use crate::Result;
use crate::channels::{Attachment, AttachmentKind};

pub use vision::VisionClient;

/// Processes attachments and returns text descriptions/transcriptions
///
/// Vision and transcription run only on the gateway client the caller passes
/// for the turn (the user's product-scoped key in cloud mode, the operator's
/// gateway when self-hosted), so attachment AI is metered and charged exactly
/// like chat. Without a gateway client, media is left as a placeholder; PDF
/// text extraction is local and always runs
pub struct AttachmentProcessor {
    /// Gateway model for image and keyframe descriptions
    vision_model: String,
    /// STT model identifier for gateway transcription
    stt_model: String,
    /// Operator's own gateway for channel messages that carry no Omni
    /// identity; set only on self-hosted instances, never in cloud mode
    operator_gateway: Option<Arc<SynapseClient>>,
    /// HTTP client for downloading attachments
    client: reqwest::Client,
    /// Maximum keyframes to extract from video
    max_video_frames: u32,
}

/// The gateway client one turn's attachment AI runs on
#[derive(Clone, Copy)]
struct Turn<'a> {
    vision: Option<&'a VisionClient>,
    synapse: Option<&'a Arc<SynapseClient>>,
}

impl AttachmentProcessor {
    /// Create a new attachment processor
    #[must_use]
    pub fn new(vision_model: String, stt_model: String) -> Self {
        Self {
            vision_model,
            stt_model,
            operator_gateway: None,
            client: reqwest::Client::new(),
            max_video_frames: 3,
        }
    }

    /// Set the maximum number of video keyframes to extract
    #[must_use]
    pub const fn with_max_video_frames(mut self, max_frames: u32) -> Self {
        self.max_video_frames = max_frames;
        self
    }

    /// Set the operator's gateway for channel messages without an Omni
    /// identity (self-hosted only)
    #[must_use]
    pub fn with_operator_gateway(mut self, gateway: Option<Arc<SynapseClient>>) -> Self {
        self.operator_gateway = gateway;
        self
    }

    /// The operator's gateway, if this is a self-hosted instance
    #[must_use]
    pub const fn operator_gateway(&self) -> Option<&Arc<SynapseClient>> {
        self.operator_gateway.as_ref()
    }

    /// Process all attachments and return augmented text
    ///
    /// `gateway` is the client this turn's AI runs on; `None` leaves images,
    /// audio, and video as placeholders
    ///
    /// # Errors
    ///
    /// Returns error if attachment processing fails
    pub async fn process_attachments(
        &self,
        attachments: &[Attachment],
        gateway: Option<&Arc<SynapseClient>>,
    ) -> Result<String> {
        if attachments.is_empty() {
            return Ok(String::new());
        }

        let vision = gateway.map(|client| {
            VisionClient::from_synapse(Arc::clone(client)).with_model(self.vision_model.clone())
        });
        let turn = Turn {
            vision: vision.as_ref(),
            synapse: gateway,
        };

        let mut parts = Vec::new();

        for attachment in attachments {
            let description = self.process_single(attachment, turn).await;
            parts.push(description);
        }

        Ok(parts.join("\n"))
    }

    /// Process a single attachment
    async fn process_single(&self, attachment: &Attachment, turn: Turn<'_>) -> String {
        match attachment.kind {
            AttachmentKind::Image => self.process_image(attachment, turn.vision).await,
            AttachmentKind::Audio => self.process_audio(attachment, turn.synapse).await,
            AttachmentKind::Video => self.process_video(attachment, turn.vision).await,
            AttachmentKind::File => self.process_file(attachment).await,
        }
    }

    /// Process an image attachment using vision
    async fn process_image(
        &self,
        attachment: &Attachment,
        vision: Option<&VisionClient>,
    ) -> String {
        let Some(vision) = vision else {
            return format!(
                "[Image: {}]",
                attachment.filename.as_deref().unwrap_or("image")
            );
        };

        // Get image data
        let image_data = match self.get_attachment_data(attachment).await {
            Ok(data) => data,
            Err(e) => {
                tracing::warn!(error = %e, "failed to download image");
                return format!(
                    "[Image: {} (could not download)]",
                    attachment.filename.as_deref().unwrap_or("image")
                );
            }
        };

        // Analyze with vision
        match vision
            .describe_image(&image_data, &attachment.mime_type)
            .await
        {
            Ok(description) => {
                format!(
                    "[Image: {}]\n{}",
                    attachment.filename.as_deref().unwrap_or("image"),
                    description
                )
            }
            Err(e) => {
                tracing::warn!(error = %e, "vision analysis failed");
                format!(
                    "[Image: {} (analysis failed)]",
                    attachment.filename.as_deref().unwrap_or("image")
                )
            }
        }
    }

    /// Process an audio attachment using Synapse STT
    async fn process_audio(
        &self,
        attachment: &Attachment,
        synapse: Option<&Arc<SynapseClient>>,
    ) -> String {
        let Some(synapse) = synapse else {
            return format!(
                "[Audio: {}]",
                attachment.filename.as_deref().unwrap_or("audio")
            );
        };

        // Get audio data
        let audio_data = match self.get_attachment_data(attachment).await {
            Ok(data) => data,
            Err(e) => {
                tracing::warn!(error = %e, "failed to download audio");
                return format!(
                    "[Audio: {} (could not download)]",
                    attachment.filename.as_deref().unwrap_or("audio")
                );
            }
        };

        // Convert to WAV if needed and transcribe
        let wav_data = match convert_to_wav(&audio_data, &attachment.mime_type) {
            Ok(data) => data,
            Err(e) => {
                tracing::warn!(error = %e, "audio conversion failed");
                return format!(
                    "[Audio: {} (unsupported format)]",
                    attachment.filename.as_deref().unwrap_or("audio")
                );
            }
        };

        let filename = attachment.filename.as_deref().unwrap_or("audio.wav");
        match synapse
            .transcribe(wav_data.into(), filename, &self.stt_model)
            .await
        {
            Ok(result) => {
                format!(
                    "[Audio transcription: {}]\n\"{}\"",
                    attachment.filename.as_deref().unwrap_or("audio"),
                    result.text
                )
            }
            Err(e) => {
                tracing::warn!(error = %e, "transcription failed");
                format!(
                    "[Audio: {} (transcription failed)]",
                    attachment.filename.as_deref().unwrap_or("audio")
                )
            }
        }
    }

    /// Process a video attachment by extracting keyframes and analyzing with vision
    async fn process_video(
        &self,
        attachment: &Attachment,
        vision: Option<&VisionClient>,
    ) -> String {
        let filename = attachment.filename.as_deref().unwrap_or("video");

        let Some(vision) = vision else {
            return format!("[Video: {filename}]");
        };

        // Get video data
        let video_data = match self.get_attachment_data(attachment).await {
            Ok(data) => data,
            Err(e) => {
                tracing::warn!(error = %e, "failed to download video");
                return format!("[Video: {filename} (could not download)]");
            }
        };

        // Extract keyframes
        let frames = match video::extract_keyframes(&video_data, self.max_video_frames).await {
            Ok(frames) if !frames.is_empty() => frames,
            Ok(_) => {
                tracing::debug!("no keyframes extracted from video");
                return format!("[Video: {filename}]");
            }
            Err(e) => {
                tracing::warn!(error = %e, "keyframe extraction failed");
                return format!("[Video: {filename}]");
            }
        };

        // Analyze each keyframe with vision
        let mut descriptions = Vec::new();
        for (i, frame_data) in frames.iter().enumerate() {
            match vision.describe_image(frame_data, "image/jpeg").await {
                Ok(desc) => descriptions.push(format!("Frame {}: {desc}", i + 1)),
                Err(e) => {
                    tracing::warn!(frame = i, error = %e, "vision analysis failed for keyframe");
                }
            }
        }

        if descriptions.is_empty() {
            return format!("[Video: {filename}]");
        }

        format!("[Video: {filename}]\n{}", descriptions.join("\n"))
    }

    /// Process a generic file attachment, extracting text from PDFs
    async fn process_file(&self, attachment: &Attachment) -> String {
        let filename = attachment.filename.as_deref().unwrap_or("file");

        if pdf::is_pdf_attachment(&attachment.mime_type) {
            let data = match self.get_attachment_data(attachment).await {
                Ok(data) => data,
                Err(e) => {
                    tracing::warn!(error = %e, "failed to download PDF");
                    return format!("[PDF: {filename} (could not download)]");
                }
            };

            match pdf::extract_pdf_text(&data) {
                Ok(text) => format!("[PDF: {filename}]\n{text}"),
                Err(e) => {
                    tracing::warn!(error = %e, "PDF text extraction failed");
                    format!("[PDF: {filename} (extraction failed)]")
                }
            }
        } else {
            format!("[File: {filename} ({})]", attachment.mime_type)
        }
    }

    /// Get attachment data from URL or inline data
    async fn get_attachment_data(&self, attachment: &Attachment) -> Result<Vec<u8>> {
        // If we have inline data, use it
        if let Some(data) = &attachment.data {
            return Ok(data.clone());
        }

        // Otherwise download from URL
        let url = attachment
            .url
            .as_ref()
            .ok_or_else(|| crate::Error::Attachment("No URL or data for attachment".to_string()))?;

        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| crate::Error::Attachment(format!("Download failed: {e}")))?;

        if !response.status().is_success() {
            return Err(crate::Error::Attachment(format!(
                "Download failed: {}",
                response.status()
            )));
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| crate::Error::Attachment(format!("Read failed: {e}")))?;

        Ok(bytes.to_vec())
    }
}

/// Convert audio to WAV format for STT
fn convert_to_wav(data: &[u8], mime_type: &str) -> Result<Vec<u8>> {
    match mime_type {
        "audio/wav" | "audio/wave" | "audio/x-wav" => Ok(data.to_vec()),
        "audio/mpeg" | "audio/mp3" => convert_mp3_to_wav(data),
        "audio/ogg" | "audio/opus" => {
            // OGG/Opus conversion would require additional dependencies
            Err(crate::Error::Attachment(
                "OGG/Opus conversion not yet supported".to_string(),
            ))
        }
        _ => {
            // Try to use as-is and let the gateway STT provider handle it
            Ok(data.to_vec())
        }
    }
}

/// Convert MP3 to WAV using minimp3
#[allow(clippy::cast_sign_loss)]
fn convert_mp3_to_wav(mp3_data: &[u8]) -> Result<Vec<u8>> {
    use crate::voice::samples_to_wav;

    let mut decoder = minimp3::Decoder::new(mp3_data);
    let mut samples: Vec<f32> = Vec::new();
    let mut sample_rate = 16000_u32;

    loop {
        match decoder.next_frame() {
            Ok(frame) => {
                sample_rate = frame.sample_rate as u32;
                // Convert to mono f32 if stereo
                if frame.channels == 2 {
                    for chunk in frame.data.chunks(2) {
                        let mono =
                            f32::midpoint(f32::from(chunk[0]), f32::from(chunk[1])) / 32768.0;
                        samples.push(mono);
                    }
                } else {
                    for &s in &frame.data {
                        samples.push(f32::from(s) / 32768.0);
                    }
                }
            }
            Err(minimp3::Error::Eof) => break,
            Err(e) => {
                return Err(crate::Error::Attachment(format!("MP3 decode error: {e}")));
            }
        }
    }

    // Resample to 16kHz if needed
    let resampled = if sample_rate == 16000 {
        samples
    } else {
        resample_audio(&samples, sample_rate, 16000)?
    };

    samples_to_wav(&resampled, 16000)
}

/// Resample audio using rubato
#[allow(clippy::cast_possible_truncation)]
fn resample_audio(samples: &[f32], from_rate: u32, to_rate: u32) -> Result<Vec<f32>> {
    use rubato::{FftFixedIn, Resampler};

    let chunk_size = 1024;
    let sub_chunks = 2;

    let mut resampler = FftFixedIn::<f64>::new(
        from_rate as usize,
        to_rate as usize,
        chunk_size,
        sub_chunks,
        1,
    )
    .map_err(|e| crate::Error::Attachment(format!("Resampler init failed: {e}")))?;

    // Convert to f64
    let input: Vec<f64> = samples.iter().map(|&s| f64::from(s)).collect();

    let mut output = Vec::new();

    for chunk in input.chunks(chunk_size) {
        if chunk.len() == chunk_size {
            let result = resampler
                .process(&[chunk.to_vec()], None)
                .map_err(|e| crate::Error::Attachment(format!("Resample failed: {e}")))?;
            output.extend_from_slice(&result[0]);
        }
    }

    // Convert back to f32
    Ok(output.iter().map(|&s| s as f32).collect())
}
