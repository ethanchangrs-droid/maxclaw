use super::traits::{Channel, ChannelMessage, SendMessage};
use async_trait::async_trait;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

const DEFAULT_BASE_URL: &str = "https://ilinkai.weixin.qq.com";
const DEFAULT_POLL_TIMEOUT_MS: u64 = 35_000;
const MAX_MESSAGE_LENGTH: usize = 2000;
const POLL_ERROR_BACKOFF_SECS: u64 = 5;
const TOKEN_FILE_NAME: &str = "weixin_context_tokens.json";
const CURSOR_FILE_NAME: &str = "weixin_cursor.txt";
const TYPING_INTERVAL_SECS: u64 = 4;

/// WeChat personal account channel via iLink Bot HTTP API.
///
/// Uses long-polling (`POST getupdates`) to receive messages and
/// `POST sendmessage` to reply. Replies require a `context_token`
/// obtained from the most recent inbound message of each user.
pub struct WeixinChannel {
    bot_token: String,
    base_url: String,
    allowed_users: Vec<String>,
    poll_timeout_ms: u64,
    context_tokens: Arc<RwLock<HashMap<String, String>>>,
    token_store_path: PathBuf,
    cursor_store_path: PathBuf,
    typing_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

#[derive(Debug, Deserialize)]
struct GetUpdatesResp {
    #[serde(default)]
    errcode: i64,
    #[serde(default)]
    errmsg: Option<String>,
    #[serde(default)]
    msgs: Vec<WeixinMessage>,
    #[serde(default)]
    get_updates_buf: String,
}

#[derive(Debug, Deserialize)]
struct WeixinMessage {
    #[serde(default)]
    msg_id: String,
    #[serde(default)]
    from_user_id: String,
    #[serde(default)]
    message_type: i32,
    #[serde(default)]
    context_token: String,
    #[serde(default)]
    create_time: u64,
    #[serde(default)]
    item_list: Vec<WeixinMessageItem>,
}

#[derive(Debug, Deserialize)]
struct WeixinMessageItem {
    #[serde(rename = "type", default)]
    item_type: i32,
    #[serde(default)]
    text_item: Option<TextItem>,
}

#[derive(Debug, Deserialize)]
struct TextItem {
    #[serde(default)]
    text: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct SendMessageResp {
    #[serde(default)]
    errcode: i64,
    #[serde(default)]
    errmsg: Option<String>,
}

impl WeixinChannel {
    pub fn new(
        bot_token: String,
        base_url: Option<String>,
        allowed_users: Vec<String>,
        poll_timeout_ms: Option<u64>,
        workspace_dir: Option<&Path>,
    ) -> Self {
        let store_dir = workspace_dir.unwrap_or_else(|| Path::new("."));
        let weixin_dir = store_dir.join("weixin");
        let token_store_path = weixin_dir.join(TOKEN_FILE_NAME);
        let cursor_store_path = weixin_dir.join(CURSOR_FILE_NAME);

        Self {
            bot_token,
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            allowed_users,
            poll_timeout_ms: poll_timeout_ms.unwrap_or(DEFAULT_POLL_TIMEOUT_MS),
            context_tokens: Arc::new(RwLock::new(HashMap::new())),
            token_store_path,
            cursor_store_path,
            typing_handle: Mutex::new(None),
        }
    }

    fn http_client(&self) -> reqwest::Client {
        crate::config::build_runtime_proxy_client_with_timeouts(
            "channel.weixin",
            self.poll_timeout_ms / 1000 + 10,
            10,
        )
    }

    fn build_headers(&self) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("Content-Type", "application/json".parse().unwrap());
        headers.insert("AuthorizationType", "ilink_bot_token".parse().unwrap());
        headers.insert(
            "Authorization",
            format!("Bearer {}", self.bot_token).parse().unwrap(),
        );
        let uin = rand::random::<u32>().to_string();
        let uin_b64 =
            base64::engine::general_purpose::STANDARD.encode(uin.as_bytes());
        headers.insert("X-WECHAT-UIN", uin_b64.parse().unwrap());
        headers
    }

    fn is_user_allowed(&self, user_id: &str) -> bool {
        self.allowed_users.is_empty()
            || self
                .allowed_users
                .iter()
                .any(|u| u == "*" || u == user_id)
    }

    async fn poll_messages(
        &self,
        cursor: &str,
    ) -> anyhow::Result<(Vec<WeixinMessage>, String)> {
        let url = format!("{}/ilink/bot/getupdates", self.base_url);
        let resp = self
            .http_client()
            .post(&url)
            .headers(self.build_headers())
            .json(&serde_json::json!({
                "get_updates_buf": cursor,
                "base_info": { "channel_version": "" }
            }))
            .timeout(std::time::Duration::from_millis(
                self.poll_timeout_ms + 5_000,
            ))
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("iLink getupdates HTTP {status}: {body}");
        }

        let data: GetUpdatesResp = resp.json().await?;
        if data.errcode != 0 {
            let msg = data.errmsg.as_deref().unwrap_or("unknown");
            anyhow::bail!("iLink getupdates errcode={}: {msg}", data.errcode);
        }

        Ok((data.msgs, data.get_updates_buf))
    }

    async fn send_text(
        &self,
        to: &str,
        text: &str,
        context_token: &str,
    ) -> anyhow::Result<()> {
        let url = format!("{}/ilink/bot/sendmessage", self.base_url);
        let client_id = format!("maxclaw_{}", uuid::Uuid::new_v4());
        let resp = self
            .http_client()
            .post(&url)
            .headers(self.build_headers())
            .json(&serde_json::json!({
                "to_user_id": to,
                "context_token": context_token,
                "client_msg_id": client_id,
                "item_list": [{
                    "type": 1,
                    "text_item": { "text": text }
                }]
            }))
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("iLink sendmessage HTTP {status}: {body}");
        }

        let result: SendMessageResp = resp.json().await?;
        if result.errcode != 0 {
            let msg = result.errmsg.as_deref().unwrap_or("unknown");
            anyhow::bail!("iLink sendmessage errcode={}: {msg}", result.errcode);
        }

        Ok(())
    }

    async fn save_context_token(&self, user_id: &str, token: &str) {
        {
            let mut tokens = self.context_tokens.write().await;
            tokens.insert(user_id.to_string(), token.to_string());
        }
        self.persist_tokens().await;
    }

    async fn get_context_token(&self, user_id: &str) -> Option<String> {
        let tokens = self.context_tokens.read().await;
        tokens.get(user_id).cloned()
    }

    async fn persist_tokens(&self) {
        let tokens = self.context_tokens.read().await;
        let map: &HashMap<String, String> = &tokens;
        match serde_json::to_string_pretty(map) {
            Ok(json) => {
                if let Some(parent) = self.token_store_path.parent() {
                    let _ = tokio::fs::create_dir_all(parent).await;
                }
                if let Err(e) = tokio::fs::write(&self.token_store_path, json).await {
                    tracing::warn!("Failed to persist weixin context tokens: {e}");
                }
            }
            Err(e) => tracing::warn!("Failed to serialize weixin context tokens: {e}"),
        }
    }

    async fn load_tokens(&self) {
        match tokio::fs::read_to_string(&self.token_store_path).await {
            Ok(json) => match serde_json::from_str::<HashMap<String, String>>(&json) {
                Ok(map) => {
                    let mut tokens = self.context_tokens.write().await;
                    *tokens = map;
                    tracing::info!(
                        "Loaded {} weixin context token(s) from disk",
                        tokens.len()
                    );
                }
                Err(e) => tracing::warn!("Failed to parse weixin context tokens file: {e}"),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!("No existing weixin context tokens file");
            }
            Err(e) => tracing::warn!("Failed to read weixin context tokens file: {e}"),
        }
    }

    async fn get_typing_ticket(
        &self,
        to: &str,
        context_token: &str,
    ) -> anyhow::Result<Option<String>> {
        let url = format!("{}/ilink/bot/getconfig", self.base_url);

        #[derive(serde::Deserialize)]
        struct ConfigResp {
            #[serde(default)]
            typing_ticket: Option<String>,
        }

        let resp = self
            .http_client()
            .post(&url)
            .headers(self.build_headers())
            .json(&serde_json::json!({
                "to_user_id": to,
                "context_token": context_token,
                "base_info": { "channel_version": "2.0.0" }
            }))
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await?;

        if resp.status().is_success() {
            if let Ok(data) = resp.json::<ConfigResp>().await {
                return Ok(data.typing_ticket);
            }
        }
        Ok(None)
    }

    async fn save_cursor(&self, cursor: &str) {
        if let Some(parent) = self.cursor_store_path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        if let Err(e) = tokio::fs::write(&self.cursor_store_path, cursor).await {
            tracing::warn!("Failed to persist weixin cursor: {e}");
        }
    }

    async fn load_cursor(&self) -> String {
        match tokio::fs::read_to_string(&self.cursor_store_path).await {
            Ok(cursor) => {
                let cursor = cursor.trim().to_string();
                if !cursor.is_empty() {
                    tracing::info!("Loaded weixin cursor from disk");
                }
                cursor
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!("No existing weixin cursor file");
                String::new()
            }
            Err(e) => {
                tracing::warn!("Failed to read weixin cursor file: {e}");
                String::new()
            }
        }
    }

    fn extract_text(msg: &WeixinMessage) -> Option<String> {
        for item in &msg.item_list {
            if item.item_type == 1 {
                if let Some(ref text_item) = item.text_item {
                    if !text_item.text.is_empty() {
                        return Some(text_item.text.clone());
                    }
                }
            }
        }
        None
    }

    fn split_long_message(text: &str) -> Vec<&str> {
        if text.len() <= MAX_MESSAGE_LENGTH {
            return vec![text];
        }
        let mut chunks = Vec::new();
        let mut start = 0;
        while start < text.len() {
            let end = (start + MAX_MESSAGE_LENGTH).min(text.len());
            let boundary = if end < text.len() {
                text[start..end]
                    .rfind('\n')
                    .map(|p| start + p + 1)
                    .unwrap_or(end)
            } else {
                end
            };
            chunks.push(&text[start..boundary]);
            start = boundary;
        }
        chunks
    }
}

use base64::Engine as _;

#[async_trait]
impl Channel for WeixinChannel {
    fn name(&self) -> &str {
        "weixin"
    }

    async fn send(&self, message: &SendMessage) -> anyhow::Result<()> {
        let user_id = &message.recipient;
        let context_token = self.get_context_token(user_id).await.ok_or_else(|| {
            anyhow::anyhow!(
                "No context_token for weixin user {user_id}. \
                 The user must send at least one message before the bot can reply."
            )
        })?;

        let chunks = Self::split_long_message(&message.content);
        for chunk in chunks {
            self.send_text(user_id, chunk, &context_token).await?;
        }

        Ok(())
    }

    async fn listen(
        &self,
        tx: tokio::sync::mpsc::Sender<ChannelMessage>,
    ) -> anyhow::Result<()> {
        self.load_tokens().await;

        let mut cursor = self.load_cursor().await;

        tracing::info!("WeiXin iLink channel listening for messages...");

        loop {
            match self.poll_messages(&cursor).await {
                Ok((msgs, new_cursor)) => {
                    if new_cursor != cursor {
                        cursor = new_cursor;
                        self.save_cursor(&cursor).await;
                    }

                    for msg in msgs {
                        if msg.from_user_id.is_empty() {
                            continue;
                        }

                        if !self.is_user_allowed(&msg.from_user_id) {
                            tracing::debug!(
                                "WeiXin: ignoring message from non-allowed user {}",
                                msg.from_user_id
                            );
                            continue;
                        }

                        if !msg.context_token.is_empty() {
                            self.save_context_token(&msg.from_user_id, &msg.context_token)
                                .await;
                        }

                        let text = match Self::extract_text(&msg) {
                            Some(t) => t,
                            None => {
                                tracing::debug!(
                                    "WeiXin: skipping non-text message (type={}) from {}",
                                    msg.message_type,
                                    msg.from_user_id
                                );
                                continue;
                            }
                        };

                        let channel_msg = ChannelMessage {
                            id: msg.msg_id,
                            sender: msg.from_user_id.clone(),
                            reply_target: msg.from_user_id,
                            content: text,
                            channel: "weixin".to_string(),
                            timestamp: msg.create_time,
                            thread_ts: None,
                        };

                        if tx.send(channel_msg).await.is_err() {
                            tracing::info!("WeiXin: mpsc channel closed, stopping listener");
                            return Ok(());
                        }
                    }
                }
                Err(e) => {
                    let err_str = e.to_string();
                    if err_str.contains("errcode=-14") {
                        tracing::error!(
                            "WeiXin: session expired (errcode=-14). \
                             Please re-scan QR code and update bot_token."
                        );
                        return Err(e);
                    }
                    tracing::warn!(
                        "WeiXin poll error: {e}; retrying in {POLL_ERROR_BACKOFF_SECS}s"
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(
                        POLL_ERROR_BACKOFF_SECS,
                    ))
                    .await;
                }
            }
        }
    }

    async fn health_check(&self) -> bool {
        let url = format!("{}/ilink/bot/getupdates", self.base_url);
        let resp = self
            .http_client()
            .post(&url)
            .headers(self.build_headers())
            .json(&serde_json::json!({
                "get_updates_buf": "",
                "base_info": { "channel_version": "" }
            }))
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await;

        match resp {
            Ok(r) => r.status().is_success(),
            Err(_) => false,
        }
    }

    async fn start_typing(&self, recipient: &str) -> anyhow::Result<()> {
        self.stop_typing(recipient).await?;

        let context_token = match self.get_context_token(recipient).await {
            Some(t) => t,
            None => {
                tracing::debug!(
                    "WeiXin: no context_token for {recipient}, skipping typing indicator"
                );
                return Ok(());
            }
        };

        let typing_ticket = self
            .get_typing_ticket(recipient, &context_token)
            .await
            .unwrap_or(None);

        let typing_ticket = match typing_ticket {
            Some(t) if !t.is_empty() => t,
            _ => {
                tracing::debug!(
                    "WeiXin: no typing_ticket for {recipient}, skipping typing indicator"
                );
                return Ok(());
            }
        };

        let client = self.http_client();
        let headers = self.build_headers();
        let url = format!("{}/ilink/bot/sendtyping", self.base_url);
        let user_id = recipient.to_string();
        let ctx_token = context_token;

        let handle = tokio::spawn(async move {
            // status=1 starts typing, status=2 stops
            let _ = client
                .post(&url)
                .headers(headers.clone())
                .json(&serde_json::json!({
                    "to_user_id": &user_id,
                    "context_token": &ctx_token,
                    "typing_ticket": &typing_ticket,
                    "status": 1,
                    "base_info": { "channel_version": "2.0.0" }
                }))
                .timeout(std::time::Duration::from_secs(10))
                .send()
                .await;

            loop {
                tokio::time::sleep(std::time::Duration::from_secs(TYPING_INTERVAL_SECS)).await;
                let _ = client
                    .post(&url)
                    .headers(headers.clone())
                    .json(&serde_json::json!({
                        "to_user_id": &user_id,
                        "context_token": &ctx_token,
                        "typing_ticket": &typing_ticket,
                        "status": 1,
                        "base_info": { "channel_version": "2.0.0" }
                    }))
                    .timeout(std::time::Duration::from_secs(10))
                    .send()
                    .await;
            }
        });

        let mut guard = self.typing_handle.lock();
        *guard = Some(handle);

        Ok(())
    }

    async fn stop_typing(&self, _recipient: &str) -> anyhow::Result<()> {
        let mut guard = self.typing_handle.lock();
        if let Some(handle) = guard.take() {
            handle.abort();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_name() {
        let ch = WeixinChannel::new(
            "test-token".into(),
            None,
            vec![],
            None,
            None,
        );
        assert_eq!(ch.name(), "weixin");
    }

    #[test]
    fn test_user_allowed_empty_allows_all() {
        let ch = WeixinChannel::new("t".into(), None, vec![], None, None);
        assert!(ch.is_user_allowed("anyone"));
    }

    #[test]
    fn test_user_allowed_wildcard() {
        let ch = WeixinChannel::new("t".into(), None, vec!["*".into()], None, None);
        assert!(ch.is_user_allowed("anyone"));
    }

    #[test]
    fn test_user_allowed_specific() {
        let ch = WeixinChannel::new(
            "t".into(),
            None,
            vec!["wxid_abc".into()],
            None,
            None,
        );
        assert!(ch.is_user_allowed("wxid_abc"));
        assert!(!ch.is_user_allowed("wxid_other"));
    }

    #[test]
    fn test_split_short_message() {
        let text = "Hello, world!";
        let chunks = WeixinChannel::split_long_message(text);
        assert_eq!(chunks, vec!["Hello, world!"]);
    }

    #[test]
    fn test_split_long_message() {
        let text = "a".repeat(MAX_MESSAGE_LENGTH + 100);
        let chunks = WeixinChannel::split_long_message(&text);
        assert!(chunks.len() > 1);
        let total: usize = chunks.iter().map(|c| c.len()).sum();
        assert_eq!(total, text.len());
    }

    #[test]
    fn test_extract_text() {
        let msg = WeixinMessage {
            msg_id: "1".into(),
            from_user_id: "user1".into(),
            message_type: 1,
            context_token: "ct".into(),
            create_time: 1000,
            item_list: vec![WeixinMessageItem {
                item_type: 1,
                text_item: Some(TextItem {
                    text: "hello".into(),
                }),
            }],
        };
        assert_eq!(WeixinChannel::extract_text(&msg), Some("hello".into()));
    }

    #[test]
    fn test_extract_text_empty_items() {
        let msg = WeixinMessage {
            msg_id: "1".into(),
            from_user_id: "user1".into(),
            message_type: 3,
            context_token: "ct".into(),
            create_time: 1000,
            item_list: vec![],
        };
        assert_eq!(WeixinChannel::extract_text(&msg), None);
    }

    #[test]
    fn test_config_serde() {
        let toml_str = r#"
bot_token = "test_token_123"
allowed_users = ["wxid_abc", "wxid_def"]
poll_timeout_ms = 30000
"#;
        let config: crate::config::schema::WeixinConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.bot_token, "test_token_123");
        assert_eq!(config.allowed_users, vec!["wxid_abc", "wxid_def"]);
        assert_eq!(config.poll_timeout_ms, Some(30000));
    }

    #[test]
    fn test_config_serde_defaults() {
        let toml_str = r#"
bot_token = "tk"
"#;
        let config: crate::config::schema::WeixinConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.bot_token, "tk");
        assert!(config.allowed_users.is_empty());
        assert!(config.base_url.is_none());
        assert!(config.poll_timeout_ms.is_none());
    }

    #[test]
    fn typing_handle_starts_as_none() {
        let ch = WeixinChannel::new("t".into(), None, vec![], None, None);
        let guard = ch.typing_handle.lock();
        assert!(guard.is_none());
    }

    #[tokio::test]
    async fn stop_typing_clears_handle() {
        let ch = WeixinChannel::new("t".into(), None, vec![], None, None);
        {
            let mut guard = ch.typing_handle.lock();
            *guard = Some(tokio::spawn(async {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            }));
        }
        ch.stop_typing("user1").await.unwrap();
        let guard = ch.typing_handle.lock();
        assert!(guard.is_none());
    }

    #[tokio::test]
    async fn cursor_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let ch = WeixinChannel::new(
            "t".into(),
            None,
            vec![],
            None,
            Some(dir.path()),
        );

        assert!(ch.load_cursor().await.is_empty());

        ch.save_cursor("cursor_abc_123").await;
        let loaded = ch.load_cursor().await;
        assert_eq!(loaded, "cursor_abc_123");

        ch.save_cursor("cursor_def_456").await;
        let loaded2 = ch.load_cursor().await;
        assert_eq!(loaded2, "cursor_def_456");
    }

    #[tokio::test]
    async fn start_typing_skips_without_context_token() {
        let ch = WeixinChannel::new("t".into(), None, vec![], None, None);
        let result = ch.start_typing("unknown_user").await;
        assert!(result.is_ok());
        let guard = ch.typing_handle.lock();
        assert!(guard.is_none());
    }
}
