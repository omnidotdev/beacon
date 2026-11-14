//! Attachment AI (vision, transcription) runs only on the gateway client the
//! caller passes for the turn, never on a client the processor holds

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::http::HeaderMap;
use axum::routing::post;
use beacon_gateway::attachments::AttachmentProcessor;
use beacon_gateway::channels::{Attachment, AttachmentKind};
use synapse_client::SynapseClient;

fn image() -> Attachment {
    Attachment {
        kind: AttachmentKind::Image,
        url: None,
        data: Some(vec![0x89, b'P', b'N', b'G']),
        mime_type: "image/png".to_string(),
        filename: Some("cat.png".to_string()),
    }
}

fn audio() -> Attachment {
    Attachment {
        kind: AttachmentKind::Audio,
        url: None,
        data: Some(b"RIFF....WAVE".to_vec()),
        mime_type: "audio/wav".to_string(),
        filename: Some("note.wav".to_string()),
    }
}

/// Start a fake gateway that records the bearer token of every request
async fn fake_gateway() -> (String, Arc<Mutex<Vec<String>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = |seen: Arc<Mutex<Vec<String>>>, headers: &HeaderMap| {
        let auth = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        seen.lock().unwrap().push(auth);
    };

    let chat_seen = Arc::clone(&seen);
    let stt_seen = Arc::clone(&seen);
    let app = Router::new()
        .route(
            "/v1/chat/completions",
            post(move |headers: HeaderMap| {
                record(Arc::clone(&chat_seen), &headers);
                async {
                    axum::Json(serde_json::json!({
                        "id": "chatcmpl-1",
                        "object": "chat.completion",
                        "created": 0,
                        "model": "anthropic/claude-sonnet-5-5",
                        "choices": [{
                            "index": 0,
                            "message": { "role": "assistant", "content": "a cat" },
                            "finish_reason": "stop"
                        }]
                    }))
                }
            }),
        )
        .route(
            "/v1/audio/transcriptions",
            post(move |headers: HeaderMap| {
                record(Arc::clone(&stt_seen), &headers);
                async { axum::Json(serde_json::json!({ "text": "hello there" })) }
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), seen)
}

#[tokio::test]
async fn without_a_gateway_media_is_left_as_placeholders() {
    let processor =
        AttachmentProcessor::new("anthropic/claude-sonnet-5-5".into(), "whisper-1".into());

    let text = processor
        .process_attachments(&[image(), audio()], None)
        .await
        .unwrap();

    assert!(text.contains("[Image: cat.png]"), "{text}");
    assert!(text.contains("[Audio: note.wav]"), "{text}");
}

#[tokio::test]
async fn media_runs_on_the_turn_gateway_key() {
    let (url, seen) = fake_gateway().await;
    let turn = Arc::new(
        SynapseClient::new(&url)
            .unwrap()
            .with_api_key("synapse_user_product_key".to_string()),
    );
    let processor =
        AttachmentProcessor::new("anthropic/claude-sonnet-5-5".into(), "whisper-1".into());

    let text = processor
        .process_attachments(&[image(), audio()], Some(&turn))
        .await
        .unwrap();

    assert!(text.contains("a cat"), "{text}");
    assert!(text.contains("hello there"), "{text}");
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert!(
        seen.iter()
            .all(|auth| auth == "Bearer synapse_user_product_key"),
        "{seen:?}"
    );
}
