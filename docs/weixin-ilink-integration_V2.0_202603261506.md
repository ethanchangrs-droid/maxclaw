# 微信 iLink 接入 MaxClaw 架构方案

| 项目 | 内容 |
|------|------|
| 版本 | V2.0 |
| 日期 | 2026-03-26 15:06 (UTC+8) |
| 方案 | 方案 A：Rust 原生 Channel 实现 |
| 前置文档 | weixin-integration-analysis V1.0 (2026-03-23)、Channel 与 Gateway 架构分析 V2.0 (2026-03-26) |

版本历史：
- V1.0 (2026-03-23 16:36): 初始可行性分析与方案对比
- V2.0 (2026-03-26 15:06): 基于架构文档审查修正——精确化 Telegram 同构表述、完善 context_token 持久化方案、补充 ChannelMessage 字段映射、深化 Cron 投递分析、补充 Draft/Typing 可行性评估、调整开发量估算

---

## 目录

1. [需求概述](#1-需求概述)
2. [现状分析](#2-现状分析)
3. [可行性判定](#3-可行性判定)
4. [方案选型](#4-方案选型)
5. [架构详细设计](#5-架构详细设计)
6. [context_token 管理方案](#6-context_token-管理方案)
7. [登录与会话过期处理](#7-登录与会话过期处理)
8. [定时消息（Cron）投递微信](#8-定时消息cron投递微信)
9. [实施路线图](#9-实施路线图)
10. [风险与缓解](#10-风险与缓解)
11. [结论](#11-结论)

---

## 1. 需求概述

通过微信个人号远程与部署在 Docker 中的 MaxClaw（ZeroClaw 改名）进行对话。微信侧使用腾讯官方 iLink Bot HTTP API（`openclaw-weixin` 插件所封装的协议），MaxClaw 侧已有 HTTP Gateway + WebSocket + Channel 体系。

---

## 2. 现状分析

### 2.1 微信插件侧（openclaw-weixin）

| 项目 | 说明 |
|------|------|
| 协议 | 标准 HTTP JSON（iLink Bot API），基址 `https://ilinkai.weixin.qq.com` |
| 鉴权 | 扫码登录获取 `bot_token`，后续请求 `Bearer {token}` |
| 收消息 | 长轮询 `POST getupdates`（约 35s 超时），返回消息数组 + 游标 |
| 发消息 | `POST sendmessage`，必须携带 `context_token`（来自入站消息） |
| 媒体 | AES-128-ECB 加密上传 CDN，下载后解密 |
| 运行时 | Node.js，深度耦合 `openclaw/plugin-sdk`（routing / session / reply） |
| 可剥离层 | `api/types.ts` + `api/api.ts` + `auth/login-qr.ts` + `cdn/*` 约 6 个文件无 SDK 依赖，可直接复用 |

### 2.2 MaxClaw 侧

| 项目 | 说明 |
|------|------|
| 语言 | Rust（Tokio 异步运行时） |
| 网关 | Axum HTTP，端口 42617 |
| WebSocket | `GET /ws/chat`，JSON 协议（message/chunk/reasoning/tool_call/tool_result/done/error） |
| Webhook | `POST /webhook` 使用 `run_gateway_chat_simple`（无工具循环）；平台专用路由使用 `run_gateway_chat_with_tools` |
| Channel trait | `name()` + `send(&SendMessage)` + `listen(tx: Sender)` + typing/draft/reaction 等可选方法 |
| 已有通道 | Telegram / Discord / Slack / WhatsApp / WeCom / Lark / DingTalk / Email 等 22+ |
| 消息处理 | `Channel::listen` → mpsc(100) → `run_message_dispatch_loop` → `process_channel_message` → `run_tool_call_loop`（完整工具循环） |
| Docker | `docker-compose.yml`，端口 42617，`zeroclaw-data` 卷，Debian slim 镜像 |

---

## 3. 可行性判定

结论：完全可行。

依据：

1. **协议兼容**：iLink Bot API 是纯 HTTP JSON，无需微信官方 SDK，任何语言/运行时均可调用
2. **架构模式已验证**：MaxClaw 的 Telegram Channel 采用完全相同的长轮询模式（`getUpdates` + offset 游标），iLink 的 `getupdates` + `get_updates_buf` 游标在协议结构上高度相似
3. **Docker 网络无障碍**：长轮询是出站 HTTPS 连接（容器主动连微信服务器），无需入站端口，无需公网 IP
4. **登录凭证可持久化**：`bot_token` 存入 config 或挂载卷即可跨容器重启保持

注意：iLink 的回复机制与 Telegram 存在本质差异（详见第 6 节），但不影响整体可行性。

---

## 4. 方案选型

### 4.1 候选方案对比

| 维度 | 方案 A：Rust 原生通道 | 方案 B：Node.js 桥接服务 | 方案 C：OpenClaw 中转 |
|------|----------------------|------------------------|--------------------|
| 架构复杂度 | 低（单进程） | 中（双容器） | 高（完整 OpenClaw + 配置） |
| 延迟 | 最低（进程内） | 中等（+1 次 HTTP） | 高（+2 次转发） |
| 资源占用 | 最小（Rust 二进制内） | +Node.js 容器约 60-100MB | +OpenClaw 容器约 200MB+ |
| 开发量 | 中（~600-800 行 Rust，含持久化和错误处理） | 中（~300 行 TS + Docker 集成） | 低（配置为主）但调试困难 |
| 与现有架构一致性 | 完全一致（主动连接型 Channel，走路径一） | 异构（引入 Node.js 依赖） | 异构（引入完整外部系统） |
| 数据传输路径 | 路径一：listen → mpsc → run_tool_call_loop（完整工具循环） | 路径三/四：WebSocket 或 /webhook（/webhook 无工具循环） | 多层转发 |
| 会话隔离 | 自然集成（{channel}_{sender}） | 需额外处理（WebSocket 为连接级会话） | 受限于 OpenClaw 配置 |
| 维护成本 | 低（同仓库、同语言） | 中（需维护两种语言） | 高（需跟踪 OpenClaw 版本） |
| Cron 投递 | 可支持 | 需额外实现 | 不支持 |

### 4.2 其他路径排除

基于架构文档定义的五类数据传输路径，逐一评估后排除：

| 路径 | 是否适用 | 原因 |
|------|---------|------|
| 路径一：主动连接型 Channel | 适用（方案 A） | iLink 长轮询完全符合此模式 |
| 路径二：被动回调型 Channel | 不适用 | iLink API 不支持 Webhook 回调，微信侧不会主动推送消息到 HTTP 端点 |
| 路径三：Gateway WebSocket | 可行但不理想 | WebSocket 会话是连接级的，微信多用户场景需要用户级会话隔离 |
| 路径四：通用 Webhook | 不可接受 | `/webhook` 使用 `run_gateway_chat_simple`，无工具循环 |
| 路径五：Gateway SSE | 不适用 | 仅为只读观测通道，不能触发 Agent |

### 4.3 选定方案：方案 A — Rust 原生 Channel 实现

理由：
- 方案 A 走路径一，是唯一同时满足完整工具循环 + 用户级会话隔离 + 架构一致性的路径
- MaxClaw 是 Rust-first 项目，已有 22+ 通道的 trait 实现模式，增加一个微信通道是标准操作
- iLink API 协议简单（4 个核心端点），Rust 的 `reqwest` + `serde` 完全胜任
- 无需引入 Node.js 运行时，保持单二进制部署优势
- 长轮询是容器主动出站 HTTPS 连接，无需暴露额外端口

---

## 5. 架构详细设计

### 5.1 整体架构

```
┌─────────────────────────────────────────────────────────┐
│                    Docker Container                      │
│                                                          │
│  ┌────────────────────────────────────────────────────┐  │
│  │               MaxClaw (Rust Binary)                │  │
│  │                                                    │  │
│  │  ┌──────────┐   ┌──────────┐   ┌───────────────┐  │  │
│  │  │ Gateway  │   │ Agent    │   │ Providers     │  │  │
│  │  │ (HTTP/WS)│   │ Loop     │   │ (LLM)        │  │  │
│  │  └────┬─────┘   └────┬─────┘   └───────────────┘  │  │
│  │       │              │                             │  │
│  │       │         ┌────┴─────┐                       │  │
│  │       │         │ Channels │                       │  │
│  │       │         │ Dispatch │                       │  │
│  │       │         └────┬─────┘                       │  │
│  │       │    ┌─────────┼─────────┐                   │  │
│  │       │    │         │         │                    │  │
│  │  ┌────┴──┐│  ┌──────┴───┐  ┌──┴───────┐           │  │
│  │  │Web UI ││  │ Telegram │  │ WeiXin   │  ...      │  │
│  │  │(WS)   ││  │ Channel  │  │ Channel  │           │  │
│  │  └───────┘│  └──────────┘  └────┬─────┘           │  │
│  │           │                     │                  │  │
│  └───────────┼─────────────────────┼──────────────────┘  │
│              │                     │                      │
└──────────────┼─────────────────────┼──────────────────────┘
               │                     │ HTTPS (出站)
               │                     ▼
          用户浏览器          ilinkai.weixin.qq.com
                              (微信 iLink Bot API)
```

数据传输路径（路径一）：

```
微信用户 → iLink API → WeixinChannel.listen()（长轮询）
  → tx.send(ChannelMessage) → mpsc 消息总线
  → run_message_dispatch_loop → process_channel_message
  → run_tool_call_loop（完整工具循环）
  → WeixinChannel.send(SendMessage)
  → iLink API → 微信用户
```

### 5.2 与 Telegram Channel 的对比

微信 iLink Channel 在长轮询层面与 Telegram Channel 同构，但在回复机制上存在本质差异：

| 对比维度 | Telegram | 微信 iLink |
|----------|----------|-----------|
| 长轮询拉取 | `POST getUpdates` + `offset` 整数游标 | `POST getupdates` + `get_updates_buf` 字符串游标 |
| 回复目标标识 | `chat_id`（数字，全局稳定） | `to_user_id` + `context_token`（来自入站消息，必须携带） |
| 主动发消息 | 可随时向任意已知 chat_id 发送 | 必须有用户的 `context_token`，否则无法发送 |
| 超时与错误 | HTTP 错误 sleep 5s；409 冲突 sleep 35s | errcode -14 会话过期，需重新扫码登录 |
| 在 SendMessage 中的映射 | `recipient` = chat_id（直接可用） | `recipient` = from_user_id（但 send 时还需内部查找 context_token） |

关键差异：Telegram 的 `send()` 是无状态的（只需 chat_id），而微信 iLink 的 `send()` 是有状态的（需要 context_token），这要求 WeixinChannel 在 `listen()` 和 `send()` 之间维护内部状态。

### 5.3 新增文件与模块

```
src/channels/
  weixin.rs              # WeiXin Channel 实现（~600-800 行）
  mod.rs                 # 注册 weixin channel 到工厂

src/config/
  schema.rs              # 添加 WeixinConfig 配置结构

src/cron/
  scheduler.rs           # deliver_announcement 添加 weixin 分支
```

### 5.4 配置结构（schema.rs 新增）

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct WeixinConfig {
    /// 是否启用微信通道
    pub enabled: Option<bool>,
    /// iLink Bot Token（扫码登录后获取）
    pub bot_token: Option<String>,
    /// iLink API 基地址（默认 https://ilinkai.weixin.qq.com）
    pub base_url: Option<String>,
    /// 允许对话的微信用户 ID 白名单（为空则允许所有）
    pub allowed_users: Option<Vec<String>>,
    /// 长轮询超时（ms，默认 35000）
    pub poll_timeout_ms: Option<u64>,
}
```

TOML 配置示例：

```toml
[channels.weixin]
enabled = true
bot_token = "your_bot_token_from_scan_login"
# base_url = "https://ilinkai.weixin.qq.com"  # 默认值
# allowed_users = ["wxid_xxx"]                 # 可选白名单
```

### 5.5 ChannelMessage 字段映射

从 iLink API 响应到 MaxClaw `ChannelMessage` 结构体的字段映射：

| ChannelMessage 字段 | iLink 来源 | 说明 |
|---------------------|-----------|------|
| id | `msg_id`（入站消息的唯一 ID） | 用于去重和 followup_thread_id |
| sender | `from_user_id` | 微信用户标识 |
| reply_target | `from_user_id` | send() 时用作 SendMessage.recipient，同时作为 context_token 查找键 |
| content | `item_list[0].text_item.text`（type=1 时） | 文本消息内容 |
| channel | `"weixin"` | 固定值 |
| timestamp | `create_time` 或系统当前时间戳 | 消息时间 |
| thread_ts | `None` | iLink 无线程概念 |

会话隔离 key 自动生成为 `weixin_{from_user_id}`。

### 5.6 Channel 实现要点

```rust
pub struct WeixinChannel {
    config: WeixinConfig,
    http_client: reqwest::Client,
    /// sender_id → latest context_token（内存缓存 + 磁盘持久化）
    context_tokens: Arc<RwLock<HashMap<String, String>>>,
    /// context_token 持久化路径
    token_store_path: PathBuf,
}

impl Channel for WeixinChannel {
    fn name(&self) -> &str { "weixin" }

    // listen(): 长轮询循环
    // 1. 启动时从磁盘加载已有 context_token 映射
    // 2. POST getupdates（带 get_updates_buf 游标）
    // 3. 解析 msgs 数组，过滤 message_type==1（用户消息）
    // 4. allowed_users 白名单检查（在投递 mpsc 前过滤）
    // 5. 更新 context_token 映射（内存 + 异步写磁盘）
    // 6. 构建 ChannelMessage，通过 tx.send() 投递
    // 7. 错误处理：HTTP 错误 sleep 5s 重试；errcode -14 停止并告警

    // send(): 发送回复
    // 1. 从内存 HashMap 查找 message.recipient 对应的 context_token
    // 2. 若内存无命中，尝试从磁盘加载
    // 3. 若均无，返回 anyhow::bail! 明确错误
    // 4. POST sendmessage，body 含 context_token + text
    // 5. 长消息自动分段（参考 Telegram 的 4096 字符限制处理）
}
```

### 5.7 关键协议实现

请求头构造：

```rust
fn build_headers(token: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", "application/json".parse().unwrap());
    headers.insert("AuthorizationType", "ilink_bot_token".parse().unwrap());
    headers.insert("Authorization", format!("Bearer {}", token).parse().unwrap());
    let uin = rand::random::<u32>().to_string();
    let uin_b64 = base64::engine::general_purpose::STANDARD.encode(uin.as_bytes());
    headers.insert("X-WECHAT-UIN", uin_b64.parse().unwrap());
    headers
}
```

长轮询消息拉取：

```rust
async fn poll_messages(&self, cursor: &str) -> Result<(Vec<WeixinMessage>, String)> {
    let resp = self.http_client
        .post(format!("{}/ilink/bot/getupdates", self.base_url()))
        .headers(self.build_headers())
        .json(&json!({
            "get_updates_buf": cursor,
            "base_info": { "channel_version": "" }
        }))
        .timeout(Duration::from_millis(self.poll_timeout()))
        .send().await?;
    let data: GetUpdatesResp = resp.json().await?;
    Ok((data.msgs, data.get_updates_buf))
}
```

发送消息：

```rust
async fn send_text(&self, to: &str, text: &str, context_token: &str) -> Result<()> {
    let client_id = format!("maxclaw_{}", uuid::Uuid::new_v4());
    self.http_client
        .post(format!("{}/ilink/bot/sendmessage", self.base_url()))
        .headers(self.build_headers())
        .json(&json!({
            "to_user_id": to,
            "context_token": context_token,
            "client_msg_id": client_id,
            "item_list": [{
                "type": 1,
                "text_item": { "text": text }
            }]
        }))
        .send().await?;
    Ok(())
}
```

### 5.8 Draft/Typing 可行性

| 能力 | Channel trait 方法 | iLink API 支持 | 实施建议 |
|------|-------------------|---------------|---------|
| Typing 指示 | `start_typing()` / `stop_typing()` | `POST sendtyping`（已知可用） | 阶段 2 实现 |
| Draft 流式编辑 | `supports_draft_updates()` / `send_draft()` / `update_draft()` | 待验证——iLink 可能不支持消息编辑/更新 | 暂不实现，待 API 验证后决定 |
| 表情回应 | `add_reaction()` / `remove_reaction()` | 未知 | 暂不实现 |
| 消息置顶 | `pin_message()` / `unpin_message()` | 未知 | 暂不实现 |

---

## 6. context_token 管理方案

### 6.1 背景

iLink API 的 `sendmessage` 接口必须携带 `context_token`（来自用户最近一条入站消息）。这是微信 iLink 与 Telegram/Discord 等平台的关键差异——后者仅需 `chat_id` 即可无条件发送消息。

MaxClaw 的 `SendMessage` 结构体不包含平台特定的上下文字段：

```rust
pub struct SendMessage {
    pub content: String,
    pub recipient: String,       // 来自 ChannelMessage.reply_target
    pub subject: Option<String>,
    pub thread_ts: Option<String>,
}
```

因此 `context_token` 必须由 `WeixinChannel` 内部管理，在 `send()` 方法被调用时，通过 `message.recipient`（即 `from_user_id`）查找对应的 `context_token`。

### 6.2 存储方案

采用内存缓存 + 磁盘持久化双层架构：

```
WeixinChannel
├── context_tokens: Arc<RwLock<HashMap<String, String>>>  ← 内存（热路径）
└── token_store_path: PathBuf                              ← 磁盘（持久化）
    → {workspace}/weixin/context_tokens.json
```

生命周期：

| 阶段 | 操作 |
|------|------|
| Channel 启动 | 从磁盘 `context_tokens.json` 加载到内存 HashMap |
| listen() 收消息 | 更新内存 HashMap + 异步写磁盘 |
| send() 发消息 | 从内存 HashMap 查找；未命中则回退磁盘加载；均无则返回错误 |
| Channel 关闭 | 最终写入磁盘确保数据不丢 |

### 6.3 约束与限制

| 场景 | 是否可发消息 | 说明 |
|------|------------|------|
| 用户曾给 bot 发过消息 | 可以 | 有缓存的 context_token |
| 用户从未与 bot 对话 | 不行 | 无 context_token，协议不允许主动推送 |
| bot 重启后，对之前聊过的用户 | 可以 | context_token 已持久化到磁盘 |
| context_token 过期 | 不行 | 需用户再发一条消息刷新 token |

实际使用中基本不构成限制——用户只需与 bot 发送一条消息即可激活后续所有回复和定时推送。

---

## 7. 登录与会话过期处理

### 7.1 登录流程

扫码登录是一次性操作，获取的 `bot_token` 长期有效。两种方式：

方式 1：配置文件直接填入 token（推荐初期）

- 用户在本机运行一个轻量登录脚本（可从插件抽取），扫码获取 token
- 将 token 填入 `config.toml` 的 `channels.weixin.bot_token`
- MaxClaw 启动时读取并使用

方式 2：内置登录 CLI（后续增强）

- `zeroclaw weixin login` 命令
- 调用 `get_bot_qrcode` 获取二维码，终端展示
- 长轮询 `get_qrcode_status` 等待扫码
- 成功后将 token 写入 config

### 7.2 会话过期处理

微信侧 `errcode: -14` 表示会话过期（token 失效）。处理策略：

1. 检测到 -14 后，停止长轮询
2. 通过 Observer 发出告警事件（Web UI / 日志可见）
3. 等待用户重新扫码登录并更新 token
4. 可选：在 config 中支持 `auto_relogin: true`，自动触发 QR 码生成

### 7.3 Docker 部署

- 长轮询是容器主动出站 HTTPS 连接 → 无需暴露额外端口
- 无需公网 IP 或域名
- 无需 reverse proxy 或 tunnel
- 只需容器有出站网络访问权限（默认即有）

---

## 8. 定时消息（Cron）投递微信

### 8.1 现有 Cron 投递机制

MaxClaw 的定时任务系统内置消息投递能力。每个 CronJob 包含 `DeliveryConfig`：

```rust
pub struct DeliveryConfig {
    pub mode: String,             // "none" | "announce"
    pub channel: Option<String>,  // "telegram" | "discord" | "weixin" ...
    pub to: Option<String>,       // 目标用户/频道 ID
    pub best_effort: bool,        // 投递失败是否影响任务状态
}
```

当 `mode = "announce"` 时，`scheduler.rs` 的 `deliver_announcement` 函数根据 `channel` 字段路由到对应通道的 `send()` 方法。当前已支持：telegram、discord、slack、mattermost、signal、matrix。

### 8.2 微信投递的实现要点

与 Telegram/Discord 不同，微信投递需要处理 context_token 依赖：

| 问题 | 解决方案 |
|------|---------|
| `deliver_announcement` 各分支直接构造 Channel 实例并调用 `send()` | WeixinChannel 构造时从磁盘加载 context_token 映射 |
| 新构造的 WeixinChannel 实例无内存中的 token 缓存 | 在 `WeixinChannel::new()` 中调用 `load_tokens_from_disk()`，确保新实例也能使用已持久化的 token |
| token 过期导致投递失败 | 设置 `best_effort: true`，通过 Observer 告警，提示用户需与 bot 交互一次以刷新 token |
| 用户从未与 bot 对话 | `send()` 返回明确错误信息（"无 context_token，请先与 bot 发送一条消息"） |

配置示例：

```toml
[cron.jobs.daily_report]
schedule = "0 9 * * *"
prompt = "生成今日工作摘要"
job_type = "agent"
delivery.mode = "announce"
delivery.channel = "weixin"
delivery.to = "wxid_xxx"
delivery.best_effort = true
```

### 8.3 实现步骤

在 `deliver_announcement` 的 `match` 分支中添加 `"weixin"` 分支：

1. 从配置构造 `WeixinChannel` 实例（自动加载磁盘 token）
2. 调用 `channel.send(SendMessage::new(text, to_user_id))`
3. `send()` 内部通过 `recipient` 查找 `context_token`
4. 若 token 不存在，返回错误由 `best_effort` 控制是否影响任务状态

---

## 9. 实施路线图

### 阶段 1：核心通道（MVP，约 2-3 天）

| 任务 | 说明 |
|------|------|
| 定义 `WeixinConfig` | schema.rs 添加配置结构 |
| 实现 `WeixinChannel` | weixin.rs：getupdates 长轮询 + sendmessage |
| 注册到工厂 | channels/mod.rs `collect_configured_channels` 添加 weixin 分支 |
| ChannelMessage 字段映射 | 按 5.5 节定义的映射表实现 |
| context_token 管理 | `Arc&lt;RwLock&lt;HashMap>>` + 磁盘持久化（context_tokens.json） |
| 会话过期检测 | errcode -14 处理 + Observer 告警 |
| 用户白名单 | `allowed_users` 在 listen() 中过滤，避免无效消息进入 mpsc 总线 |
| 长消息分段 | 超长文本自动拆分发送 |
| Cron 投递 | deliver_announcement 添加 weixin 分支 |
| 配置文档 | README 添加微信配置说明 |

### 阶段 2：登录与增强（可选，约 1 天）

| 任务 | 说明 |
|------|------|
| 扫码登录 CLI | `zeroclaw weixin login` 终端二维码 |
| typing 指示器 | `start_typing` / `stop_typing` 实现 `sendtyping` |
| 游标持久化 | get_updates_buf 写入文件，重启不丢消息 |

### 阶段 3：多媒体（可选，约 1-2 天）

| 任务 | 说明 |
|------|------|
| 图片接收 | CDN 下载 + AES-ECB 解密 |
| 图片发送 | AES-ECB 加密 + CDN 上传 |
| 语音支持 | SILK 解码（需 FFI 或外部工具） |

### 阶段 4：流式编辑（可选，待 API 验证）

| 任务 | 说明 |
|------|------|
| Draft 可行性验证 | 确认 iLink 是否支持消息编辑/更新 |
| Draft 实现 | 若支持，实现 `supports_draft_updates` / `send_draft` / `update_draft` |

---

## 10. 风险与缓解

| 风险 | 影响 | 风险等级 | 缓解措施 |
|------|------|---------|---------|
| iLink API 非公开文档，接口可能变更 | 通道失效 | 高 | 关注插件 npm 包更新；实现 API 响应结构化日志便于变更定位；错误码监控 |
| bot_token 过期频率未知 | 需重新扫码 | 中 | 实现过期检测 + Observer 告警；探索 token 刷新机制 |
| context_token 管理复杂度 | 多实例同步、过期、Cron 场景 | 中 | 磁盘持久化 + 启动预加载；send() 失败时返回明确错误 |
| 腾讯可能限流或封禁非官方调用 | 服务中断 | 中 | 遵守协议规范；控制请求频率；避免滥用 |
| 单一 bot_token 并发限制 | 多设备冲突 | 未知 | 确保仅单实例长轮询；处理可能的 409 类冲突 |
| 微信侧无 Webhook 回调 | 只能长轮询 | 低 | 已是设计方案，与 Telegram 同模式 |
| 长消息可能超微信限制 | 消息截断 | 低 | 实现自动分段发送 |
| MVP 仅处理 text（type=1） | 非文本消息被丢弃 | 低 | 对不支持的消息类型回复提示文案 |

---

## 11. 结论

| 项目 | 结论 |
|------|------|
| 可行性 | 完全可行，协议简单、架构匹配 |
| 选定方案 | 方案 A：Rust 原生 Channel 实现 |
| 数据传输路径 | 路径一：主动连接型 Channel（listen → mpsc → run_tool_call_loop） |
| 与 Telegram 的关系 | 长轮询同构，回复机制有差异（context_token 依赖） |
| 开发量 | MVP 约 600-800 行 Rust 代码（含 context_token 持久化、错误处理） |
| 核心依赖 | 无额外外部依赖（reqwest/serde 已在项目中） |
| 部署影响 | 零：无需额外端口、容器或基础设施 |
