# MaxClaw (ZeroClaw) Docker 使用指南

**版本：** V1.0_202603171845
**日期：** 2026-03-17 18:45 (UTC+8)
**更新内容：** 初始版本

---

## 前置条件

- Docker 容器已构建并运行（`docker compose up -d --build`）
- `.env` 文件已配置 API Key、Provider 和 Model
- Gateway 监听端口：42617

---

## 第1步：配对（首次使用）

容器启动后需要完成一次设备配对，获取 Bearer Token。

### 获取配对码

```bash
docker compose logs --tail=20 zeroclaw
```

在日志中找到配对码，例如：

```
🔐 PAIRING REQUIRED — use this one-time code:
   ┌──────────────┐
   │  027991  │
   └──────────────┘
```

### 执行配对

```bash
curl -X POST http://localhost:42617/pair \
  -H "X-Pairing-Code: {配对码}"
```

成功响应：

```json
{
  "message": "Save this token — use it as Authorization: Bearer <token>",
  "paired": true,
  "persisted": true,
  "token": "zc_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
}
```

当前已获取的 Token：

```
zc_a59ce20de599dca07ebccbdc748da242991e67272491759d83760a2899d96e3e
```

> 注意：Token 通过 Docker Volume 持久化，容器重启后无需重新配对。仅在删除 Volume 后需要重新配对。

---

## 第2步：与 Agent 交互

以下所有请求均需在 Header 中携带 Token：

```
Authorization: Bearer zc_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
```

### 方式一：Webhook（HTTP POST）

最简单的方式，适合单次问答和脚本调用。

```bash
curl -s -X POST http://localhost:42617/webhook \
  -H "Authorization: Bearer {你的token}" \
  -H "Content-Type: application/json" \
  -d '{"message": "你好，请介绍一下你自己"}'
```

响应示例：

```json
{
  "model": "qwen/qwen3.5-plus-02-15",
  "response": "我是 AI 助手，可以帮助你解答问题、完成任务和提供信息支持。"
}
```

### 方式二：WebSocket（实时对话）

适合持续对话场景，支持流式响应。

连接地址：

```
ws://localhost:42617/ws/chat
```

可使用 `websocat` 等工具测试：

```bash
# 安装 websocat（macOS）
brew install websocat

# 连接（需携带 Token）
websocat "ws://localhost:42617/ws/chat" -H "Authorization: Bearer {你的token}"
```

### 方式三：Web Dashboard

浏览器直接访问：

```
http://localhost:42617/
```

提供可视化的 Web 界面进行交互。

### 方式四：REST API

通过 REST API 进行更精细的控制：

```
GET http://localhost:42617/api/*
```

需要 Bearer Token 认证。

---

## 运维命令

### 查看容器状态

```bash
docker compose ps
```

### 查看实时日志

```bash
docker compose logs -f zeroclaw
```

### 进入容器调试

```bash
docker exec -it zeroclaw bash
```

### 检查健康状态

```bash
curl http://localhost:42617/health
```

### 查看 Prometheus 指标

```bash
curl http://localhost:42617/metrics
```

### 重启容器

```bash
docker compose restart zeroclaw
```

### 停止服务

```bash
docker compose down
```

---

## 配置管理

### .env 文件

位于项目根目录，Docker Compose 自动读取：

```bash
API_KEY=你的LLM服务商API密钥
PROVIDER=openrouter
ZEROCLAW_MODEL=qwen/qwen3.5-plus-02-15
```

### 可配置项

| 配置项 | 默认值 | 修改方式 |
|---|---|---|
| Gateway 端口 | 42617 | `.env` 中设置 `HOST_PORT=其他端口` |
| LLM 模型 | anthropic/claude-sonnet-4 | `.env` 中设置 `ZEROCLAW_MODEL=模型ID` |
| LLM Provider | openrouter | `.env` 中设置 `PROVIDER=openai` 等 |
| CPU 限制 | 2 核 | `docker-compose.yml` → `deploy.resources.limits.cpus` |
| 内存限制 | 2GB | `docker-compose.yml` → `deploy.resources.limits.memory` |

### 切换模型

修改 `.env` 中的 `ZEROCLAW_MODEL` 后重启容器：

```bash
docker compose up -d
```

---

## API 端点总览

| 方法 | 端点 | 说明 | 认证 |
|---|---|---|---|
| POST | /pair | 设备配对 | X-Pairing-Code Header |
| POST | /webhook | 发送消息 | Bearer Token |
| GET | /ws/chat | WebSocket 对话 | Bearer Token |
| GET | /api/* | REST API | Bearer Token |
| GET | /health | 健康检查 | 无 |
| GET | /metrics | Prometheus 指标 | 无 |
| GET | / | Web Dashboard | 无 |
