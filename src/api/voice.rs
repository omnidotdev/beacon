//! Voice API endpoints for speech-to-text and text-to-speech

use std::sync::Arc;

use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use serde::{Deserialize, Serialize};
use synapse_client::SynapseClient;

use super::ApiState;
use super::managed::{self, ManagedError};
use crate::billing::gateway_error::{self, GatewayFailure};

/// Build voice router
pub fn router(state: Arc<ApiState>) -> Router {
    Router::new()
        .route("/transcribe", post(transcribe))
        .route("/synthesize", post(synthesize))
        .route("/capabilities", axum::routing::get(capabilities))
        .with_state(state)
}

/// Voice capabilities response
#[derive(Debug, Serialize)]
pub struct VoiceCapabilities {
    pub stt_available: bool,
    pub tts_available: bool,
}

/// Get voice capabilities
async fn capabilities(State(state): State<Arc<ApiState>>) -> Json<VoiceCapabilities> {
    let has_synapse = state.synapse.is_some();
    Json(VoiceCapabilities {
        stt_available: has_synapse,
        tts_available: has_synapse,
    })
}

/// Resolve the Synapse client for a voice request.
///
/// Cloud mode runs on the caller's product-scoped managed key (never the
/// shared client), so speech is charged to their pooled AI credits.
/// Self-hosted mode uses the local client. Returns the client and, in cloud
/// mode, the user it bills
async fn voice_client(
    state: &ApiState,
    headers: &HeaderMap,
    not_configured: &'static str,
) -> Result<(Arc<SynapseClient>, Option<String>), VoiceError> {
    if !state.cloud_mode {
        let synapse = state
            .synapse
            .clone()
            .ok_or(VoiceError::NotConfigured(not_configured))?;
        return Ok((synapse, None));
    }

    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(VoiceError::Unauthorized)?;
    let jwt_cache = state.jwt_cache.as_ref().ok_or(VoiceError::Unauthorized)?;
    let user_id = jwt_cache
        .validate(token)
        .await
        .map_err(|_| VoiceError::Unauthorized)?
        .sub;

    let managed = managed::managed_client(state, &user_id)
        .await
        .map_err(VoiceError::Managed)?;
    Ok((managed.client, Some(user_id)))
}

/// Map a failed speech call, giving gateway refusals their own outcome
async fn speech_failure(
    state: &ApiState,
    user_id: Option<&str>,
    error_text: String,
    fallback: fn(String) -> VoiceError,
) -> VoiceError {
    match gateway_error::classify(&error_text) {
        Some(failure) => {
            if let Some(user_id) = user_id {
                managed::note_gateway_failure(state, user_id, failure).await;
            }
            VoiceError::Gateway(failure)
        }
        None => fallback(error_text),
    }
}

/// Upload filename for audio of the given `Content-Type`
///
/// Speech-to-text providers pick the decoder from the file extension, so
/// browser recordings (`WebM`/Opus from Chromium and Firefox, MP4/AAC from
/// Safari) must not be sent as `.wav`. Unknown or missing types keep `.wav`
#[must_use]
pub fn audio_filename(content_type: Option<&str>) -> &'static str {
    let mime = content_type
        .and_then(|value| value.split(';').next())
        .map(|value| value.trim().to_ascii_lowercase());

    match mime.as_deref() {
        Some("audio/webm" | "video/webm") => "audio.webm",
        Some("audio/ogg") => "audio.ogg",
        Some("audio/mp4" | "audio/m4a" | "audio/x-m4a") => "audio.m4a",
        Some("audio/mpeg" | "audio/mp3") => "audio.mp3",
        _ => "audio.wav",
    }
}

/// Transcription response
#[derive(Debug, Serialize)]
pub struct TranscribeResponse {
    pub text: String,
}

/// Transcribe audio to text
///
/// Accepts WAV, `WebM`, Ogg, MP4/M4A, or MP3 audio, named by `Content-Type`
async fn transcribe(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<TranscribeResponse>, VoiceError> {
    let (synapse, user_id) =
        voice_client(&state, &headers, "STT not configured (no Synapse client)").await?;

    if body.is_empty() {
        return Err(VoiceError::BadRequest("Empty audio data"));
    }

    let filename = audio_filename(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
    );

    let transcription = synapse.transcribe(body, filename, &state.stt_model).await;
    let transcription = match transcription {
        Ok(transcription) => transcription,
        Err(e) => {
            return Err(speech_failure(
                &state,
                user_id.as_deref(),
                e.to_string(),
                VoiceError::TranscriptionFailed,
            )
            .await);
        }
    };

    Ok(Json(TranscribeResponse {
        text: transcription.text,
    }))
}

/// Synthesis request
#[derive(Debug, Deserialize)]
pub struct SynthesizeRequest {
    pub text: String,
}

/// Synthesize text to speech
///
/// Returns audio in MP3 format
async fn synthesize(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(request): Json<SynthesizeRequest>,
) -> Result<Response, VoiceError> {
    let (synapse, user_id) =
        voice_client(&state, &headers, "TTS not configured (no Synapse client)").await?;

    if request.text.is_empty() {
        return Err(VoiceError::BadRequest("Empty text"));
    }

    let speech_request = synapse_client::SpeechRequest {
        model: state.tts_model.clone(),
        input: request.text,
        voice: state.tts_voice.clone(),
        response_format: None,
        speed: Some(state.tts_speed),
    };

    let audio = match synapse.synthesize(&speech_request).await {
        Ok(audio) => audio,
        Err(e) => {
            return Err(speech_failure(
                &state,
                user_id.as_deref(),
                e.to_string(),
                VoiceError::SynthesisFailed,
            )
            .await);
        }
    };

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "audio/mpeg")],
        audio,
    )
        .into_response())
}

/// Voice API errors
#[derive(Debug)]
pub enum VoiceError {
    NotConfigured(&'static str),
    BadRequest(&'static str),
    TranscriptionFailed(String),
    SynthesisFailed(String),
    /// Cloud mode request without a valid Bearer JWT
    Unauthorized,
    /// No managed key could be provisioned
    Managed(ManagedError),
    /// The Synapse gateway refused the request (402 / 503 / 401)
    Gateway(GatewayFailure),
}

impl IntoResponse for VoiceError {
    fn into_response(self) -> Response {
        #[derive(Serialize)]
        struct ErrorResponse {
            error: ErrorBody,
        }

        #[derive(Serialize)]
        struct ErrorBody {
            code: &'static str,
            message: String,
        }

        let (status, code, message) = match self {
            Self::NotConfigured(msg) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "not_configured",
                msg.to_string(),
            ),
            Self::BadRequest(msg) => (StatusCode::BAD_REQUEST, "bad_request", msg.to_string()),
            Self::TranscriptionFailed(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "transcription_failed",
                msg,
            ),
            Self::SynthesisFailed(msg) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "synthesis_failed", msg)
            }
            Self::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "auth_required",
                "Authentication required".to_string(),
            ),
            Self::Managed(failure) => (failure.status(), failure.code(), failure.user_message()),
            Self::Gateway(failure) => (
                match failure {
                    GatewayFailure::OutOfCredits | GatewayFailure::PaymentRequired => {
                        StatusCode::PAYMENT_REQUIRED
                    }
                    GatewayFailure::Unavailable | GatewayFailure::Unauthorized => {
                        StatusCode::SERVICE_UNAVAILABLE
                    }
                },
                failure.code(),
                failure.user_message(),
            ),
        };

        (
            status,
            Json(ErrorResponse {
                error: ErrorBody { code, message },
            }),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_browser_recordings_by_type() {
        assert_eq!(audio_filename(Some("audio/webm;codecs=opus")), "audio.webm");
        assert_eq!(audio_filename(Some("audio/mp4")), "audio.m4a");
        assert_eq!(audio_filename(Some("audio/ogg; codecs=opus")), "audio.ogg");
        assert_eq!(audio_filename(Some("audio/wav")), "audio.wav");
        assert_eq!(audio_filename(None), "audio.wav");
    }

    async fn body_json(response: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn out_of_credits_is_402_with_a_billing_link() {
        let response = VoiceError::Gateway(GatewayFailure::OutOfCredits).into_response();
        assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);
        let body = body_json(response).await;
        assert_eq!(body["error"]["code"], "out_of_credits");
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap()
                .contains("/settings")
        );
    }

    #[tokio::test]
    async fn payment_required_is_402_with_its_own_code() {
        let response = VoiceError::Gateway(GatewayFailure::PaymentRequired).into_response();
        assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);
        let body = body_json(response).await;
        assert_eq!(body["error"]["code"], "payment_required");
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap()
                .contains("/settings")
        );
    }

    #[tokio::test]
    async fn gateway_unavailable_is_503() {
        for failure in [GatewayFailure::Unavailable, GatewayFailure::Unauthorized] {
            let response = VoiceError::Gateway(failure).into_response();
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(body_json(response).await["error"]["code"], "ai_unavailable");
        }
    }

    #[tokio::test]
    async fn managed_failures_map_to_401_403_and_503() {
        let denied = VoiceError::Managed(ManagedError::Denied).into_response();
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        let reauth = VoiceError::Managed(ManagedError::ReauthRequired).into_response();
        assert_eq!(reauth.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(body_json(reauth).await["error"]["code"], "reauth_required");
        let unavailable = VoiceError::Managed(ManagedError::Unavailable).into_response();
        assert_eq!(unavailable.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
