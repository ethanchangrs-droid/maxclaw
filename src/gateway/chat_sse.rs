//! SSE streaming chat endpoint.
//!
//! `POST /api/chat` accepts a JSON body with `message` (and optional `session_id`),
//! creates an ephemeral `Agent`, streams `AgentEvent`s back as SSE `data:` lines,
//! and closes the connection after the final `done` or `error` event.

use super::AppState;
use crate::agent::AgentEvent;
use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
    Json,
};
use serde::Deserialize;
use std::convert::Infallible;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

#[derive(Deserialize)]
pub struct ChatRequest {
    pub message: String,
    pub session_id: Option<String>,
}

/// POST /api/chat — SSE streaming chat with the agent.
pub async fn handle_chat_sse(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<ChatRequest>, axum::extract::rejection::JsonRejection>,
) -> impl IntoResponse {
    // Auth
    if state.pairing.require_pairing() {
        let token = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|auth| auth.strip_prefix("Bearer "))
            .unwrap_or("");

        if !state.pairing.is_authenticated(token) {
            return (
                StatusCode::UNAUTHORIZED,
                "Unauthorized — provide Authorization: Bearer <token>",
            )
                .into_response();
        }
    }

    let Json(body) = match body {
        Ok(b) => b,
        Err(e) => {
            let err = serde_json::json!({ "type": "error", "message": format!("Bad request: {e}") });
            return (StatusCode::BAD_REQUEST, Json(err)).into_response();
        }
    };

    if body.message.is_empty() {
        let err = serde_json::json!({ "type": "error", "message": "message must not be empty" });
        return (StatusCode::BAD_REQUEST, Json(err)).into_response();
    }

    let config = state.config.lock().clone();
    let mut agent = match crate::agent::Agent::from_config(&config) {
        Ok(a) => a,
        Err(e) => {
            let err = serde_json::json!({
                "type": "error",
                "message": format!("Failed to initialise agent: {e}"),
            });
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(err)).into_response();
        }
    };

    let session_id = body
        .session_id
        .as_deref()
        .map(|s| format!("sse_{s}"))
        .or_else(|| Some(format!("sse_{}", uuid::Uuid::new_v4())));
    agent.set_memory_session_id(session_id);

    let (event_tx, event_rx) = tokio::sync::mpsc::channel::<AgentEvent>(64);
    let error_tx = event_tx.clone();
    agent.set_event_sender(Some(event_tx));

    let message = body.message;
    tokio::spawn(async move {
        if let Err(e) = agent.turn(&message).await {
            let sanitized = crate::providers::sanitize_api_error(&e.to_string());
            let _ = error_tx
                .send(AgentEvent::Error {
                    message: sanitized,
                })
                .await;
        }
    });

    let stream = ReceiverStream::new(event_rx).map(|event| {
        let json = match event {
            AgentEvent::Reasoning(content) => serde_json::json!({
                "type": "reasoning",
                "content": content,
            }),
            AgentEvent::Chunk(content) => serde_json::json!({
                "type": "chunk",
                "content": content,
            }),
            AgentEvent::ToolCallStart { name, arguments } => serde_json::json!({
                "type": "tool_call",
                "name": name,
                "args": arguments,
            }),
            AgentEvent::ToolCallComplete {
                name,
                output,
                success,
                duration_ms,
            } => serde_json::json!({
                "type": "tool_result",
                "name": name,
                "output": output,
                "success": success,
                "duration_ms": duration_ms,
            }),
            AgentEvent::Done { text, .. } => serde_json::json!({
                "type": "done",
                "full_response": text,
            }),
            AgentEvent::Error { message } => serde_json::json!({
                "type": "error",
                "message": message,
            }),
        };
        Ok::<_, Infallible>(Event::default().data(json.to_string()))
    });

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_request_deserializes_with_message_only() {
        let json = r#"{"message": "hello"}"#;
        let req: ChatRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.message, "hello");
        assert!(req.session_id.is_none());
    }

    #[test]
    fn chat_request_deserializes_with_session_id() {
        let json = r#"{"message": "hello", "session_id": "app_123"}"#;
        let req: ChatRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.message, "hello");
        assert_eq!(req.session_id.as_deref(), Some("app_123"));
    }

    #[test]
    fn chat_request_rejects_missing_message() {
        let json = r#"{"session_id": "app_123"}"#;
        assert!(serde_json::from_str::<ChatRequest>(json).is_err());
    }
}
