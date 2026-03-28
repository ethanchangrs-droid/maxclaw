# 外层编排路径分析

| 项目 | 内容 |
|------|------|
| 版本 | V1.0 |
| 日期 | 2026-03-25 18:04 (UTC+8) |
| 范围 | 5 条外层编排路径的职责对比、差异分析、统一可行性评估 |
| 前置依赖 | agent-loop-unification_V1.0 |

---

## 1. 路径一览

| # | 入口 | 编排代码位置 | 核心 loop 入口 |
|---|------|-------------|---------------|
| 1 | WebSocket `/ws/chat` | `ws.rs` → `Agent::from_config()` + `agent.turn()` | `turn()` 自有 loop 体 |
| 2 | Channel dispatch（20+ 平台） | `channels/mod.rs` → `start_channels()` + `process_channel_message()` | `run_tool_call_loop()` |
| 3 | CLI（`zeroclaw run`） | `loop_.rs::run()` | `run_tool_call_loop()` |
| 4 | Gateway webhook（WhatsApp/Linq/WATI/Nextcloud Talk） | `gateway/mod.rs` → `run_gateway_chat_with_tools()` → `process_message()` | `agent_turn()` → `run_tool_call_loop()` |
| 5 | Gateway simple chat（`/webhook` POST） | `gateway/mod.rs` → `run_gateway_chat()` | `provider.chat_with_history()` — 无 tool loop |

---

## 2. 编排职责拆解

### 2.1 阶段 A — 组件初始化

| 组件 | WS `from_config()` | Channel `start_channels()` | CLI `run()` | Gateway `process_message()` |
|------|--------------------|-----------------------------|-------------|----------------------------|
| Provider | `create_routed_provider` | `create_routed_provider_with_options` | `create_routed_provider_with_options` | `create_routed_provider_with_options` |
| ProviderRuntimeOptions | 无 | 有（auth_profile, reasoning, timeout, extra_headers, api_path） | 有 | 有 |
| Memory | 有 | 有 | 有 | 有 |
| Observer | 有 | 有 | 有 | 有 |
| Security | 有 | 有 | 有 | 有 |
| Tools 基础 | `all_tools_with_runtime` | `all_tools_with_runtime` | `all_tools_with_runtime` | `all_tools_with_runtime` |
| MCP 工具 | 无 | 有（eager） | 有（eager/deferred） | 有 |
| Peripheral 工具 | 无 | 有 | 有 | 有 |
| 工具过滤 | 无 | `non_cli_excluded_tools` | `allowed_tools` CLI 参数 | 无 |
| 组件生命周期 | 连接级（persistent Agent） | 进程级（Arc 共享） | 进程级 | 请求级（每次重建） |

### 2.2 阶段 B — 每轮消息准备

| 步骤 | WS `turn()` | Channel `process_channel_message` | CLI `run()` | Gateway `process_message` |
|------|-------------|-----------------------------------|-------------|--------------------------|
| System prompt | `SystemPromptBuilder`（内部） | `build_channel_system_prompt` | `build_system_prompt_with_mode` | `build_system_prompt_with_mode` |
| Memory 自动存储 | `memory.store("user_msg")` — 无长度门槛 | `memory.store(autosave_key)` — 有门槛 | `mem.store(user_key)` — 有门槛 | `mem.store()` — 有门槛 |
| Memory 上下文加载 | `memory_loader.load_context()` 每轮 | `build_memory_context()` 仅首轮 | `build_context()` + HW RAG | `build_context()` + HW RAG |
| 时间戳注入 | `[timestamp] msg` | 无 | `[timestamp] msg` | `[timestamp] msg` |
| Query 分类/路由 | `classify_model()` | `classifier::classify()` | 无（CLI 参数） | 无 |
| Response cache | 有 | 无 | 无 | 无 |

### 2.3 阶段 B' — 传输层特有步骤

| 步骤 | WS | Channel | CLI | Gateway webhook |
|------|-----|---------|-----|-----------------|
| AgentEvent 事件流 | 有 | 无 | 无 | 无 |
| Hooks（message_received/sending） | 无 | 有 | 无 | 无 |
| Typing indicator | 无 | 有 | 无 | 无 |
| Draft update（流式更新消息） | 无 | 有 | 无 | 无 |
| Ack reaction（表情确认） | 无 | 有 | 无 | 无 |
| Tool call thread notify | 无 | 有 | 无 | 无 |
| Timeout budget | 无 | 有 | 无 | 无 |
| CancellationToken | 无 | 有 | 无 | 无 |
| IMAGE marker 清理 | 无 | 有 | 无 | 无 |

### 2.4 阶段 C — 后处理

| 步骤 | WS `turn()` | Channel | CLI `run()` | Gateway webhook |
|------|-------------|---------|-------------|-----------------|
| History 持久化 | 内存（连接生命周期） | HashMap + JSONL 文件 | session state 文件 | 无（无状态） |
| History 压缩 | `trim_history()` 简单截断 | `proactive_trim_turns` 预算制 | `auto_compact_history` + `trim_history` | 无 |
| 回复发送 | AgentEvent::Done → WS frame | `channel.send()` → 平台 API | CLI stdout | HTTP response body |

---

## 3. WS 路径（`Agent::from_config`）的能力缺失

| 缺失项 | 代码位置 | 影响 | 修复方式 |
|--------|---------|------|---------|
| 无 MCP 工具 | `agent.rs` L310-353 中 `from_config` 未调用 `McpRegistry::connect_all` | WS 用户无法使用 MCP 集成的外部工具 | 在 `from_config` 中增加 MCP 初始化逻辑 |
| 无 Peripheral 工具 | `agent.rs` L339 中 `all_tools_with_runtime` 后未追加 peripheral_tools | WS 用户无法控制硬件外设 | 在 `from_config` 中追加 `create_peripheral_tools` |
| 无 `ProviderRuntimeOptions` | `agent.rs` L363 调用 `create_routed_provider` 而非 `create_routed_provider_with_options` | 缺少 reasoning_enabled、provider_timeout、extra_headers 等配置 | 改用 `create_routed_provider_with_options` |
| Memory 自动存储无长度门槛 | `agent.rs` L557-567 无条件存储 | 短消息（"hi"）也存入 memory，造成噪音 | 加入 `AUTOSAVE_MIN_MESSAGE_CHARS` 和 `should_skip_autosave_content` 检查 |
| `success` 永远为 `true` | `agent.rs` L500-503 | 工具执行失败时不正确标记，下游分析失准 | 在 F003 loop 统一后自动解决 |

---

## 4. 统一评估

### 4.1 不宜统一的部分（传输层固有差异）

以下步骤是传输层本质决定的，不应强行统一：

- **WS**：AgentEvent 推送、response cache — 实时双向长连接特有
- **Channel**：typing indicator、draft update、ack reaction、hooks、thread notify — 即时通讯 UX 特有
- **CLI**：interactive session、Hardware RAG、approval manager — 本地交互特有
- **Gateway webhook**：无状态 per-request 初始化 — HTTP 请求-响应模型特有

### 4.2 可提取为共享模块的部分

以下步骤在 3-4 条路径中存在逻辑相同或高度相似的重复实现：

| 共有步骤 | 当前重复点 | 建议抽象 |
|---------|-----------|---------|
| 组件初始化（Provider + Memory + Observer + Security + Tools + MCP + Peripheral） | `Agent::from_config`、`start_channels`、`run()`、`process_message()` — 4 处近似代码 | `ComponentFactory::from_config(config) -> ComponentSet` |
| System prompt 构建 | `SystemPromptBuilder`、`build_system_prompt_with_mode`、`build_channel_system_prompt` — 3 种方式 | `SystemPromptFactory::build(mode, workspace, model, tools, skills, identity)` |
| Memory 自动存储 | 4 处重复的 `memory.store()` + 门槛判断 | `auto_save_user_message(memory, msg, session_id)` |
| Memory 上下文加载 + 消息增强 | `memory_loader.load_context`、`build_memory_context`、`build_context` — 3 种 API | `enrich_message(memory, msg, session_id) -> String` |
| History trim/compact | `trim_history`、`proactive_trim_turns`、`auto_compact_history` — 3 种策略 | `HistoryManager::trim(history, strategy)` |

### 4.3 提取后的架构

```
各路径外层编排
    │
    ├── WS handler
    │   ├── ComponentFactory::from_config()  ← 共享
    │   ├── enrich_message()                 ← 共享
    │   ├── auto_save_user_message()         ← 共享
    │   ├── AgentEvent streaming             ← WS 特有
    │   ├── response cache                   ← WS 特有
    │   └── run_tool_call_loop()             ← 统一核心引擎
    │
    ├── Channel dispatch
    │   ├── ComponentFactory (进程级共享)     ← 共享
    │   ├── enrich_message()                 ← 共享
    │   ├── auto_save_user_message()         ← 共享
    │   ├── typing / draft / hooks / ack     ← Channel 特有
    │   ├── timeout + cancellation           ← Channel 特有
    │   └── run_tool_call_loop()             ← 统一核心引擎
    │
    ├── CLI run()
    │   ├── ComponentFactory::from_config()  ← 共享
    │   ├── enrich_message()                 ← 共享
    │   ├── auto_save_user_message()         ← 共享
    │   ├── interactive session / HW RAG     ← CLI 特有
    │   └── run_tool_call_loop()             ← 统一核心引擎
    │
    └── Gateway webhook
        ├── ComponentFactory::from_config()  ← 共享
        ├── enrich_message()                 ← 共享
        ├── auto_save_user_message()         ← 共享
        └── run_tool_call_loop()             ← 统一核心引擎
```

---

## 5. 实施优先级

| 优先级 | 任务 | 关联 |
|--------|------|------|
| P0 | WS 路径能力补齐（MCP / Peripheral / ProviderRuntimeOptions / auto_save 门槛） | F004 |
| P0 | 核心 Loop 统一（turn → run_tool_call_loop 委托） | F003 |
| P3 | 提取共有编排步骤为可复用模块（ComponentFactory / enrich_message / auto_save / HistoryManager） | F005 |

---

*文档结束*
