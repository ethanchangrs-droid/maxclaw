# WebSocket Reasoning/Done 消息顺序竞态修复

## 用户要求
界面最后一条 thinking 显示在 text 之后，但应该显示在 text 之前。分析问题层面并修复。

## 问题分析

### 现象
WebSocket 聊天界面中，Thinking（reasoning）块偶尔显示在文本响应之后，顺序颠倒。

### 根因
`src/gateway/ws.rs` 的 `handle_socket` 中存在竞态条件：

1. Agent 层（`agent.rs` `turn()`）通过 `event_sender` 通道按正确顺序发出 `Reasoning` → `Done` 事件
2. `forward_agent_events` 任务从通道读取事件转发为 WS 消息，但 **故意跳过** `Done` 事件（`continue`）
3. 主循环在 `turn()` 返回后 **直接** 向 `ws_write_tx` 发送 `done` 消息
4. 两个独立写入者（forward 任务 vs 主循环）没有同步机制，`done` 可能抢先于 `reasoning`

### 影响范围
- 仅影响 WebSocket 聊天界面（`/ws/chat`）
- CLI 和 Channel 通道不受影响（使用 `loop_.rs` 的不同代码路径）

## 修复方案（方案 A）

统一为单写入者：所有 agent 事件（reasoning、tool_call、tool_result、done）都通过 `forward_agent_events` 发出，利用 mpsc 通道的 FIFO 保证顺序。

### 代码变更

文件：`src/gateway/ws.rs`

1. `forward_agent_events` 中 `AgentEvent::Done` 从 `continue` 改为构造并发送 `done` WS 消息
2. `handle_socket` 主循环 `Ok` 分支不再直接发送 `done`，仅保留 `agent_end` SSE 广播
3. 更新注释，反映新的单写入者架构

### 验证
- `cargo check`：通过
- `cargo clippy --all-targets -- -D warnings`：零警告
- Linter：无错误

## 后续任务（已记录到 feature_list.json）
- F002（P2）：WebSocket 真正流式输出（Provider SSE 级别）
- F003（P2）：统一 agent loop（消除 agent.rs / loop_.rs 双实现）
