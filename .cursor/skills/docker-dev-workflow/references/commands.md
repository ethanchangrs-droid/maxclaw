# Docker 命令速查

## 日常开发

```bash
# 构建并启动（改完代码后用）
docker compose up -d --build

# 仅启动（镜像未变时）
docker compose up -d

# 停止服务
docker compose down

# 停止并删除数据卷（慎用）
docker compose down --volumes
```

## 调试

```bash
# 实时日志
docker compose logs -f zeroclaw

# 最近 N 行日志
docker compose logs --tail=100 zeroclaw

# 进入容器（仅 Dockerfile.debian）
docker exec -it zeroclaw bash

# 容器内检查
docker exec zeroclaw zeroclaw status
docker exec zeroclaw zeroclaw doctor

# 健康检查
curl http://localhost:42617/health
```

## 发布

```bash
# 构建生产镜像（distroless）
docker build -t maxclaw:release .

# 打标签
docker tag maxclaw:release <registry>/maxclaw:<version>

# 推送
docker push <registry>/maxclaw:<version>
```

## 清理

```bash
# 清理未使用镜像（安全）
docker image prune -f

# 全量清理（危险，需确认）
docker system prune -af --volumes
```

## 强制重建

```bash
# 跳过缓存完全重建
docker compose build --no-cache

# 重建后启动
docker compose build --no-cache && docker compose up -d
```

## 端口排查

```bash
# 查找占用 42617 端口的进程
lsof -i :42617

# 使用备用端口（在 .env 中设置）
# HOST_PORT=42618
```
