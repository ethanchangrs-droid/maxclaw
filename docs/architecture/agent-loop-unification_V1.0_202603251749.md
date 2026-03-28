# Agent Loop 统一与 History 类型统一分析

| 项目 | 内容 |
|------|------|
| 版本 | V1.0 |
| 日期 | 2026-03-25 17:49 (UTC+8) |
| 范围 | `Agent::turn()` vs `run_tool_call_loop()` 统一方案 |
| 前置依赖 | MaxClaw-Channel与Gateway架构及数据传输路径分析_V1.0 |

---

## 1. 问题陈述

MaxClaw 当前存在**两套独立的 Agent tool loop 实现**，功能重叠但能力不一致：

| | Loop A：`Agent::turn()` | Loop B：`run_tool_call_loop()` |
|---|---|---|
| 代码位置 | `src/agent/agent.rs` L548-789 | `src/agent/loop_.rs` L2342-3001 |
| 代码行数 | ~242 行 | ~660 行 |
| 调用方 | WebSocket handler (`ws.rs`) | Channel dispatch (`channels/mod.rs`)、Gateway webhook handlers、CLI |
| History 类型 | `Vec<ConversationMessage>` | `Vec<ChatMessage>` |

两套 loop 导致的核心问题：
1. **同一功能的重复实现**——修复一处，另一处不受益
2. **评测无法代表生产**——评测 APP 走 WS（turn），微信走 Channel（loop），代码路径不同
3. **维护成本翻倍**——每次新增特性需同步到两处

---

## 2. 两套 Loop 能力对比

### 2.1 安全与防护

| 特性 | `turn()` | `run_tool_call_loop()` |
|------|----------|------------------------|
| Approval（审批） | 无 | 有 |
| Tool call 去重 | 无 | 有（`seen_tool_signatures` + `dedup_exempt_tools`） |
| Cancellation（取消） | 无 | 有（`CancellationToken`） |
| Credential scrubbing | 无 | 有（tool args/output 日志脱敏） |
| Timeout budget | 无 | 有（外层 `tokio::time::timeout` 包裹） |

### 2.2 可观测性

| 特性 | `turn()` | `run_tool_call_loop()` |
|------|----------|------------------------|
| ObserverEvent::LlmRequest | 无 | 有 |
| ObserverEvent::LlmResponse（含 token 统计） | 无 | 有 |
| runtime_trace 事件种类 | 2/7（`llm_response`, `turn_final_response`） | 7/7（完整） |

### 2.3 工具执行

| 特性 | `turn()` | `run_tool_call_loop()` |
|------|----------|------------------------|
| Hooks（before/after tool call, llm_input） | 无 | 有 |
| Deferred tool activation（MCP 动态工具） | 无 | 有 |
| Multimodal（图片/音频） | 无 | 有 |
| 并行执行策略 | `config.parallel_tools` 开关 | 按 approval 模式智能决定 |
| **success 字段 bug** | 有（永远返回 true） | 无 |
| Tool call 解析失败检测 | 无 | 有（`detect_tool_call_parse_issue`） |

### 2.4 客户端事件流

| 特性 | `turn()` | `run_tool_call_loop()` |
|------|----------|------------------------|
| AgentEvent 推送（6 种结构化事件） | 有 | **无** |
| on_delta（纯文本进度） | 无 | 有 |

### 2.5 turn() 独有特性

| 特性 | 说明 |
|------|------|
| Response cache | 对相同 prompt 缓存响应 |
| Model classification | query routing（hint 分类） |
| 有状态多轮对话 | persistent `&mut self`，连接内 history 自动累积 |

---

## 3. 统一方案评估

### 3.1 方案 A：基于 `run_tool_call_loop()` 改造（推荐）

**做法**：给 `run_tool_call_loop()` 新增 `event_sender: Option<Sender<AgentEvent>>` 参数，让 `turn()` 删除自己的 loop 体（L592-784），改为调用 `run_tool_call_loop()`。

**改动清单**：

| 文件 | 改动 | 工作量 | 风险 |
|------|------|--------|------|
| `src/agent/loop_.rs` | `run_tool_call_loop()` 加 `event_sender` 参数 + 5-6 处事件发送 | 小 | 低 |
| `src/channels/mod.rs` | 调用处补 `None`（event_sender 位置） | 小 | 极低 |
| `src/agent/agent.rs` | `turn()` 删除 loop 体，改为委托调用；history 类型改为 `Vec&lt;ChatMessage>` | 中 | 中 |
| `src/agent/dispatcher.rs` | 删除（ToolDispatcher 抽象不再需要） | 小 | 低 |
| 测试 | `turn()` 的 ~20 个测试需回归验证 | 中 | 中 |

**优势**：
- `run_tool_call_loop()` 已被所有 Channel（20+ 平台）、CLI、webhook 验证，是经过生产检验的核心引擎
- 改动方向是"给已有基础设施加能力"（+event_sender），而非"搬运能力再删原处"
- 改错了最多影响 WS 一个调用点，不波及 Channel 全线

**event_sender 注入位置**：

```
run_tool_call_loop() 中需要发送 AgentEvent 的位置：
├─ LLM 响应后的 reasoning_content → AgentEvent::Reasoning
├─ 工具执行前 → AgentEvent::ToolCallStart
├─ 工具执行后 → AgentEvent::ToolCallComplete
├─ LLM 返回非工具文本时 → AgentEvent::Chunk
├─ 最终回复 → AgentEvent::Done
└─ 错误 → AgentEvent::Error
```

### 3.2 方案 B：基于 `Agent::turn()` 改造（不推荐）

**做法**：把 `run_tool_call_loop()` 的生产特性搬进 `turn()`，让 Channel dispatch 和 webhook 也调用 `Agent::turn()`。

**致命问题**：
1. `Agent` 是 `&mut self` 有状态设计，Channel 需要共享 provider + per-sender history + 并发——不兼容
2. Channel dispatch 需全面重写（构造 Agent 实例池、session 同步到 JSONL）
3. `process_message()` 的 webhook 场景需要每次 HTTP 请求构造完整 Agent 实例（20+ 字段）——比调函数重得多
4. 受影响测试 ~45 个，回滚极难

---

## 4. History 类型统一

### 4.1 现状

| 类型 | 使用者 | 特点 |
|------|--------|------|
| `ConversationMessage` | 仅 `turn()` 及其 `ToolDispatcher` 生态（5 个文件） | 枚举类型，含 `AssistantToolCalls` 结构化变体 |
| `ChatMessage` | 整个 Channel 子系统、hooks、session store、multimodal、loop_.rs、Provider 层 | 扁平 struct `{role, content}` |

### 4.2 结论：统一到 `ChatMessage`

`ConversationMessage` 存在的唯一理由是让 `NativeToolDispatcher.to_provider_messages()` 能把 `AssistantToolCalls` 变体还原为 Provider 要求的 native tool call format。但 `run_tool_call_loop()` 已经用 `build_native_assistant_history()` 在 `ChatMessage` 层面做了完全相同的事。

| 功能 | ConversationMessage 路径 | ChatMessage 路径 |
|------|--------------------------|------------------|
| 保存 assistant tool calls | `AssistantToolCalls { tool_calls, ... }` | `ChatMessage::assistant(build_native_assistant_history(...))` |
| 转为 Provider 格式 | `NativeToolDispatcher.to_provider_messages()` | Provider 的 `convert_messages()` 解析 JSON |
| 生产验证度 | 仅 WS 路径 | 所有 Channel + CLI + webhook |

统一后可删除的代码：
- `ConversationMessage` 枚举定义
- `ToolDispatcher` trait 及 `NativeToolDispatcher`、`XmlToolDispatcher` 实现
- `src/agent/dispatcher.rs` 文件
- 预计净减少 ~300 行

---

## 5. 统一后的架构

```
WS handler (ws.rs)           Channel dispatch              Gateway webhook
      │                       (channels/mod.rs)             (gateway/mod.rs)
      ▼                            │                            │
   turn()                  process_channel_message()     process_message()
      │                            │                            │
      │ 外层编排：                  │ 外层编排：                  │ 外层编排：
      │ - system prompt            │ - typing indicator          │ - 从 Config 创建
      │ - memory context           │ - allowlist                 │   全部组件
      │ - model classify           │ - history load (JSONL)      │
      │ - response cache           │ - model routing             │
      │ - history manage           │ - draft updates             │
      │                            │                            │
      └────────────┬───────────────┘───────────────┘
                   │
                   ▼
          run_tool_call_loop()         ← 唯一核心引擎
          ├─ event_sender（新增）       → WS 路径传入，推送 AgentEvent
          ├─ on_delta（已有）           → Channel 路径传入，推送文本进度
          ├─ approval / dedup / hooks  → 生产级安全防护
          ├─ credential scrubbing      → 日志脱敏
          ├─ cancellation              → 任务取消
          └─ 完整 observability        → LlmRequest/Response/ToolCall + runtime_trace
```

### 5.1 三条路径的外层编排不可互换

| | 为什么不能用 Channel dispatch 替代 turn() | 为什么不能用 turn() 替代 Channel dispatch |
|---|---|---|
| 原因 | Channel 的 `send()` 只返回最终文本，无 AgentEvent 流；会话模型不兼容；需伪造 Channel 实例 + 构造 30+ 字段的 ChannelRuntimeContext | Agent 是 `&mut self` 有状态设计，与 Channel 的共享 provider + per-sender history + 并发模型冲突 |

每条路径的外层编排是该场景专有的，不可替代。但底层共用同一个 `run_tool_call_loop()`，确保核心逻辑一致。

---

## 6. 实施计划

### Phase 1：给 run_tool_call_loop 加 event_sender（低风险）

1. `run_tool_call_loop()` 新增 `event_sender: Option<Sender<AgentEvent>>` 参数
2. 在 loop 内 6 个位置加入 `if let Some(ref tx) = event_sender { ... }` 发送事件
3. 所有现有调用点补 `None`
4. 验证：全部现有测试通过（行为无变化）

### Phase 2：turn() 委托到 run_tool_call_loop（中风险）

1. `turn()` 的 history 类型从 `Vec<ConversationMessage>` 改为 `Vec<ChatMessage>`
2. 删除 turn() 的 loop 体（L592-784），替换为对 `run_tool_call_loop()` 的调用
3. 保留 turn() 的外层编排逻辑（system prompt、memory、cache、classify）
4. 传入 `self.event_sender` 作为 event_sender 参数
5. 验证：turn() 的 ~20 个测试全部通过

### Phase 3：清理废弃代码（低风险）

1. 删除 `ConversationMessage` 枚举
2. 删除 `ToolDispatcher` trait 及实现
3. 删除 `src/agent/dispatcher.rs`
4. 清理 `src/agent/tests.rs` 中对 dispatcher 的引用

---

## 7. 风险评估

| 风险 | 缓解策略 |
|------|---------|
| Phase 2 改动 turn() 可能影响 WS 连接的多轮对话 | turn() 的 ~20 个测试覆盖多轮、history trim、tool call 等场景 |
| event_sender 在 loop 中的发送时机不精确 | 参照 turn() 现有的 AgentEvent 发送位置，一一对应 |
| ChatMessage history 丢失 AssistantToolCalls 结构信息 | run_tool_call_loop 的 `build_native_assistant_history` 已用 JSON 编码保留了等价信息 |
| Phase 3 删代码可能遗漏引用 | 编译器会报错，Rust 的类型系统保证不会遗漏 |

---

*文档结束*
