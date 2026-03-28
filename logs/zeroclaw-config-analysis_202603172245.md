# ZeroClaw Docker 部署：日常配置 vs 评测配置分析

> 生成时间：2026-03-17 22:45 (UTC+8)

---

## 1. 项目配置架构总览

ZeroClaw 采用**三层配置覆盖**机制，优先级从高到低：

| 层级 | 来源 | 适用场景 |
|------|------|----------|
| 第 1 层 | 环境变量（`ZEROCLAW_*` / `API_KEY` / `PROVIDER` 等） | 运行时临时切换，评测场景 |
| 第 2 层 | 配置文件 `config.toml`（容器内 `/zeroclaw-data/.zeroclaw/config.toml`） | 日常持久化配置 |
| 第 3 层 | 代码默认值（`Config::default()`） | 未配置时的兜底 |

配置文件路径解析顺序：
1. `ZEROCLAW_CONFIG_DIR` 环境变量
2. `ZEROCLAW_WORKSPACE` 环境变量
3. `active_workspace.toml` 标记文件
4. 默认路径 `~/.zeroclaw/config.toml`

---

## 2. 当前 Docker 部署中的"日常配置"

### 2.1 环境变量层（`.env` 文件 → docker-compose.yml 注入）

```
API_KEY=sk-or-v1-...              # OpenRouter API Key
PROVIDER=openrouter                # LLM 提供商
ZEROCLAW_MODEL=qwen/qwen3.5-plus-02-15  # 使用的模型
```

这些通过 `docker-compose.yml` 的 `environment` 块注入容器：

```yaml
environment:
  - API_KEY=${API_KEY:-}
  - PROVIDER=${PROVIDER:-openrouter}
  - ZEROCLAW_MODEL=${ZEROCLAW_MODEL:-anthropic/claude-sonnet-4}
  - ZEROCLAW_ALLOW_PUBLIC_BIND=true
  - ZEROCLAW_GATEWAY_PORT=${ZEROCLAW_GATEWAY_PORT:-42617}
```

### 2.2 配置文件层（`config/config.toml` → 容器内持久卷）

容器内路径：`/zeroclaw-data/.zeroclaw/config.toml`

当前日常配置关键项：

| 配置项 | 值 | 含义 |
|--------|-----|------|
| default_provider | openrouter | LLM 提供商 |
| default_model | qwen/qwen3.5-plus-02-15 | 默认模型 |
| default_temperature | 0.7 | 生成温度 |
| provider_timeout_secs | 120 | API 超时 |
| autonomy.level | supervised | 自治级别（需审批） |
| autonomy.max_actions_per_hour | 20 | 每小时最大操作数 |
| autonomy.max_cost_per_day_cents | 500 | 每日成本上限（分） |
| agent.max_tool_iterations | 10 | 单轮最大工具调用 |
| agent.max_history_messages | 50 | 历史消息上限 |
| agent.max_context_tokens | 32000 | 上下文 token 上限 |
| gateway.port | 42617 | 网关端口 |
| gateway.require_pairing | true | 设备配对 |
| security.resources.max_memory_mb | 512 | 内存限制 |
| security.resources.max_cpu_time_seconds | 60 | CPU 时间限制 |

### 2.3 Docker 资源限制层

```yaml
deploy:
  resources:
    limits:
      cpus: '2'
      memory: 2G
    reservations:
      cpus: '0.5'
      memory: 512M
```

---

## 3. 评测配置的特殊之处

### 3.1 评测目录挂载

`docker-compose.yml` 中包含评测专用的 bind-mount：

```yaml
volumes:
  - /Users/david/Desktop/pitem/downloadcode/agent/agenteval/evalspace:/zeroclaw-data/workspace/evalspace
```

这将宿主机的 `agenteval/evalspace` 目录挂载到容器内的 `/zeroclaw-data/workspace/evalspace`，使评测用例能在容器内被 agent 读写。

### 3.2 评测时可能需要调整的配置项

评测场景与日常使用的核心差异：

| 维度 | 日常配置 | 评测配置建议 | 调整方式 |
|------|----------|-------------|---------|
| 模型 | qwen/qwen3.5-plus-02-15 | 按评测需要切换（如 claude-sonnet-4） | 修改 `.env` 中 `ZEROCLAW_MODEL` |
| 提供商 | openrouter | 按模型要求切换 | 修改 `.env` 中 `PROVIDER` |
| Temperature | 0.7 | 评测通常用 0（确定性输出） | 修改 config.toml 或设置 `ZEROCLAW_TEMPERATURE` |
| 自治级别 | supervised（需审批） | 评测可能需要 autonomous | 修改 config.toml `autonomy.level` |
| 每小时操作上限 | 20 | 评测可能需要更高 | 修改 config.toml `autonomy.max_actions_per_hour` |
| 工具调用上限 | 10 次/轮 | 复杂任务可能需要更多 | 修改 config.toml `agent.max_tool_iterations` |
| 上下文 Token | 32000 | 大模型可提高到 128000+ | 修改 config.toml `agent.max_context_tokens` |
| API 超时 | 120s | 推理模型可能需要更长 | 修改 config.toml `provider_timeout_secs` |
| Shell 命令白名单 | 有限列表 | 评测可能需要更多命令 | 修改 config.toml `autonomy.allowed_commands` |
| 禁止路径 | 系统路径 | 评测可能需要放宽 | 修改 config.toml `autonomy.forbidden_paths` |

---

## 4. 配置生效机制

### 4.1 启动时加载

```
容器启动 → zeroclaw gateway
  │
  ├─ Config::load_or_init()
  │   ├─ 解析 config.toml（/zeroclaw-data/.zeroclaw/config.toml）
  │   ├─ apply_env_overrides()  ← 环境变量覆盖配置文件值
  │   └─ validate()
  │
  └─ 启动 Gateway 服务
```

### 4.2 环境变量覆盖优先级

在 `apply_env_overrides()` 方法中，环境变量按以下优先级覆盖 config.toml：

**API Key 优先级**：`ZEROCLAW_API_KEY` > `API_KEY` > config.toml 中的 `api_key`

**Provider 优先级**：`ZEROCLAW_PROVIDER` > `ZEROCLAW_MODEL_PROVIDER` / `MODEL_PROVIDER` > `PROVIDER`（仅当 config 未自定义时） > config.toml

**Model 优先级**：`ZEROCLAW_MODEL` > `MODEL` > config.toml 中的 `default_model`

### 4.3 修改配置后如何生效

| 修改对象 | 生效方式 | 命令 |
|----------|---------|------|
| `.env` 文件 | 重建容器 | `docker compose up -d --force-recreate` |
| docker-compose.yml 中的 environment | 重建容器 | `docker compose up -d --force-recreate` |
| config.toml（持久卷内） | 重启容器 | `docker compose restart` |
| config.toml（宿主机 config/ 目录） | 需先复制到容器卷内 | 手动复制后重启 |

> 注意：当前 `docker-compose.yml` 未将宿主机的 `config/config.toml` 挂载到容器内。容器内的 config.toml 位于 Docker Volume `zeroclaw-data` 中（`/zeroclaw-data/.zeroclaw/config.toml`），是构建时生成的默认配置，后续通过 `zeroclaw onboard` 命令更新。

### 4.4 当前配置的潜在问题

1. **config.toml 双份存在**：宿主机 `config/config.toml` 是详细配置，但未挂载到容器中。容器内使用的是 Dockerfile 构建时生成的精简默认配置。两者内容不同步。

2. **评测目录固定绑定**：evalspace 的宿主机路径是硬编码的绝对路径，换机器需要修改。

3. **环境变量与 config.toml 重叠**：`ZEROCLAW_MODEL` 和 config.toml 中的 `default_model` 目前设为相同值（`qwen/qwen3.5-plus-02-15`），若需要切换，只改一处即可（环境变量优先）。

---

## 5. 快速操作指南

### 切换评测模型

```bash
# 修改 .env 文件
ZEROCLAW_MODEL=anthropic/claude-sonnet-4

# 重建容器使其生效
docker compose up -d --force-recreate
```

### 进入容器查看当前实际配置

```bash
docker exec -it zeroclaw cat /zeroclaw-data/.zeroclaw/config.toml
```

### 将宿主机 config.toml 同步到容器（如需使用详细配置）

方法一 — 在 docker-compose.yml 中添加 bind-mount：

```yaml
volumes:
  - zeroclaw-data:/zeroclaw-data
  - ./config/config.toml:/zeroclaw-data/.zeroclaw/config.toml:ro
```

方法二 — 手动复制：

```bash
docker cp config/config.toml zeroclaw:/zeroclaw-data/.zeroclaw/config.toml
docker compose restart
```

### 查看运行时环境变量

```bash
docker exec -it zeroclaw env | grep -E 'API_KEY|PROVIDER|MODEL|GATEWAY'
```

---

## 6. 总结

| 项目 | 日常使用 | 评测使用 |
|------|---------|---------|
| 核心差异 | 使用廉价模型，受限自治 | 按需切换模型，可能放宽限制 |
| 模型切换 | 修改 `.env` 中 `ZEROCLAW_MODEL` | 同左 |
| 配置持久化 | config.toml 在 Docker Volume 中 | 同左 |
| 文件共享 | 标准 workspace | evalspace bind-mount |
| 生效方式 | 改 .env → `docker compose up -d --force-recreate` | 同左 |
| 改 config.toml | 需重启或重建容器 | 同左 |
