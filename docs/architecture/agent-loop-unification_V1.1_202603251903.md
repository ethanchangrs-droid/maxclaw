# Agent Loop 统一与 History 类型统一分析

| 项目 | 内容 |
|------|------|
| 版本 | V1.1 |
| 日期 | 2026-03-25 19:03 (UTC+8) |
| 范围 | `Agent::turn()` vs `run_tool_call_loop()` 统一方案 |
| 前置依赖 | MaxClaw-Channel与Gateway架构及数据传输路径分析_V1.0 |

V1.1 变更：补充 response cache 处理策略（LoopOutcome 返回值）、Agent struct/Builder/Config 清理项、trim_history 迁移、success 字段行为变更风险、测试重写（非仅回归验证）、event_sender/on_delta 互斥规则。

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

**做法**：给 `run_tool_call_loop()` 新增 `event_sender: Option<Sender<AgentEvent>>` 参数并改返回值为 `LoopOutcome`，让 `turn()` 删除自己的 loop 体（L592-784），改为调用 `run_tool_call_loop()`。

**改动清单**：

| 文件 | 改动 | 工作量 | 风险 |
|------|------|--------|------|
| `src/agent/loop_.rs` | `run_tool_call_loop()` 加 `event_sender` 参数 + 6 处事件发送；返回值改为 `LoopOutcome` | 小 | 低 |
| `src/channels/mod.rs` | 调用处补 `None`（event_sender 位置）；适配 `LoopOutcome` 返回值 | 小 | 低 |
| `src/agent/agent.rs` | `turn()` 删除 loop 体，改为委托调用；history 类型改为 `Vec&lt;ChatMessage>`；`trim_history()` 改为基于 `ChatMessage`；response cache 逻辑移至 loop 调用前后；删除 `tool_dispatcher` 字段；`build_system_prompt()` 改用 `provider.supports_native_tools()` 判断工具指令；`history()` 返回 `&[ChatMessage]` | 大 | 中 |
| `src/agent/dispatcher.rs` | 整文件删除（`ToolDispatcher` trait + `NativeToolDispatcher` + `XmlToolDispatcher`） | 小 | 低 |
| `src/providers/traits.rs` | 删除 `ConversationMessage` 枚举 | 小 | 低 |
| `src/config/schema.rs` | 删除或标记废弃 `agent.tool_dispatcher` 配置字段 | 小 | 低 |
| `src/agent/tests.rs` | ~20 个测试需**重写**（非仅回归验证）：`ConversationMessage` 模式匹配改为 `ChatMessage`，序列化往返测试删除 | 中 | 中 |

**优势**：
- `run_tool_call_loop()` 已被所有 Channel（20+ 平台）、CLI、webhook 验证，是经过生产检验的核心引擎
- 改动方向是"给已有基础设施加能力"（+event_sender），而非"搬运能力再删原处"
- 改错了最多影响 WS 一个调用点，不波及 Channel 全线

**返回值改造 — LoopOutcome**：

当前 `run_tool_call_loop()` 返回 `Result<String>`，无法区分"首轮直接文本"与"多轮工具后的最终文本"。改为返回结构体，让调用方获得更丰富的上下文：

```rust
pub(crate) struct LoopOutcome {
    pub text: String,
    pub iterations_used: usize,
}
```

`iterations_used == 1` 表示 LLM 首轮即返回文本（无工具调用），turn() 据此决定是否缓存 response。Channel 路径和 webhook 路径忽略此字段，仅取 `.text`。

**Response cache 处理策略**：

当前 cache 逻辑嵌在 turn() 的 loop 体内部（每次迭代开头检查 cache，无工具调用时存入 cache）。分析发现，cache 仅在首轮迭代有意义——若首轮命中，直接返回；若首轮触发了工具调用，后续迭代的消息已变化，命中概率接近零。

因此 cache 可留在 turn() 外层编排，不必注入 `run_tool_call_loop()`：

```
turn() 外层：
  1. 构建 messages（system prompt + memory context + user message）
  2. 检查 response cache → 命中则直接返回，不进入 loop
  3. 调用 run_tool_call_loop() → 返回 LoopOutcome
  4. 如果 iterations_used == 1 → 存入 response cache
  5. trim_history()
  6. 返回 text
```

这样 `run_tool_call_loop()` 保持纯粹的工具循环职责，不感知 cache 概念。

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

**event_sender 与 on_delta 的互斥规则**：

两个 sender 服务不同消费者，调用时应互斥传入：

| 路径 | event_sender | on_delta |
|------|-------------|----------|
| WS（turn） | `Some(...)` | `None` |
| Channel dispatch | `None` | `Some(...)` |
| Gateway webhook | `None` | 视场景 |

若同时传入两者，同一触发点会产生双重推送（结构化事件 + 文本进度），客户端收到重复信息。

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
- `ConversationMessage` 枚举定义（`src/providers/traits.rs`）
- `ToolDispatcher` trait 及 `NativeToolDispatcher`、`XmlToolDispatcher` 实现
- `src/agent/dispatcher.rs` 整文件
- Agent struct 的 `tool_dispatcher: Box<dyn ToolDispatcher>` 字段及 Builder setter
- `Agent::from_config()` 中的 dispatcher 选择逻辑（`config.agent.tool_dispatcher`）
- `config.agent.tool_dispatcher` 配置字段（schema.rs）
- `src/agent/tests.rs` 中 `ConversationMessage` 序列化往返测试（Test 16）
- 预计净减少 ~350 行

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
      │ - response cache (外层)    │ - model routing             │
      │ - history trim (外层)      │ - draft updates             │
      │                            │                            │
      └────────────┬───────────────┘───────────────┘
                   │
                   ▼
          run_tool_call_loop()         ← 唯一核心引擎
          │                            ← 返回 LoopOutcome { text, iterations_used }
          ├─ event_sender（新增）       → WS 路径传入，推送 AgentEvent
          ├─ on_delta（已有）           → Channel 路径传入，推送文本进度
          ├─ approval / dedup / hooks  → 生产级安全防护
          ├─ credential scrubbing      → 日志脱敏
          ├─ cancellation              → 任务取消
          └─ 完整 observability        → LlmRequest/Response/ToolCall + runtime_trace
```

### 5.1 turn() 委托后的完整流程

```
turn(user_message):
  1. 首次调用 → build_system_prompt()
     └─ 用 provider.supports_native_tools() 决定是否注入 XML 工具指令
  2. 加载 memory context，拼接时间戳
  3. 推入 user message 到 self.history: Vec<ChatMessage>
  4. classify_model 选择有效模型

  5. [cache 检查] 构建 cache key → 命中则发 AgentEvent::Done 并直接返回

  6. 调用 run_tool_call_loop(
         provider, &mut self.history, tools, observer,
         model, temperature,
         event_sender = Some(...),     ← WS 专用
         on_delta = None,              ← WS 路径不传
         ...
     ) → LoopOutcome { text, iterations_used }

  7. [cache 存储] 如果 iterations_used == 1 → 存入 response cache
  8. trim_history()（基于 ChatMessage 的版本）
  9. 返回 text
```

### 5.2 三条路径的外层编排不可互换

| | 为什么不能用 Channel dispatch 替代 turn() | 为什么不能用 turn() 替代 Channel dispatch |
|---|---|---|
| 原因 | Channel 的 `send()` 只返回最终文本，无 AgentEvent 流；会话模型不兼容；需伪造 Channel 实例 + 构造 30+ 字段的 ChannelRuntimeContext | Agent 是 `&mut self` 有状态设计，与 Channel 的共享 provider + per-sender history + 并发模型冲突 |

每条路径的外层编排是该场景专有的，不可替代。但底层共用同一个 `run_tool_call_loop()`，确保核心逻辑一致。

### 5.3 职责边界

| 关注点 | 归属 | 原因 |
|--------|------|------|
| Response cache | turn() 外层 | 仅 WS 路径需要，且只在首轮迭代有意义 |
| History trim | turn() 外层 | Channel 路径不 trim（依赖 JSONL 外部管理），是 WS 有状态模式的专有需求 |
| 工具指令注入 system prompt | turn() 外层 | 用 `provider.supports_native_tools()` 判断，复用 `loop_::build_tool_instructions()` |
| Tool parsing（native + XML 等） | run_tool_call_loop() 内部 | 已涵盖 10+ 种格式，远超原 XmlToolDispatcher |
| AgentEvent 推送 | run_tool_call_loop() 内部 | 通过 `event_sender` 可选参数，不传则无操作 |

---

## 6. 实施计划

### Phase 1：给 run_tool_call_loop 加 event_sender + 改返回值（低风险）

1. 定义 `LoopOutcome` 结构体（`text: String`, `iterations_used: usize`）
2. `run_tool_call_loop()` 返回值从 `Result<String>` 改为 `Result<LoopOutcome>`
3. 新增 `event_sender: Option<Sender<AgentEvent>>` 参数
4. 在 loop 内 6 个位置加入 `if let Some(ref tx) = event_sender { ... }` 发送事件
5. 所有现有调用点：补 `event_sender = None`，适配 `LoopOutcome`（取 `.text`）
6. 验证：全部现有测试通过（行为无变化）

### Phase 2：turn() 委托到 run_tool_call_loop（中风险）

1. Agent struct 的 `history` 字段类型从 `Vec<ConversationMessage>` 改为 `Vec<ChatMessage>`
2. `trim_history()` 重写：`ConversationMessage` 模式匹配改为 `ChatMessage` 的 `role == "system"` 判断
3. `build_system_prompt()` 中的工具指令注入：删除 `self.tool_dispatcher.prompt_instructions()` 调用，改为按 `self.provider.supports_native_tools()` 判断——返回 false 时调用 `loop_::build_tool_instructions()`，返回 true 时注入空串
4. 删除 turn() 的 loop 体（L592-784），替换为：
   - cache 检查放在 `run_tool_call_loop()` 调用之前
   - 调用 `run_tool_call_loop()` 传入 `event_sender = Some(...)`, `on_delta = None`
   - 根据 `LoopOutcome.iterations_used == 1` 决定是否存入 response cache
   - 调用 `trim_history()`
5. 删除 Agent struct 的 `tool_dispatcher` 字段和 `AgentBuilder.tool_dispatcher()` setter
6. `pub fn history()` 返回类型改为 `&[ChatMessage]`
7. 验证：turn() 的 ~20 个测试需**重写后**通过（见 Phase 3）

### Phase 3：清理废弃代码 + 测试重写（低-中风险）

1. 删除 `ConversationMessage` 枚举（`src/providers/traits.rs`）
2. 删除 `ToolDispatcher` trait 及 `NativeToolDispatcher`、`XmlToolDispatcher` 实现
3. 删除 `src/agent/dispatcher.rs` 整文件
4. 删除 `Agent::from_config()` 中的 dispatcher 选择逻辑
5. 删除或标记废弃 `config.agent.tool_dispatcher` 配置字段（`src/config/schema.rs`）
6. 重写 `src/agent/tests.rs`：
   - 所有 `ConversationMessage::Chat(c)` 模式匹配改为直接检查 `ChatMessage` 的 role/content
   - 所有 `ConversationMessage::AssistantToolCalls { .. }` 和 `ConversationMessage::ToolResults(_)` 断言改为检查 `ChatMessage` 的 role 和 JSON content
   - 删除 Test 16（`ConversationMessage` 序列化往返测试）——不再有此类型
   - dispatcher 专属测试（XML parse、native roundtrip 等）已有 `loop_.rs` 内等价测试覆盖，确认后删除

---

## 7. 风险评估

| 风险 | 级别 | 缓解策略 |
|------|------|---------|
| Phase 2 改动 turn() 可能影响 WS 连接的多轮对话 | 中 | turn() 的 ~20 个测试覆盖多轮、history trim、tool call 等场景；重写后保持等价断言 |
| event_sender 在 loop 中的发送时机不精确 | 低 | 参照 turn() 现有的 AgentEvent 发送位置，6 个注入点一一对应 |
| ChatMessage history 丢失 AssistantToolCalls 结构信息 | 低 | `run_tool_call_loop` 的 `build_native_assistant_history` 已用 JSON 编码保留了等价信息 |
| Phase 3 删代码可能遗漏引用 | 极低 | Rust 编译器类型系统保证不会遗漏 |
| success 字段行为变更 | 中 | 当前 turn() 的 `execute_tool_call()` success 永远返回 true（bug）；切换到 `run_tool_call_loop()` 后 success 正确反映工具执行结果。WS 客户端（评测 APP 前端）如依赖此字段做 UI 渲染逻辑，需同步检查。发布时在 changelog 标注此行为修复 |
| `history()` 公开 API 返回类型变更 | 低 | `pub fn history()` 从 `&[ConversationMessage]` 变为 `&[ChatMessage]`。当前仅测试代码调用此方法，无外部消费者。若将来有外部 crate 依赖，属于 breaking change |
| response cache 语义微调 | 低 | 原实现每次 loop 迭代都检查 cache，改后仅 loop 前检查一次。实际影响接近零——多轮工具调用后 messages 已变化，后续迭代 cache 命中概率接近零 |
| trim_history 时机变更 | 极低 | 原实现在 loop 内每次工具结果后 trim，改后仅在 loop 结束后 trim 一次。max_tool_iterations 默认 10，最多多攒 ~20 条消息后一次性裁剪，内存影响可忽略 |
| event_sender 与 on_delta 误传 | 低 | 文档明确互斥规则。若两者同时传入会导致双重推送，但不影响正确性。可在 `run_tool_call_loop()` 入口加 debug_assert 防御 |

---

*文档结束*
