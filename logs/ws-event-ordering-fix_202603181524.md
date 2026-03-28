# WebSocket 事件顺序修复

- 时间：2025-03-18 15:24 (UTC+8)
- 类型：Bug 修复

## 用户要求

修复 WebSocket 聊天中工具执行结果（tool_result）显示在错误消息（error）之后的问题。同时排查并修复 `turn()` 方法中其他未通过 AgentEvent 发送的消息。

## 根因分析

### 竞态条件

`agent.rs` 的 `turn()` 方法通过 `event_sender`（mpsc channel）发送 AgentEvent，由 `ws.rs` 的 `forward_agent_events` 异步任务转发到 WebSocket。但错误消息直接通过 `ws_write_tx` 发送，绕过了 FIFO 通道。当迭代耗尽时，最后一次的 `ToolCallComplete` 事件可能还在 forwarder 队列中，error 消息却抢先到达客户端。

### 遗漏的 AgentEvent

审计发现 `turn()` 中还有两处消息未通过 AgentEvent 发送：
1. 缓存命中时直接 return，没有发送 `Done` 事件
2. 混合回复（文本+工具调用）的中间文本只打印到 stdout，未发送到前端

## 修复方案

### 1. AgentEvent 新增变体（agent.rs）
- `Chunk(String)` — 用于混合回复的中间文本
- `Error { message: String }` — 用于错误消息走 FIFO 通道

### 2. turn() 补齐事件（agent.rs）
- 缓存命中时发送 `AgentEvent::Done`
- 混合回复中间文本发送 `AgentEvent::Chunk`

### 3. ws.rs 统一 FIFO 发送
- 克隆 `event_tx` 为 `error_event_tx`
- `Err` 分支改为通过 `error_event_tx` 发送 `AgentEvent::Error`，保证错误消息排在所有已入队事件之后
- `forward_agent_events` 新增 `Chunk` 和 `Error` 事件的处理

### 4. 前端无需修改
- `WsMessage` 已定义 `chunk` 和 `error` 类型
- `AgentChat.tsx` 已有对应处理逻辑

## 改动文件

| 文件 | 改动 |
|------|------|
| src/agent/agent.rs | AgentEvent 新增 Chunk/Error 变体；缓存命中发 Done；混合回复发 Chunk |
| src/gateway/ws.rs | 克隆 event_tx；error 走 event channel；forwarder 处理新事件类型 |

## 验证

- cargo clippy: 通过（0 warnings）
- cargo test: 4042 passed, 5 failed（均为既有沙盒权限问题）
- agent 测试: 11/11 通过
- gateway 测试: 9/9 通过
