# F003: Agent 核心 Loop 统一与 History 类型统一

## 用户要求
开发 F003 — 将 Agent::turn() 的内部 loop 体替换为对 run_tool_call_loop() 的委托调用，消除两套重复实现，统一 history 类型。

## 任务计划
基于设计文档 `docs/architecture/agent-loop-unification_V1.1_202603251903.md` 的 3 阶段方案执行：

1. **Phase 1**: run_tool_call_loop 新增 event_sender 参数 + LoopOutcome 返回值
2. **Phase 2**: turn() 委托到 run_tool_call_loop
3. **Phase 3**: 清理废弃代码 + 测试重写

## 执行过程

### Phase 1: run_tool_call_loop 增强
- 新增 `LoopOutcome` 结构体（text + iterations_used）
- 在 `run_tool_call_loop` 签名中添加 `event_sender: Option<Sender<AgentEvent>>`
- 6 处注入 AgentEvent 发送：
  - Reasoning: LLM 返回 reasoning_content 时
  - ToolCallStart: 工具执行前
  - ToolCallComplete: 工具执行后
  - Chunk: 工具调用同时有文本输出时
  - Done: 无工具调用的最终文本响应时
- 更新所有 call sites（loop_.rs 内部 3 处、channels/mod.rs、tools/delegate.rs）

### Phase 2: turn() 重构
- Agent.history 类型从 `Vec<ConversationMessage>` 改为 `Vec<ChatMessage>`
- 删除 execute_tool_call() / execute_tools() 方法
- turn() 方法体完全重写为对 run_tool_call_loop() 的委托：
  - System prompt + user message 插入 history
  - Memory auto-save 逻辑
  - Response cache 检查（调用前）+ 存储（调用后，基于 iterations_used == 1）
  - 传入 event_sender
  - 最终 trim_history()
- trim_history() 重写为直接操作 ChatMessage 字段
- build_system_prompt() 改用 provider.supports_native_tools() 判断
- AgentBuilder 移除 tool_dispatcher 字段

### Phase 3: 清理与测试
- 删除 `ConversationMessage` 枚举（src/providers/traits.rs）
- 删除 `ToolDispatcher` trait + `NativeToolDispatcher` + `XmlToolDispatcher`
- 删除 `src/agent/dispatcher.rs` 文件（443 行）
- 移除 `pub mod dispatcher` 从 agent/mod.rs
- 废弃 `config.agent.tool_dispatcher` 配置字段（保留 serde 向后兼容）
- 重命名 `PromptContext.dispatcher_instructions` → `tool_instructions`
- 重写 agent/tests.rs 中 22 个测试：
  - 所有 ConversationMessage 断言改为 ChatMessage 字段访问
  - 修复去重逻辑导致的测试行为不匹配（工具调用参数唯一化）
  - 删除 4 个与已删除类型相关的过时测试
- 更新 integration/agent.rs 测试
- 清理 file_read.rs、support/helpers.rs、benchmarks 中的 dispatcher 引用

## 结果
- **编译**: cargo check 零错误通过
- **测试**: 8344+ 测试全部通过，0 失败
- **代码变化**: 17 个文件，+426 / -1323 行（净减 897 行）
- **Git commit**: 286f8cda feat(agent): unify agent loop and history type (F003)
- **Feature status**: F003 标记 passes: true

## 涉及文件
| 文件 | 变更类型 |
|---|---|
| src/agent/agent.rs | 大幅重构 |
| src/agent/dispatcher.rs | 删除 |
| src/agent/loop_.rs | 新增 LoopOutcome + event_sender |
| src/agent/mod.rs | 移除 dispatcher 模块 |
| src/agent/prompt.rs | 字段重命名 |
| src/agent/tests.rs | 大幅重写 |
| src/channels/mod.rs | 适配 LoopOutcome |
| src/config/schema.rs | tool_dispatcher 废弃标记 |
| src/gateway/ws.rs | AgentEvent 转发逻辑 |
| src/providers/mod.rs | 移除 ConversationMessage 导出 |
| src/providers/traits.rs | 删除 ConversationMessage 枚举 |
| src/tools/delegate.rs | 适配 LoopOutcome |
| src/tools/file_read.rs | 移除 dispatcher 测试引用 |
| tests/component/config_persistence.rs | deprecated 标记 |
| tests/integration/agent.rs | 适配 ChatMessage |
| tests/support/helpers.rs | 移除 dispatcher 引用 |
| benches/agent_benchmarks.rs | 移除 dispatcher 基准测试 |
