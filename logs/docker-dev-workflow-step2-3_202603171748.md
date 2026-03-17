# Docker 开发工作流 — 第2步 & 第3步执行记录

**日期：** 2026-03-17 17:48 (UTC+8)
**参考文档：** `MaxClaw_Docker_Dev_Workflow_V1.0_202603171601.md`

---

## 用户要求

完成文档中的第2步和第3步：
- 第2步：修改 `docker-compose.yml`，将预构建镜像替换为本地构建（使用 `Dockerfile.debian`）
- 第3步：创建 `.env` 文件配置 API Key

---

## 执行过程

### 第2步：修改 docker-compose.yml

**修改前：**
```yaml
    image: ghcr.io/zeroclaw-labs/zeroclaw:latest
    # Or build locally (distroless, no shell):
    # build: .
    # Or build the Debian variant (includes bash, git, curl):
    # build:
    #   context: .
    #   dockerfile: Dockerfile.debian
```

**修改后：**
```yaml
    # image: ghcr.io/zeroclaw-labs/zeroclaw:latest
    build:
      context: .
      dockerfile: Dockerfile.debian
```

- 注释掉了远程预构建镜像
- 启用了本地构建，使用 `Dockerfile.debian`（Debian 变体，含 bash/git/curl，方便调试）

### 第3步：创建 .env 文件

在项目根目录创建了 `.env` 文件，内容：

```
API_KEY=你的LLM服务商API密钥
PROVIDER=openrouter
```

**安全检查：** `.gitignore` 已包含 `.env` 规则（第16行），密钥文件不会被提交到 Git。

---

## 执行结果

| 步骤 | 状态 | 说明 |
|---|---|---|
| 第2步：修改 docker-compose.yml | 已完成 | 已切换为本地 Dockerfile.debian 构建 |
| 第3步：创建 .env 文件 | 已完成 | 需用户替换实际 API Key |

---

## 后续步骤

用户需要将 `.env` 文件中的 `API_KEY=你的LLM服务商API密钥` 替换为实际的 API 密钥，然后即可执行第4步：

```bash
docker compose up -d --build
```
