# Reasoning & Tool Call 实时展示功能实现

- 时间：2026-03-18 10:49 (UTC+8)
- 分支：feature/all

## 用户要求

在 feature/all 分支上实现 reasoning（思维过程）和 tool call（工具调用）的实时用户展示功能：
1. 实时展示 LLM 的 thinking 过程
2. 实时展示 tool call 状态（调用中/完成/失败）
3. reasoning_content 落到 run_trace 日志记录中

## 实施方案

采用最小侵入式方案：在现有 `Agent::turn()` 中注入事件发送逻辑，不重构不复制，降低与上游合并冲突风险。

## 执行过程

### 1. 定义 AgentEvent 枚举（src/agent/agent.rs）
- 新增 `AgentEvent` 枚举：`Reasoning`, `ToolCallStart`, `ToolCallComplete`, `Done`
- Agent 结构体新增 `event_sender: Option<tokio::sync::mpsc::Sender<AgentEvent>>` 字段
- 新增 `set_event_sender()` setter 方法
- 通过 `src/agent/mod.rs` 导出 `AgentEvent`

### 2. turn() 内插入 4 处事件发送（src/agent/agent.rs）
- 插入点 A：LLM 响应后，发送 `AgentEvent::Reasoning`
- 插入点 B：无 tool calls 最终响应前，发送 `AgentEvent::Done`
- 插入点 C：tool calls 执行前，发送 `AgentEvent::ToolCallStart`
- 插入点 D：tool calls 执行后，发送 `AgentEvent::ToolCallComplete`
- 每处都是 `if let Some(ref tx) = self.event_sender` 条件发送，无 sender 时零开销

### 3. runtime_trace 记录（src/agent/loop_.rs + agent.rs）
- `run_tool_call_loop` 的 `llm_response` trace payload 增加 `reasoning_content` 字段
- `run_tool_call_loop` 的 `turn_final_response` trace payload 增加 `reasoning_content` 字段
- 为支持 `turn_final_response` 访问 reasoning_content，将变量加入 match 返回元组
- `turn()` 中 LLM 响应和最终响应处也写入 runtime_trace 记录

### 4. WebSocket handler 改造（src/gateway/ws.rs）
- 引入 AgentEvent mpsc channel，调用 `agent.set_event_sender(Some(event_tx))`
- 新建 ws_write_tx/ws_write_rx mpsc channel 作为统一的 WebSocket 写入队列，解决 SplitSink 不可 clone 问题
- 提取独立 `forward_agent_events()` 函数：接收 AgentEvent，转换为 JSON WebSocket 消息
- Done 事件由主循环的 `agent.turn()` 返回后直接发送，forward 中跳过

### 5. 前端类型扩展（web/src/types/api.ts）
- `WsMessage.type` 增加 `'reasoning'`
- 新增 `success?: boolean` 和 `duration_ms?: number` 字段

### 6. 前端新组件
- `web/src/components/ReasoningBlock.tsx`：折叠式 Thinking 面板，Brain 图标 + 展开/收起
- `web/src/components/ToolCallCard.tsx`：工具调用/结果卡片，状态图标 + 参数/输出可展开

### 7. AgentChat.tsx 更新
- ChatMessage 接口增加 `type`, `toolName`, `toolArgs`, `toolOutput`, `toolSuccess`, `toolDuration` 字段
- onMessage handler 增加 `reasoning` case，修改 `tool_call` 和 `tool_result` case 存储结构化数据
- 渲染区域根据 `msg.type` 条件渲染：reasoning/tool_call/tool_result 用新组件，其余保持不变

## 结果

- Rust 编译通过（cargo check 成功）
- 前端无 lint 错误
- 改动文件清单：
  - `src/agent/agent.rs` - AgentEvent 定义 + turn() 事件注入
  - `src/agent/mod.rs` - 导出 AgentEvent
  - `src/agent/loop_.rs` - trace payload 增加 reasoning_content
  - `src/gateway/ws.rs` - WebSocket 事件转发
  - `web/src/types/api.ts` - WsMessage 类型扩展
  - `web/src/components/ReasoningBlock.tsx` - 新文件
  - `web/src/components/ToolCallCard.tsx` - 新文件
  - `web/src/pages/AgentChat.tsx` - 渲染逻辑更新
