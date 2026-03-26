//! WebSocket agent chat handler.
//!
//! Protocol:
//! ```text
//! Client -> Server: {"type":"message","content":"Hello"}
//! Server -> Client: {"type":"chunk","content":"Hi! "}
//! Server -> Client: {"type":"tool_call","name":"shell","args":{...}}
//! Server -> Client: {"type":"tool_result","name":"shell","output":"..."}
//! Server -> Client: {"type":"done","full_response":"..."}
//! ```

use super::AppState;
use crate::agent::AgentEvent;
use axum::{
    extract::{
        ws::{Message, WebSocket},
        Query, State, WebSocketUpgrade,
    },
    http::{header, HeaderMap},
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;

/// The sub-protocol we support for the chat WebSocket.
const WS_PROTOCOL: &str = "zeroclaw.v1";

/// Prefix used in `Sec-WebSocket-Protocol` to carry a bearer token.
const BEARER_SUBPROTO_PREFIX: &str = "bearer.";

#[derive(Deserialize)]
pub struct WsQuery {
    pub token: Option<String>,
    pub session_id: Option<String>,
}

/// Extract a bearer token from WebSocket-compatible sources.
///
/// Precedence (first non-empty wins):
/// 1. `Authorization: Bearer <token>` header
/// 2. `Sec-WebSocket-Protocol: bearer.<token>` subprotocol
/// 3. `?token=<token>` query parameter
///
/// Browsers cannot set custom headers on `new WebSocket(url)`, so the query
/// parameter and subprotocol paths are required for browser-based clients.
fn extract_ws_token<'a>(headers: &'a HeaderMap, query_token: Option<&'a str>) -> Option<&'a str> {
    // 1. Authorization header
    if let Some(t) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|auth| auth.strip_prefix("Bearer "))
    {
        if !t.is_empty() {
            return Some(t);
        }
    }

    // 2. Sec-WebSocket-Protocol: bearer.<token>
    if let Some(t) = headers
        .get("sec-websocket-protocol")
        .and_then(|v| v.to_str().ok())
        .and_then(|protos| {
            protos
                .split(',')
                .map(|p| p.trim())
                .find_map(|p| p.strip_prefix(BEARER_SUBPROTO_PREFIX))
        })
    {
        if !t.is_empty() {
            return Some(t);
        }
    }

    // 3. ?token= query parameter
    if let Some(t) = query_token {
        if !t.is_empty() {
            return Some(t);
        }
    }

    None
}

/// GET /ws/chat — WebSocket upgrade for agent chat
pub async fn handle_ws_chat(
    State(state): State<AppState>,
    Query(params): Query<WsQuery>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    // Auth: check header, subprotocol, then query param (precedence order)
    if state.pairing.require_pairing() {
        let token = extract_ws_token(&headers, params.token.as_deref()).unwrap_or("");
        if !state.pairing.is_authenticated(token) {
            return (
                axum::http::StatusCode::UNAUTHORIZED,
                "Unauthorized — provide Authorization header, Sec-WebSocket-Protocol bearer, or ?token= query param",
            )
                .into_response();
        }
    }

    // Echo Sec-WebSocket-Protocol if the client requests our sub-protocol.
    let ws = if headers
        .get("sec-websocket-protocol")
        .and_then(|v| v.to_str().ok())
        .map_or(false, |protos| {
            protos.split(',').any(|p| p.trim() == WS_PROTOCOL)
        }) {
        ws.protocols([WS_PROTOCOL])
    } else {
        ws
    };

    let session_id = params.session_id.clone();
    ws.on_upgrade(move |socket| handle_socket(socket, state, session_id))
        .into_response()
}

async fn handle_socket(socket: WebSocket, state: AppState, session_id: Option<String>) {
    let (mut sender, mut receiver) = socket.split();

    // Build a persistent Agent for this connection so history is maintained across turns.
    let config = state.config.lock().clone();
    let mut agent = match crate::agent::Agent::from_config(&config) {
        Ok(a) => a,
        Err(e) => {
            let err = serde_json::json!({"type": "error", "message": format!("Failed to initialise agent: {e}")});
            let _ = sender.send(Message::Text(err.to_string().into())).await;
            return;
        }
    };
    agent.set_memory_session_id(session_id.clone());

    let (event_tx, event_rx) = tokio::sync::mpsc::channel::<AgentEvent>(64);
    let error_event_tx = event_tx.clone();
    agent.set_event_sender(Some(event_tx));

    // WebSocket write queue serialises all outbound frames.  All agent
    // events — including errors — flow through the forwarder to preserve
    // strict FIFO ordering with preceding Reasoning/ToolCall events.
    let (ws_write_tx, mut ws_write_rx) = tokio::sync::mpsc::channel::<Message>(64);
    let write_handle = tokio::spawn(async move {
        while let Some(msg) = ws_write_rx.recv().await {
            if sender.send(msg).await.is_err() {
                break;
            }
        }
    });

    let forward_handle = forward_agent_events(event_rx, ws_write_tx.clone());

    while let Some(msg) = receiver.next().await {
        let msg = match msg {
            Ok(Message::Text(text)) => text,
            Ok(Message::Close(_)) | Err(_) => break,
            _ => continue,
        };

        // Parse incoming message
        let parsed: serde_json::Value = match serde_json::from_str(&msg) {
            Ok(v) => v,
            Err(_) => {
                let _ = error_event_tx
                    .send(AgentEvent::Error {
                        message: "Invalid JSON".into(),
                    })
                    .await;
                continue;
            }
        };

        let msg_type = parsed["type"].as_str().unwrap_or("");
        if msg_type != "message" {
            continue;
        }

        let content = parsed["content"].as_str().unwrap_or("").to_string();
        if content.is_empty() {
            continue;
        }

        // Process message with the LLM provider
        let provider_label = state
            .config
            .lock()
            .default_provider
            .clone()
            .unwrap_or_else(|| "unknown".to_string());

        // Broadcast agent_start event
        let _ = state.event_tx.send(serde_json::json!({
            "type": "agent_start",
            "provider": provider_label,
            "model": state.model,
        }));

        // Multi-turn chat via persistent Agent (history is maintained across turns)
        match agent.turn(&content).await {
            Ok(_response) => {
                // The done message is sent by forward_agent_events when it
                // processes AgentEvent::Done, preserving FIFO ordering with
                // preceding Reasoning/ToolCall events in the event channel.

                // Broadcast agent_end event (SSE, separate from WS)
                let _ = state.event_tx.send(serde_json::json!({
                    "type": "agent_end",
                    "provider": provider_label,
                    "model": state.model,
                }));
            }
            Err(e) => {
                let sanitized = crate::providers::sanitize_api_error(&e.to_string());

                // Send error through the event channel so it is FIFO-ordered
                // after any pending ToolCallComplete / Reasoning events.
                let _ = error_event_tx
                    .send(AgentEvent::Error {
                        message: sanitized.clone(),
                    })
                    .await;

                // Broadcast error event (SSE, separate from WS)
                let _ = state.event_tx.send(serde_json::json!({
                    "type": "error",
                    "component": "ws_chat",
                    "message": sanitized,
                }));
            }
        }
    }

    drop(ws_write_tx);
    drop(error_event_tx);
    forward_handle.abort();
    let _ = write_handle.await;
}

fn forward_agent_events(
    mut event_rx: tokio::sync::mpsc::Receiver<AgentEvent>,
    ws_tx: tokio::sync::mpsc::Sender<Message>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            let msg = match event {
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
            if ws_tx
                .send(Message::Text(msg.to_string().into()))
                .await
                .is_err()
            {
                break;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    #[test]
    fn extract_ws_token_from_authorization_header() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer zc_test123".parse().unwrap());
        assert_eq!(extract_ws_token(&headers, None), Some("zc_test123"));
    }

    #[test]
    fn extract_ws_token_from_subprotocol() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "sec-websocket-protocol",
            "zeroclaw.v1, bearer.zc_sub456".parse().unwrap(),
        );
        assert_eq!(extract_ws_token(&headers, None), Some("zc_sub456"));
    }

    #[test]
    fn extract_ws_token_from_query_param() {
        let headers = HeaderMap::new();
        assert_eq!(
            extract_ws_token(&headers, Some("zc_query789")),
            Some("zc_query789")
        );
    }

    #[test]
    fn extract_ws_token_precedence_header_over_subprotocol() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer zc_header".parse().unwrap());
        headers.insert("sec-websocket-protocol", "bearer.zc_sub".parse().unwrap());
        assert_eq!(
            extract_ws_token(&headers, Some("zc_query")),
            Some("zc_header")
        );
    }

    #[test]
    fn extract_ws_token_precedence_subprotocol_over_query() {
        let mut headers = HeaderMap::new();
        headers.insert("sec-websocket-protocol", "bearer.zc_sub".parse().unwrap());
        assert_eq!(extract_ws_token(&headers, Some("zc_query")), Some("zc_sub"));
    }

    #[test]
    fn extract_ws_token_returns_none_when_empty() {
        let headers = HeaderMap::new();
        assert_eq!(extract_ws_token(&headers, None), None);
    }

    #[test]
    fn extract_ws_token_skips_empty_header_value() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer ".parse().unwrap());
        assert_eq!(
            extract_ws_token(&headers, Some("zc_fallback")),
            Some("zc_fallback")
        );
    }

    #[test]
    fn extract_ws_token_skips_empty_query_param() {
        let headers = HeaderMap::new();
        assert_eq!(extract_ws_token(&headers, Some("")), None);
    }

    #[test]
    fn extract_ws_token_subprotocol_with_multiple_entries() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "sec-websocket-protocol",
            "zeroclaw.v1, bearer.zc_tok, other".parse().unwrap(),
        );
        assert_eq!(extract_ws_token(&headers, None), Some("zc_tok"));
    }
}
