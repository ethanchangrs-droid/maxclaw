# F010 SSE Chat 端点（POST + SSE 流式响应）

| 项目 | 内容 |
|------|------|
| Feature ID | F010 |
| 版本 | V1.0 |
| 日期 | 2026-03-26 17:34 (UTC+8) |
| 优先级 | P0 |
| 状态 | 待开发 |
| 前置依赖 | F003 Agent 核心 Loop 统一、F004 WebSocket 路径能力补齐 |

---

## 1. 目标

在 MaxClaw Gateway 中新增 `POST /api/chat` 端点，以 SSE（Server-Sent Events）流式返回 `AgentEvent` 事件。使客户端能通过标准 HTTP 协议获得与 WebSocket `/ws/chat` 完全一致的实时事件流体验（reasoning、tool_call、tool_result、chunk、done）。

---

## 2. 动机

### 2.1 远程连接需求

鸿蒙评测 APP 需要支持远程连接（非局域网），POST + SSE 相比 WebSocket 在远程场景下具备显著优势：

| 维度 | POST + SSE | WebSocket |
|------|-----------|-----------|
| CDN / 反向代理 | 天然支持（标准 HTTP） | 需专门配置 WS 转发和超时 |
| 防火墙穿透 | 走 80/443 端口，与普通网页请求无异 | 部分防火墙拦截 WS Upgrade |
| 移动网络切换 | 每次请求独立，WiFi ↔ 4G 切换无感 | 连接必断，需重连 |
| TLS | 标准 HTTPS | WSS 需额外配置 |
| Cloudflare Tunnel | 零配置支持 | 支持但需注意超时 |

### 2.2 行业标准

POST + SSE 是当前 AI API 的事实标准（OpenAI、Anthropic、Google 均采用此模式）。

---

## 3. 技术方案

### 3.1 架构位置

```
Gateway 路由层
├── /ws/chat          GET    WebSocket（现有，不变）
├── /api/events       GET    SSE 系统事件（现有，不变）
├── /api/chat         POST   SSE Chat（新增） ← 本 Feature
├── /webhook          POST   通用 Webhook（现有，不变）
├── /whatsapp         POST   WhatsApp（现有，不变）
└── /api/*            各方法  REST 管理接口（现有，不变）
```

### 3.2 数据流

```
APP 端                                 MaxClaw Gateway
  │                                      │
  │── POST /api/chat ───────────────────►│
  │   Headers:                           │
  │     Authorization: Bearer {token}    │
  │     Content-Type: application/json   │
  │     Accept: text/event-stream        │
  │   Body:                              │
  │     {"message": "...",               │
  │      "session_id": "app_{id}"}       │
  │                                      │
  │                    ┌─────────────────┤
  │                    │ handle_chat_sse │
  │                    │  ├─ 认证验证     │
  │                    │  ├─ Agent 创建   │
  │                    │  ├─ set_session  │
  │                    │  ├─ set_event_tx │
  │                    │  └─ agent.turn() │
  │                    │     └─ run_tool_call_loop()
  │                    └─────────────────┤
  │                                      │
  │◄── HTTP 200                          │
  │    Content-Type: text/event-stream   │
  │                                      │
  │◄── data: {"type":"reasoning",...}    │
  │◄── data: {"type":"tool_call",...}    │
  │◄── data: {"type":"tool_result",...}  │
  │◄── data: {"type":"chunk",...}        │
  │◄── data: {"type":"done",...}         │
  │                                      │
  │  （HTTP 连接关闭）                     │
```

### 3.3 事件格式

与 WebSocket `/ws/chat` 完全一致的 AgentEvent JSON 格式，封装为 SSE `data:` 行：

| AgentEvent 变体 | SSE data JSON `type` | 说明 |
|-----------------|---------------------|------|
| `Reasoning(content)` | `"reasoning"` | LLM 推理过程 |
| `ToolCallStart { name, arguments }` | `"tool_call"` | 工具调用请求 |
| `ToolCallComplete { name, output, success, duration_ms }` | `"tool_result"` | 工具执行结果 |
| `Chunk(content)` | `"chunk"` | 流式文本增量 |
| `Done { text, reasoning_content }` | `"done"` | 最终完整回复 |
| `Error { message }` | `"error"` | 错误信息 |

### 3.4 请求格式

```json
{
  "message": "用户输入的消息内容",
  "session_id": "app_{device_id}"
}
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| message | string | 是 | 用户消息内容 |
| session_id | string | 否 | 会话标识，用于 Memory 隔离和多轮对话。缺省时每次请求创建独立会话 |

---

## 4. 改动清单

### 4.1 新增文件

| 文件 | 说明 |
|------|------|
| `src/gateway/chat_sse.rs` | SSE Chat handler：认证、Agent 创建、event_sender 设置、agent.turn() 调用、AgentEvent → SSE 流转发 |

### 4.2 修改文件

| 文件 | 改动内容 | 改动量 |
|------|---------|--------|
| `src/gateway/mod.rs` | 1. `pub mod chat_sse;` 模块声明 | 1 行 |
| | 2. `.route("/api/chat", post(chat_sse::handle_chat_sse))` 路由注册 | 1 行 |
| | 3. SSE chat 路由独立超时配置（300s，豁免全局 30s 超时） | ~5 行 |

### 4.3 不改动的文件

| 组件 | 文件 | 原因 |
|------|------|------|
| Agent 核心 | `src/agent/agent.rs` | SSE handler 只调用 `Agent::from_config()` + `turn()`，不修改接口 |
| 核心 Loop | `src/agent/loop_.rs` | `run_tool_call_loop()` 通过 `event_sender` 发事件，SSE handler 只消费 |
| AgentEvent | `src/agent/agent.rs` | 事件枚举不变，SSE handler 做 JSON 序列化（可复用 ws.rs 逻辑） |
| Memory | `src/memory/` | 通过 `session_id` 隔离，机制不变 |
| 认证 | `src/security/pairing.rs` | 复用 PairingGuard Bearer Token 验证 |
| 所有 Channel | `src/channels/*` | 完全不涉及 |
| 所有现有 Gateway 端点 | `src/gateway/ws.rs` 等 | 独立路由，零交叉 |

### 4.4 代码量估算

| 项 | 行数 |
|----|------|
| `chat_sse.rs` 新增 | 60-80 行 |
| `mod.rs` 修改 | ~7 行 |
| 总计 | ~70-90 行 Rust |

---

## 5. 隔离与安全

### 5.1 session_id 隔离

| 通道 | session_id 前缀 | 互相隔离 |
|------|----------------|---------|
| 微信 Channel | `weixin_{sender}` | 是 |
| WebSocket | `ws_{conn_id}` | 是 |
| SSE Chat（新增） | `sse_{session_id}` 或用户传入 | 是 |

各通道 session_id 前缀不同，Memory recall/list 自动按 session_id 过滤，天然隔离。

### 5.2 认证

复用现有 PairingGuard Bearer Token 机制（与 `/api/events`、`/ws/chat` 一致）。

### 5.3 超时

SSE chat 路由独立配置 300 秒超时（agent 工具执行可能需要较长时间），不影响其他路由的 30 秒全局超时。

---

## 6. 多轮对话策略

| 阶段 | 方案 | 说明 |
|------|------|------|
| MVP | 单轮模式 | 每次 POST 创建新 Agent，无多轮历史。适合评测场景（一问一答） |
| 后续增强 | session 缓存 | AppState 增加 `HashMap&lt;session_id, Agent&gt;`（带 TTL 自动清理），支持连续多轮对话 |

---

## 7. 远程连接部署

SSE Chat 端点为远程连接提供基础。推荐部署方案：

| 方案 | 适用场景 | 说明 |
|------|---------|------|
| Cloudflare Tunnel | 推荐 | Mac 端安装 cloudflared，零端口暴露，自动 HTTPS，免费 |
| 云服务器部署 | 长期稳定运行 | MaxClaw 部署到 VPS，APP 直连公网地址 |

APP 端代码不区分局域网/远程——连接地址从 `http://192.168.x.x:42617` 变为 `https://maxclaw.yourdomain.com` 即可。

---

## 8. 影响范围

| 影响范围 | 说明 |
|---------|------|
| 现有端点 | 零影响。新增独立路由，不修改任何现有 handler |
| Channel 子系统 | 零影响。完全不涉及 Channel trait 或任何 Channel 实现 |
| Agent 核心 | 零影响。只读调用现有接口 |
| 配置文件 | 无需新增配置项 |
