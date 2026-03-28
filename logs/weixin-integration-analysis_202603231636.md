# 微信 iLink 接入 MaxClaw 可行性分析与架构方案

> 版本：V1.0
> 日期：2026-03-23 16:36 (UTC+8)

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
| 发消息 | `POST sendmessage`，必须携带 `context_token` |
| 媒体 | AES-128-ECB 加密上传 CDN，下载后解密 |
| 运行时 | Node.js，深度耦合 `openclaw/plugin-sdk`（routing / session / reply） |
| 可剥离层 | `api/types.ts` + `api/api.ts` + `auth/login-qr.ts` + `cdn/*` 约 6 个文件无 SDK 依赖，可直接复用 |

### 2.2 MaxClaw 侧

| 项目 | 说明 |
|------|------|
| 语言 | Rust（Tokio 异步运行时） |
| 网关 | Axum HTTP，端口 42617 |
| WebSocket | `GET /ws/chat`，JSON 协议（message/chunk/reasoning/tool_call/tool_result/done/error） |
| Webhook | `POST /webhook`，简单聊天（无工具）；各平台专用路由带工具 |
| Channel trait | `name()` + `send()` + `listen()` + typing/draft/reaction 等可选方法 |
| 已有通道 | Telegram / Discord / Slack / WhatsApp / WeCom / Lark / DingTalk / Email 等 20+ |
| 消息处理 | `Channel::listen` → mpsc → `run_message_dispatch_loop` → `run_tool_call_loop`（完整工具循环） |
| Docker | `docker-compose.yml`，端口 42617，`zeroclaw-data` 卷，Debian slim 镜像 |

---

## 3. 可行性判定

**结论：完全可行。**

依据：

1. **协议兼容**：iLink Bot API 是纯 HTTP JSON，无需微信官方 SDK，任何语言/运行时均可调用
2. **架构模式已验证**：MaxClaw 已有 Telegram（长轮询）和 WhatsApp（Webhook）等通道实现，微信 iLink 的长轮询模式与 Telegram Bot API 几乎同构
3. **Docker 网络无障碍**：长轮询是出站 HTTPS 连接（容器主动连微信服务器），无需入站端口，无需公网 IP
4. **登录凭证可持久化**：`bot_token` 存入 config 或挂载卷即可跨容器重启保持

---

## 4. 方案评估

### 4.1 三种候选方案对比

| 维度 | 方案 A：Rust 原生通道 | 方案 B：Node.js 桥接服务 | 方案 C：OpenClaw 中转 |
|------|----------------------|------------------------|--------------------|
| 架构复杂度 | 低（单进程） | 中（双容器） | 高（完整 OpenClaw + 配置） |
| 延迟 | 最低（进程内） | 中等（+1 次 HTTP） | 高（+2 次转发） |
| 资源占用 | 最小（Rust 二进制内） | +Node.js 容器约 60-100MB | +OpenClaw 容器约 200MB+ |
| 开发量 | 中（~400 行 Rust） | 中（~300 行 TS + Docker 集成） | 低（配置为主）但调试困难 |
| 与现有架构一致性 | 完全一致（和 Telegram/WhatsApp 同模式） | 异构（引入 Node.js 依赖） | 异构（引入完整外部系统） |
| 维护成本 | 低（同仓库、同语言） | 中（需维护两种语言） | 高（需跟踪 OpenClaw 版本） |
| 功能完整性 | 完整（工具循环、会话、typing、memory） | 取决于桥接深度 | 受限于 OpenClaw agent 转发配置 |

### 4.2 推荐方案：方案 A — Rust 原生 Channel 实现

理由：
- MaxClaw 是 Rust-first 项目，已有 20+ 通道的 trait 实现模式，增加一个微信通道是标准操作
- iLink API 协议简单（4 个核心端点），Rust 的 `reqwest` + `serde` 完全胜任
- 长轮询模式与已有 Telegram 通道同构，可复用经验
- 无需引入 Node.js 运行时，保持单二进制部署优势
- 与 Channel trait 体系自然集成，继承会话管理、工具循环、typing 指示等全部能力

---

## 5. 推荐架构详细设计

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

### 5.2 新增文件与模块

```
src/channels/
  weixin.rs              # WeiXin Channel 实现（~400 行）
  mod.rs                 # 注册 weixin channel 到工厂

src/config/
  schema.rs              # 添加 WeixinConfig 配置结构
```

### 5.3 WeiXin Channel 核心设计

#### 5.3.1 配置结构（schema.rs 新增）

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

#### 5.3.2 Channel 实现要点

```rust
pub struct WeixinChannel {
    config: WeixinConfig,
    http_client: reqwest::Client,
}

impl Channel for WeixinChannel {
    fn name(&self) -> &str { "weixin" }

    // listen(): 长轮询循环
    // 1. POST getupdates（带 get_updates_buf 游标）
    // 2. 解析 msgs 数组，过滤 message_type==1（用户消息）
    // 3. 构建 ChannelMessage，通过 tx.send() 投递
    // 4. 保存 context_token 到 session 关联表（发消息时需要）

    // send(): 发送回复
    // 1. 从 session 关联表取 context_token
    // 2. POST sendmessage，body 含 context_token + text
}
```

#### 5.3.3 关键协议实现

**请求头构造**：

```rust
fn build_headers(token: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", "application/json".parse().unwrap());
    headers.insert("AuthorizationType", "ilink_bot_token".parse().unwrap());
    headers.insert("Authorization", format!("Bearer {}", token).parse().unwrap());
    // X-WECHAT-UIN: 随机 u32 的 base64
    let uin = rand::random::<u32>().to_string();
    let uin_b64 = base64::engine::general_purpose::STANDARD.encode(uin.as_bytes());
    headers.insert("X-WECHAT-UIN", uin_b64.parse().unwrap());
    headers
}
```

**长轮询消息拉取**：

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

**发送消息**：

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

#### 5.3.4 context_token 管理

iLink API 要求回复消息时必须携带 `context_token`（来自入站消息）。设计：

- 在 `WeixinChannel` 内维护 `HashMap&lt;String, String>`（sender_id → latest context_token）
- `listen()` 收到消息时更新映射
- `send()` 回复时查找映射
- 使用 `Arc&lt;RwLock&lt;HashMap>>` 保证线程安全

### 5.4 登录流程设计

扫码登录是一次性操作，获取的 `bot_token` 长期有效。两种方式：

**方式 1：配置文件直接填入 token（推荐初期）**

- 用户在本机运行一个轻量登录脚本（可从插件抽取），扫码获取 token
- 将 token 填入 `config.toml` 的 `channels.weixin.bot_token`
- MaxClaw 启动时读取并使用

**方式 2：内置登录 CLI（后续增强）**

- `zeroclaw weixin login` 命令
- 调用 `get_bot_qrcode` 获取二维码，终端展示
- 长轮询 `get_qrcode_status` 等待扫码
- 成功后将 token 写入 config

### 5.5 会话过期处理

微信侧 `errcode: -14` 表示会话过期（token 失效）。处理策略：

1. 检测到 -14 后，停止长轮询
2. 通过 Observer 发出告警事件（Web UI / 日志可见）
3. 等待用户重新扫码登录并更新 token
4. 可选：在 config 中支持 `auto_relogin: true`，自动触发 QR 码生成

### 5.6 Docker 部署无需额外配置

- 长轮询是容器**主动出站** HTTPS 连接 → 无需暴露额外端口
- 无需公网 IP 或域名
- 无需 reverse proxy 或 tunnel
- 只需容器有出站网络访问权限（默认即有）

---

## 6. 实施路线图

### 阶段 1：核心通道（MVP，约 1-2 天）

| 任务 | 说明 |
|------|------|
| 定义 `WeixinConfig` | schema.rs 添加配置结构 |
| 实现 `WeixinChannel` | weixin.rs：getupdates 长轮询 + sendmessage |
| 注册到工厂 | channels/mod.rs 添加 weixin 分支 |
| context_token 管理 | Arc&lt;RwLock&lt;HashMap>> 存储 sender → token 映射 |
| 会话过期检测 | errcode -14 处理 + 日志告警 |
| 配置文档 | README 添加微信配置说明 |

### 阶段 2：登录与增强（可选，约 1 天）

| 任务 | 说明 |
|------|------|
| 扫码登录 CLI | `zeroclaw weixin login` 终端二维码 |
| typing 指示器 | `sendtyping` 支持 |
| 用户白名单 | `allowed_users` 过滤 |
| 游标持久化 | get_updates_buf 写入文件，重启不丢消息 |

### 阶段 3：多媒体（可选，约 1-2 天）

| 任务 | 说明 |
|------|------|
| 图片接收 | CDN 下载 + AES-ECB 解密 |
| 图片发送 | AES-ECB 加密 + CDN 上传 |
| 语音支持 | SILK 解码（需 FFI 或外部工具） |

---

## 7. 定时消息（Cron）投递微信

### 7.1 现有 Cron 投递机制

MaxClaw 的定时任务系统内置了消息投递能力。每个 CronJob 包含 `DeliveryConfig`：

```rust
pub struct DeliveryConfig {
    pub mode: String,        // "none" | "announce"
    pub channel: Option<String>,  // "telegram" | "discord" | "weixin" ...
    pub to: Option<String>,       // 目标用户/频道 ID
    pub best_effort: bool,        // 投递失败是否影响任务状态
}
```

当 `mode = "announce"` 时，`scheduler.rs` 的 `deliver_announcement` 函数根据 `channel` 字段路由到对应通道的 `send()` 方法。当前已支持：telegram、discord、slack、mattermost、signal、matrix。

### 7.2 微信投递：可行，需补充实现

实现微信 Channel 后，只需在 `deliver_announcement` 的 `match` 分支中添加 `"weixin"` 即可（约 10 行代码）。配置示例：

```toml
# 定时任务投递到微信
[cron.jobs.daily_report]
schedule = "0 9 * * *"
prompt = "生成今日工作摘要"
job_type = "agent"
delivery.mode = "announce"
delivery.channel = "weixin"
delivery.to = "wxid_xxx"
```

### 7.3 协议约束：`context_token` 要求

微信 iLink API 的 `sendmessage` 接口**必须携带 `context_token`**（来自用户最近一条入站消息），这与 Telegram/Discord 的无条件推送不同。

| 场景 | 是否可行 | 说明 |
|------|---------|------|
| 用户曾给 bot 发过消息，定时任务推送结果 | 可以 | 有缓存的 context_token |
| 用户从未与 bot 对话，定时任务主动推送 | 不行 | 无 context_token，协议不允许 |
| bot 重启后，对之前聊过的用户推送 | 可以 | 需持久化 context_token 到磁盘 |

### 7.4 解决方案

在微信 Channel 实现中持久化 `context_token`：

1. `listen()` 收消息时 — 将每个用户的最新 `context_token` 写入磁盘（`zeroclaw-data` 卷内的 JSON 文件）
2. `send()` 发消息时 — 从持久化存储读取该用户的 `context_token`
3. Cron `deliver_announcement` 中 `"weixin"` 分支正常调用 `channel.send()`，内部自动查找 token
4. 若 token 不存在（用户从未聊过），返回明确错误信息

实际使用中基本不构成限制 — 用户只需与 bot 发送一条消息即可激活后续所有定时推送。

---

## 8. 风险与缓解

| 风险 | 影响 | 缓解措施 |
|------|------|---------|
| iLink API 非公开文档，接口可能变更 | 通道失效 | 关注插件 npm 包更新；错误码监控 |
| bot_token 过期频率未知 | 需重新扫码 | 实现过期检测 + 告警；探索 token 刷新机制 |
| 腾讯可能限流或封禁非官方调用 | 服务中断 | 遵守协议规范；控制请求频率；避免滥用 |
| 微信侧无 Webhook 回调 | 只能长轮询 | 已是设计方案，与 Telegram 同模式 |
| 长消息可能超微信限制 | 消息截断 | 实现自动分段发送（类似 Telegram 4096 字符限制处理） |

---

## 9. 与方案 B（Node.js 桥接）的补充说明

如果因某种原因（如需快速验证、团队更熟悉 TypeScript）选择方案 B：

```
┌─────────────────────────────────────────────────┐
│              Docker Compose                      │
│                                                  │
│  ┌─────────────┐       ┌────────────────────┐   │
│  │ weixin-     │ HTTP  │ MaxClaw            │   │
│  │ bridge      ├──────►│ (POST /webhook     │   │
│  │ (Node.js)   │       │  或 WebSocket)     │   │
│  └──────┬──────┘       └────────────────────┘   │
│         │                                        │
└─────────┼────────────────────────────────────────┘
          │ HTTPS
          ▼
  ilinkai.weixin.qq.com
```

- 从插件抽取 `api/types.ts` + `api/api.ts` + `auth/login-qr.ts`（约 6 个文件）
- 桥接服务收到微信消息后，转发到 MaxClaw 的 `/webhook` 或 WebSocket
- 缺点：`/webhook` 路径无工具循环；WebSocket 路径需维护连接状态
- 需额外 Dockerfile + docker-compose service + 网络配置

**不推荐此方案**，因为增加了运维复杂度且 MaxClaw 的 Channel 体系已经完美适配此场景。

---

## 10. 结论

| 项目 | 结论 |
|------|------|
| 可行性 | 完全可行，协议简单、架构匹配 |
| 推荐方案 | 方案 A：Rust 原生 Channel 实现 |
| 开发量 | MVP 约 400 行 Rust 代码 + 配置 |
| 核心依赖 | 无额外外部依赖（reqwest/serde 已在项目中） |
| 部署影响 | 零：无需额外端口、容器或基础设施 |
| 与现有架构兼容性 | 完全兼容，与 Telegram 通道同构 |
