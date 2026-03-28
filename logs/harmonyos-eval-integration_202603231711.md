# 鸿蒙评测 APP 与 MaxClaw 集成架构分析

> 版本：V5.0
> 日期：2026-03-26 10:47 (UTC+8)
>
> 前置依赖（P0，集成前完成）：
> - F003 Agent 核心 Loop 统一 → `docs/architecture/agent-loop-unification_V1.0_202603251749.md`
> - F004 WebSocket 路径能力补齐 → `docs/architecture/orchestration-paths-analysis_V1.0_202603251804.md`
>
> 更新记录：
> - V5.0 (2026-03-26 10:47)：APP 定位简化为薄客户端——去掉本地数据库、评测引擎、指标采集、结果分析面板，新增系统管理菜单
> - V4.0 (2026-03-25 18:23)：基于 F003/F004 完成后的架构重写——以"当前能力"视角描述，修正 AgentEvent 格式，新增网络拓扑
> - V3.0 (2026-03-23 19:17)：增强 WebSocket loop 方案
> - V2.0 (2026-03-23 17:30)：EvalApp Channel 模式（已废弃）
> - V1.0 (2026-03-23 17:11)：初始版本

---

## 1. 需求概述

1. 开发一个鸿蒙（HarmonyOS）APP，用于对已部署的 MaxClaw 进行自动化评测
2. APP 通过发送消息触发 MaxClaw 的 skill 执行评测，结果以 text 返回
3. APP 需要展示完整 loop 内容：reasoning、tool calling、流式输出
4. 日常使用 MaxClaw（微信通道）的会话历史、Memory 等数据不能被评测流量污染
5. 确定最佳隔离策略：单容器 vs 双容器

---

## 2. MaxClaw 现有架构深度分析

### 2.1 网关层（Gateway）

| 端点 | 方法 | 用途 | 工具支持 |
|------|------|------|----------|
| `/ws/chat` | WebSocket | 实时交互聊天 | 生产级 `run_tool_call_loop` + 结构化 `AgentEvent` 事件流 |
| `/webhook` | POST | 通用 Webhook | 无工具（`run_gateway_chat_simple`） |
| `/whatsapp` | POST | WhatsApp 专用 | 生产级 loop（`run_tool_call_loop`） |
| `/api/status` | GET | 系统状态 | N/A |
| `/api/memory` | GET/POST/DELETE | Memory CRUD | N/A |
| `/api/config` | GET/PUT | 配置管理 | N/A |
| `/health` | GET | 健康检查 | N/A |
| `/pair` | POST | 设备配对认证 | N/A |

### 2.2 WebSocket 通道能力

WebSocket（`/ws/chat`）的核心 loop 与 Channel 路径共享同一个 `run_tool_call_loop()` 引擎，具备完整生产能力：

| 特性 | 支持情况 |
|------|---------|
| 工具执行 | 有 |
| 多轮历史 | 连接内持久 |
| Approval / Cancellation | 有 |
| Hooks | 有 |
| Tool call 去重 | 有 |
| Multimodal | 有 |
| Credential scrubbing | 完整 |
| Observability | 完整 |
| Timeout budget | 有 |
| MCP 工具 | 有 |
| Peripheral 工具 | 有 |

**WebSocket 独有优势（Channel 没有）**：

| 能力 | 说明 |
|------|------|
| 结构化事件流 | 通过 `AgentEvent` 枚举以 JSON 帧推送 reasoning、tool_call、tool_result、chunk、done 等独立事件 |
| 实时流式传输 | 双向全双工，token 级增量推送，无需回调 |
| 连接级会话隔离 | 每个 WS 连接天然独立，无需 Channel/sender 拆分 |
| 前端直接消费 | APP 直接建立 WS 连接，无需额外 HTTP 回调接收服务 |

**结论**：WebSocket = 生产级执行 + 结构化事件流 + 实时推送，是评测 APP 展示完整 loop 内容的最优选择。

### 2.3 数据存储分层

MaxClaw 有 **4 层数据存储**，每层的隔离机制不同：

| 层 | 存储位置 | 作用域 | 隔离粒度 |
|----|----------|--------|----------|
| 会话历史（Channel） | `conversation_histories` HashMap + JSONL 文件 | `{channel}_{sender}` 或 `{channel}_{thread}_{sender}` | 发送者级：天然隔离 |
| Memory（长期记忆） | SQLite / Postgres / Lucid | 所有 `store/recall` 可传 `session_id` 做过滤 | session_id 级：需主动传参 |
| Core Memory（无 session） | 同上，但 session_id 为 NULL | 全局共享 | 无隔离 |
| Workspace（文件系统） | `/zeroclaw-data/workspace/` | 全局共享 | 无隔离 |

### 2.4 session_id 机制详解

```
Memory trait:
  store(key, content, category, session_id: Option<&str>)
  recall(query, limit, session_id: Option<&str>)
  list(category, session_id: Option<&str>)

WebSocket 场景:
  当前 ws.rs 中已有: agent.set_memory_session_id(Some("ws_{connection_id}"))
  → 每个 WS 连接的 Memory 操作按 connection_id 隔离

Channel 场景:
  sender_session_id("whatsapp", msg) → "whatsapp_{sender}" 或 "whatsapp_{thread}_{sender}"
  → 作为 session_id 传入 run_tool_call_loop 链路中的 Memory 操作

隔离原理:
  微信 session_id = "weixin_{sender}" → 只能 recall 到微信的 Memory
  WS   session_id = "ws_{conn_id}"    → 只能 recall 到该 WS 连接的 Memory
  → 两者天然互不干扰
```

### 2.5 WebSocket 现有认证机制

- PairingGuard：OTP 配对 → Bearer Token
- WebSocket 连接时通过 query parameter 或首条消息传递 Bearer Token
- 复用现有认证链路，无需额外鉴权实现

---

## 3. 鸿蒙 APP 与 MaxClaw 集成架构

### 3.1 集成方式：WebSocket 通道

鸿蒙评测 APP 通过 WebSocket（`/ws/chat`）与 MaxClaw 集成。该通道具备：
- 生产级执行能力（与微信 Channel 共享同一个 `run_tool_call_loop()` 核心引擎）
- 结构化事件流（`AgentEvent` 枚举以 JSON 帧推送每个中间步骤）
- 实时流式传输（双向全双工，token 级增量推送）

**为什么不用 Channel 模式**：Channel 的 `send()` 接口只能返回最终文本，无法推送中间步骤（reasoning、tool_call、tool_result）的结构化事件。评测 APP 的核心需求是"展示完整 loop 内容"。

### 3.2 调用链路

```
ws.rs handle_socket()
  └─ agent.turn(&content)           ← 外层编排（system prompt / memory / classify / cache）
       └─ run_tool_call_loop(...)   ← 核心引擎（与 Channel 共享同一函数）
            ├─ event_sender → AgentEvent → WS JSON 帧
            ├─ approval / dedup / hooks
            └─ tool execution + credential scrubbing
```

### 3.3 集成协议

```
鸿蒙 APP                                MaxClaw (WebSocket /ws/chat)
  │                                        │
  │──── WS Connect ────────────────────────►│
  │     ws://host:42617/ws/chat            │
  │     Authorization: Bearer {token}      │
  │                                        │
  │──── WS Text Frame ────────────────────►│
  │     {                                  │
  │       "type": "message",               │  ← 必须字段
  │       "content": "测试问题"             │
  │     }                                  │
  │                                        │
  │     ┌──────────────────────────────────┤
  │     │ WS handler                       │
  │     │ → agent.turn(&content)           │
  │     │   → set_memory_session_id        │
  │     │   → build system prompt          │
  │     │   → build_context (Memory RAG)   │
  │     │   → run_tool_call_loop (生产级)  │
  │     │     ├─ LLM call                  │
  │     │     ├─ parse tool calls          │
  │     │     ├─ approval check            │
  │     │     ├─ dedup check               │
  │     │     ├─ execute tools             │
  │     │     ├─ hooks (before/after)      │
  │     │     └─ loop until final answer   │
  │     └──────────────────────────────────┤
  │                                        │
  │◄─── WS JSON 帧（多条，按序推送） ──────│
  │                                        │
  │  {"type":"reasoning","content":"..."}  │  ← thinking/reasoning 内容
  │  {"type":"tool_call","name":"shell",   │  ← 工具调用请求
  │    "args":{...}}                       │
  │  {"type":"tool_result","name":"shell", │  ← 工具执行结果
  │    "output":"...","success":true}      │
  │  {"type":"chunk","content":"部分"}      │  ← 流式文本增量
  │  {"type":"chunk","content":"回复"}      │
  │  {"type":"done","full_response":"..."}  │  ← 最终完整回复
  │                                        │
  │──── 下一条消息（多轮对话） ─────────────►│
  │     (连接保持，会话历史自动累积)          │
```

**关键优势**：
- 无需回调 URL，所有事件通过 WS 帧实时推送
- 多轮对话天然支持（同一连接内会话自动累积）
- APP 侧只需建立一个 WS 连接，无需管理 HTTP 回调服务

### 3.4 配置方式

无需额外配置。WebSocket 复用现有 Gateway 端口和认证：

```toml
# config.toml — 无需新增任何字段
# WebSocket 使用现有 gateway 端口 42617
# 认证复用 PairingGuard Bearer Token
```

### 3.5 消息流经的完整路径

```
WS Client (鸿蒙 APP)
  │
  ▼
handle_ws_chat()                      ← Axum WebSocket upgrade
  │
  ▼
handle_socket()                       ← WS handler
  │
  ├─ 认证验证 (PairingGuard)
  ├─ Agent::from_config() 创建持久实例
  ├─ agent.set_memory_session_id("ws_{conn_id}")
  ├─ 启动 forward_agent_events 协程（AgentEvent → WS JSON 帧）
  │
  ▼ (每条用户消息)
  │
  agent.turn(&content)                ← 外层编排
  ├─ build system prompt（首轮）
  ├─ memory auto_save
  ├─ load Memory context (RAG, session_id scoped)
  ├─ classify_model (query routing)
  ├─ response cache check
  │
  └─ run_tool_call_loop()             ← 生产级核心引擎
       ├─ provider.chat()
       │    └─ event_sender → WS 帧 {"type":"chunk",...}
       ├─ parse_structured_tool_calls()
       ├─ approval_manager.needs_approval()
       ├─ tool_call_signature dedup
       ├─ hooks.run_before_tool_call()
       ├─ execute_one_tool()
       │    └─ event_sender → WS 帧 {"type":"tool_call",...} + {"type":"tool_result",...}
       ├─ hooks.run_after_tool_call()
       ├─ credential scrubbing
       └─ loop until final text
            └─ event_sender → WS 帧 {"type":"done","full_response":"..."}
```

### 3.6 网络拓扑

鸿蒙 APP 运行在手机上，MaxClaw 运行在电脑端的 Docker Desktop 中。两者需在同一局域网内通信。

```
┌─────────────────┐          局域网 (Wi-Fi / 有线)          ┌─────────────────────────┐
│  鸿蒙手机        │                                        │  电脑 (macOS)            │
│                 │                                        │                         │
│  评测 APP       │◄──── ws://192.168.x.x:42617/ws/chat ──►│  Docker Desktop          │
│                 │                                        │  ├─ 端口映射 42617:42617 │
│                 │◄──── http://192.168.x.x:42617/health ──►│  └─ MaxClaw 容器         │
└─────────────────┘                                        └─────────────────────────┘
```

| 项目 | 说明 |
|------|------|
| 连接地址 | `ws://{电脑局域网 IP}:42617/ws/chat`（非 localhost） |
| IP 获取 | 电脑端执行 `ifconfig en0` 获取局域网 IP，APP 中手动配置 |
| Docker 端口映射 | `docker-compose.yml` 已有 `ports: "42617:42617"`，宿主机可达 |
| 防火墙 | macOS 防火墙需允许 42617 端口入站 |
| 协议 | 开发环境使用 WS 明文（非 WSS），局域网内可接受 |

### 3.7 鸿蒙 APP 架构设计

APP 定位为 MaxClaw 的**薄客户端**——对话交互 + 系统管理入口。评测流程和结果分析由 Docker 中的 agent skill 完成，APP 只负责发消息触发和展示文字结果。

```
┌──────────────────────────────────────────────────┐
│            鸿蒙评测 APP（薄客户端）                 │
├──────────────────────────────────────────────────┤
│                                                  │
│  ┌────────────────────────────────────────────┐  │
│  │ 对话界面                                    │  │
│  │ - 发送消息（触发 skill / 自由对话）         │  │
│  │ - 实时展示 reasoning / tool call / chunk    │  │
│  │ - 评测结果以文字显示                        │  │
│  └────────────────────────────────────────────┘  │
│                                                  │
│  ┌────────────────────────────────────────────┐  │
│  │ 系统管理菜单（纯前端查看/修改，数据在 MaxClaw）│  │
│  │ - 系统状态（/api/status）                   │  │
│  │ - 配置查看/修改（/api/config）              │  │
│  │ - Memory 查看/删除（/api/memory）           │  │
│  │ - 工具列表（/api/tools）                    │  │
│  │ - 定时任务管理（/api/cron）                 │  │
│  │ - 费用查看（/api/cost）                     │  │
│  │ - 健康检查（/health）                       │  │
│  └────────────────────────────────────────────┘  │
│                                                  │
│  ┌────────────────────────────────────────────┐  │
│  │ 核心服务层                                  │  │
│  │ - WebSocket 客户端（对话 + AgentEvent 解析）│  │
│  │ - HTTP 客户端（REST API 调用）              │  │
│  │ - 连接配置（MaxClaw IP:Port + Token）       │  │
│  │ - 自动重连（指数退避 + 心跳检测）           │  │
│  └────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────┘
         │ WS                  │ REST
         ▼                    ▼
┌──────────────────────────────────────────────────┐
│          MaxClaw（Docker Desktop, 同局域网）       │
│                                                  │
│  Gateway (:42617)                                │
│  ├─ /ws/chat    → 对话 + AgentEvent 事件流       │
│  ├─ /api/status   /api/config   /api/memory      │
│  ├─ /api/tools    /api/cron     /api/cost        │
│  ├─ /health     /api/events (SSE)                │
│  └─ /pair (设备配对)                              │
└──────────────────────────────────────────────────┘
```

### 3.8 REST API 用途

APP 的系统管理菜单通过以下已有 REST API 实现，APP 侧仅做前端展示/编辑：

| API | 用途 |
|-----|------|
| `GET /health` | 检查 MaxClaw 存活状态 |
| `GET /api/status` | 查看 Provider/Model/版本信息 |
| `GET /api/memory?query=...` | 查看 Memory 内容 |
| `DELETE /api/memory/{key}` | 删除 Memory 条目 |
| `GET /api/config` | 查看当前配置 |
| `PUT /api/config` | 修改配置 |
| `GET /api/tools` | 查看可用工具列表 |
| `GET/POST/DELETE /api/cron` | 管理定时任务 |
| `GET /api/cost` | 查看费用统计 |
| `GET /api/events` (SSE) | 实时系统事件流（可选） |

---

## 4. 隔离方案对比分析

### 4.1 需要隔离的数据维度

| 维度 | 微信日常使用 | 鸿蒙评测 APP（WebSocket） |
|------|------------|---------------------------|
| 会话历史（内存） | Channel: `weixin_{sender}` 为 key | WS: 每个连接独立 `Vec`，连接断开即清空 |
| 会话持久化（JSONL） | `sessions/weixin_{sender}.jsonl` | WS 连接内持久，无 JSONL（天然隔离） |
| Memory（SQLite） | session_id = `weixin_{sender}` | session_id = `ws_{conn_id}` |
| Core Memory（无 session） | 共享 | 共享（风险点） |
| Workspace 文件 | 共享 `/zeroclaw-data/workspace/` | 共享（风险点） |

### 4.2 方案 A：单容器 + WebSocket session_id 隔离（推荐）

```
┌──────────────────────────────────────────────────────┐
│                单个 MaxClaw 容器                       │
│                                                      │
│  ┌─────────────┐       ┌──────────────┐              │
│  │ WeixinChannel│       │ WS /ws/chat  │◄── 鸿蒙 APP │
│  │ (Channel)   │       │ (WebSocket)  │   WS 连接    │
│  │             │       │              │              │
│  │ session:    │       │ session:     │              │
│  │ weixin_*    │       │ ws_{conn_id} │              │
│  └──────┬──────┘       └──────┬───────┘              │
│         │                     │                      │
│         ▼                     ▼                      │
│  ┌────────────────────────────────────────────┐      │
│  │  run_tool_call_loop (共享生产级 loop)       │      │
│  └────────────────────┬───────────────────────┘      │
│                       │                              │
│                       ▼                              │
│  ┌────────────────────────────────────────────┐      │
│  │  Memory (SQLite)                           │      │
│  │                                            │      │
│  │  session_id = "weixin_sender_abc"          │      │
│  │  session_id = "ws_a1b2c3d4"               │      │
│  │  session_id = NULL (Core Memory) ← 共享    │      │
│  └────────────────────────────────────────────┘      │
└──────────────────────────────────────────────────────┘
```

**隔离效果**：

| 维度 | 是否隔离 | 说明 |
|------|---------|------|
| 会话历史 | 完全隔离 | WS 连接内 `Vec` vs Channel `conversation_histories` HashMap，完全独立的数据结构 |
| 会话持久化 | 完全隔离 | WS 无 JSONL 文件，Channel 有各自的 JSONL 文件 |
| Conversation Memory | 完全隔离 | session_id 前缀不同（`ws_*` vs `weixin_*`），recall/list 自动过滤 |
| Core Memory | 未隔离 | 无 session_id 的 Memory 全局可见 |
| Workspace 文件 | 未隔离 | 工具操作同一文件系统 |
| LLM 配额 | 共享 | 评测消耗影响日常使用额度 |

**优势**：
- WS 与微信走完全一致的生产级 `run_tool_call_loop` 路径
- 单容器运维最简单
- WS 连接级隔离天然比 Channel 更强（连接断开 = 会话彻底消失）
- session_id 前缀 `ws_` 与 `weixin_` 天然隔离 Memory

**风险与缓解**：

| 风险 | 缓解策略 |
|------|---------|
| Core Memory 污染 | 评测 case 设计避免触发 memory_store 写入无 session 的 Core Memory |
| Workspace 污染 | 评测文件操作限制在 `/workspace/evalspace/` 子目录 |
| LLM 配额竞争 | 评测任务串行执行，任务间加延迟 |

### 4.3 方案 B：双容器部署（最强隔离）

```
┌───────────────────────────┐  ┌───────────────────────────┐
│  容器 1：生产 (日常微信)     │  │  容器 2：评测专用           │
│                           │  │                           │
│  端口: 42617              │  │  端口: 42618              │
│  Volume: zeroclaw-prod    │  │  Volume: zeroclaw-eval    │
│                           │  │                           │
│  Channel: weixin          │  │  WS: /ws/chat             │
│  Memory: prod.sqlite      │  │  Memory: eval.sqlite      │
│  Sessions: prod/sessions/ │  │  Workspace: eval/workspace│
│  Workspace: prod/workspace│  │                           │
│                           │  │                           │
│  ◄── 微信 iLink           │  │  ◄── 鸿蒙 APP (WS 连接)  │
└───────────────────────────┘  └───────────────────────────┘
```

**docker-compose 示例**：

```yaml
services:
  zeroclaw-prod:
    build: .
    container_name: zeroclaw-prod
    ports:
      - "42617:42617"
    environment:
      - API_KEY=${API_KEY}
      - PROVIDER=${PROVIDER:-openrouter}
      - ZEROCLAW_MODEL=${ZEROCLAW_MODEL:-anthropic/claude-sonnet-4}
      - ZEROCLAW_ALLOW_PUBLIC_BIND=true
    volumes:
      - zeroclaw-prod-data:/zeroclaw-data

  zeroclaw-eval:
    build: .
    container_name: zeroclaw-eval
    ports:
      - "42618:42617"
    environment:
      - API_KEY=${API_KEY}
      - PROVIDER=${PROVIDER:-openrouter}
      - ZEROCLAW_MODEL=${ZEROCLAW_MODEL:-anthropic/claude-sonnet-4}
      - ZEROCLAW_ALLOW_PUBLIC_BIND=true
      - ZEROCLAW_REASONING_ENABLED=${ZEROCLAW_REASONING_ENABLED:-false}
    volumes:
      - zeroclaw-eval-data:/zeroclaw-data

volumes:
  zeroclaw-prod-data:
  zeroclaw-eval-data:
```

**隔离效果**：

| 维度 | 是否隔离 | 说明 |
|------|---------|------|
| 会话历史 | 完全隔离 | 独立进程、独立内存 |
| Conversation Memory | 完全隔离 | 独立 SQLite 文件 |
| Core Memory | 完全隔离 | 独立 SQLite 文件 |
| Workspace 文件 | 完全隔离 | 独立 Docker Volume |
| LLM 配额 | 共享 API Key | 同一 Provider 账号，需注意速率限制 |

**优势**：
- 完全数据隔离，零污染风险
- 可独立配置（不同模型、不同工具集、不同安全策略）
- 评测容器可随时销毁重建，不影响生产
- 可针对评测场景调整 reasoning_enabled、max_tool_iterations 等参数

**劣势**：
- 额外资源消耗（MaxClaw 本身 &lt;5MB RAM，影响微小）
- 需维护两套配置
- 共享 API Key 可能遇到 Provider 速率限制

---

## 5. 推荐方案与决策矩阵

| 考量因素 | 方案 A（单容器） | 方案 B（双容器） |
|---------|-----------------|----------------|
| 代码路径 | 生产级 run_tool_call_loop | 生产级 run_tool_call_loop |
| 结构化事件流 | 有（AgentEvent JSON 帧） | 有（同方案 A） |
| 运维复杂度 | 低 | 中 |
| 隔离强度 | 中高（会话/Memory 完全隔离，Core/Workspace 有风险） | 强（完全独立） |
| 资源消耗 | 最低 | 低（+5MB） |
| 新增配置 | 无 | docker-compose 新增 service |
| Core Memory 安全 | 有风险（可控） | 安全 |
| Workspace 安全 | 有风险（evalspace 子目录缓解） | 安全 |
| APP 连接复杂度 | 低（单个 WS 连接即可） | 低（同左，目标端口不同） |
| 适用阶段 | MVP → 长期 | 长期生产运行 / 多配置评测 |

### 5.1 最终推荐

**阶段一（MVP）：方案 A — 单容器 + WebSocket**

理由：
- WS 与微信走完全一致的生产级 `run_tool_call_loop` 路径，评测结果可信
- WebSocket 提供结构化事件流（reasoning / tool_call / tool_result / chunk / done），APP 可展示完整 loop 内容
- WS 连接级隔离天然比 Channel 更强：session_id `ws_*` 与 `weixin_*` 天然隔离
- 鸿蒙 APP 只需建立一个 WS 连接，无需管理回调服务
- 无需新增任何模块或配置字段

**阶段二（成熟期）：方案 B — 双容器**

当以下任一条件成立时升级：
- 评测 case 规模大，批量执行时 LLM API 速率受影响
- 发现 Core Memory 污染了微信日常使用体验
- 需要评测不同模型/配置组合（每个组合需要独立容器）

---

## 6. WebSocket 技术细节

### 6.1 关键源码位置

| 文件 | 职责 |
|------|------|
| `src/gateway/ws.rs` | WS handler：认证、Agent 创建、消息收发、AgentEvent 转发 |
| `src/agent/agent.rs` | `Agent::turn()`：外层编排（system prompt / memory / classify / cache）→ 委托 `run_tool_call_loop()` |
| `src/agent/loop_.rs` | `run_tool_call_loop()`：核心引擎（工具执行 / approval / hooks / observability） |

### 6.2 AgentEvent 结构化事件格式

`AgentEvent` 枚举（`src/agent/agent.rs`）通过 WS JSON 帧推送：

```rust
pub enum AgentEvent {
    Reasoning(String),
    Chunk(String),
    ToolCallStart { name: String, arguments: serde_json::Value },
    ToolCallComplete { name: String, output: String, success: bool, duration_ms: u64 },
    Done { text: String, reasoning_content: Option<String> },
    Error { message: String },
}
```

**WS 帧类型映射**：

| AgentEvent 变体 | WS JSON `type` 字段 | 说明 |
|-----------------|---------------------|------|
| `Reasoning(content)` | `"reasoning"` | LLM 推理过程 |
| `ToolCallStart { name, arguments }` | `"tool_call"` | 工具调用请求 |
| `ToolCallComplete { name, output, success, duration_ms }` | `"tool_result"` | 工具执行结果 |
| `Chunk(content)` | `"chunk"` | 流式文本增量 |
| `Done { text, reasoning_content }` | `"done"` | 最终完整回复 |
| `Error { message }` | `"error"` | 错误信息 |

**鸿蒙 APP 收到的 WS 帧序列示例**：

```json
{"type":"reasoning","content":"用户要求查看文件列表，我需要使用 shell 工具..."}
{"type":"tool_call","name":"shell","args":{"command":"ls -la"}}
{"type":"tool_result","name":"shell","output":"total 42\ndrwxr-xr-x ...","success":true,"duration_ms":120}
{"type":"chunk","content":"当前目录"}
{"type":"chunk","content":"下有以下"}
{"type":"chunk","content":"文件：\n"}
{"type":"done","full_response":"当前目录下有以下文件：\n- README.md\n- src/\n- ..."}
```

### 6.3 session_id 隔离机制

`ws.rs` 中 session_id 设置逻辑：

```rust
let session_id = format!("ws_{}", connection_id);
agent.set_memory_session_id(Some(&session_id));
// → Memory 操作自动按 "ws_{conn_id}" 过滤
// → 与 Channel 的 "weixin_{sender}" 天然隔离
```

---

## 7. 鸿蒙 APP 关键技术选型

### 7.1 网络层

| 需求 | HarmonyOS 技术方案 |
|------|-------------------|
| WebSocket 连接 | `@ohos.net.webSocket` API |
| JSON 帧解析 | ArkTS 内置 `JSON.parse/stringify` |
| HTTP REST 调用 | `@ohos.net.http` API（辅助接口：health/status/memory） |
| 认证 Token 管理 | `@ohos.data.preferences` 本地存储 |
| 自动重连 | 自行实现（指数退避 + 心跳检测） |

### 7.2 本地持久化

| 需求 | 技术方案 |
|------|---------|
| MaxClaw 连接配置（IP/Port/Token） | `@ohos.data.preferences` |
| 用户偏好（主题/字体） | `@ohos.data.preferences` |

APP 不设本地数据库。所有业务数据（会话历史、Memory、配置、评测结果）均存储在 MaxClaw 端，APP 通过 API 按需查看。

### 7.3 核心模块

1. **WebSocket 客户端**：建立/维持 WS 连接，发送消息，接收 JSON 帧
2. **AgentEvent 解析器**：解析 WS 帧中的 reasoning / tool_call / tool_result / chunk / done 事件
3. **实时展示引擎**：将结构化事件流实时渲染到对话界面（reasoning 折叠、tool call 高亮、chunk 逐字显示）
4. **REST API 客户端**：封装各管理接口的请求/响应，供系统管理菜单使用

---

## 8. 风险与注意事项

### 8.1 鸿蒙 APP 开发风险

| 风险 | 缓解策略 |
|------|---------|
| WS 连接不稳定 / 断线 | 实现自动重连（指数退避）+ 心跳检测（ping/pong） |
| AgentEvent 格式变更 | APP 侧做容错解析，未知事件类型静默忽略 |

### 8.2 集成风险

| 风险 | 缓解策略 |
|------|---------|
| MaxClaw 容器重启丢失 WS 连接和会话 | APP 端重连后自动创建新会话 |
| 评测 case 触发危险工具调用 | 配置 `[autonomy] non_cli_excluded_tools` 限制可用工具 |

---

## 9. 总结

| 问题 | 结论 |
|------|------|
| APP 定位 | 薄客户端——对话交互 + 系统管理入口；评测逻辑在 MaxClaw 端完成，结果以文字返回 |
| 最佳集成方式 | WebSocket（`/ws/chat`）：生产级 `run_tool_call_loop` 核心引擎 + 结构化 AgentEvent 事件流 |
| 系统管理 | 通过 REST API 查看/修改 MaxClaw 状态、配置、Memory、工具、费用等，APP 仅做前端展示 |
| 为什么不新增 Channel | Channel 的 `send()` 接口只能返回最终文本，无法推送 reasoning / tool_call 等中间步骤的结构化事件 |
| 是否需要双容器 | MVP 阶段不需要，单容器 + WS 的 `ws_{conn_id}` session_id 隔离即可 |
| 如何避免评测污染微信 | WS session_id `ws_*` 与 Channel session_id `weixin_*` 天然隔离；WS 会话连接断开即清空 |
| APP 如何展示完整 loop | 通过 WS JSON 帧接收 AgentEvent（reasoning / tool_call / tool_result / chunk / done），实时渲染 |
| 网络连接 | 手机与电脑同局域网，APP 连接 `ws://{电脑 IP}:42617/ws/chat` |
| APP 本地存储 | 仅存连接配置和用户偏好（Preferences），无本地数据库 |
| 何时升级到双容器 | Core Memory 污染成问题 / 需测试多配置组合 / 评测规模大 |
