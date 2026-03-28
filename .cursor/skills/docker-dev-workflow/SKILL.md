---
name: docker-dev-workflow
description: "Build, run, and manage ZeroClaw Docker containers, and send tasks to the running agent. Use when the user says: 运行, 在docker运行, run, 启动, 构建, 重建, 日志, 停止, 测试一下, 发个任务, 跑个任务, 试一下, send task, docker build, docker compose up, rebuild, restart, check logs, debug container, release image, container health, clean docker."
---

# Docker Dev Workflow

Execute Docker commands for ZeroClaw development — not just show them. Run via Shell tool, report results, surface errors. Confirm with the user before destructive operations (prune, push).

## Project Setup

Current project uses:
- `Dockerfile.debian` for build (supports bash shell for debugging)
- `docker-compose.yml` at repo root
- Container name: `zeroclaw`
- Gateway port: `42617` (host and container)
- Data volume: `zeroclaw-data` → `/zeroclaw-data`
- `.env` provides `API_KEY`, `PROVIDER`, `ZEROCLAW_MODEL`

## Quick Run

When user says "运行" / "在docker运行" / "run", execute this workflow:

### Pre-flight (once per session)

```bash
cd $(git rev-parse --show-toplevel)
docker info > /dev/null 2>&1 && echo "Docker OK" || echo "Docker not running"
ls .env 2>/dev/null && echo ".env OK" || echo ".env missing"
```

If `.env` missing, prompt user to create it:
```
API_KEY={llm-provider-key}
PROVIDER=openrouter
```

### Start / Rebuild

If `web/src/` has been modified, rebuild the frontend first — the Dockerfile does NOT run `npm run build`; it copies `web/dist/` as-is and embeds it into the binary via `rust-embed`:

```bash
cd web && npm ci && npm run build && cd ..
```

Then build and start:

```bash
docker compose up -d --build
```

- First build: 10–30 min (Rust compilation). Warn the user.
- Subsequent builds: incremental — only changed `src/` files recompile. Cargo registry/target caches via `--mount=type=cache`.
- Only `Cargo.toml`/`Cargo.lock` changes trigger full dependency rebuild.

### Verify

```bash
curl -sf http://localhost:42617/health && echo healthy || echo "not ready yet"
```

If not ready, wait 10–30s and retry. Check logs if persistent.

### Start without rebuild (image already built)

```bash
docker compose up -d
```

## Run Task

When user says "测试一下" / "发个任务" / "跑个任务" / "试一下", the container must already be running. First verify:

```bash
docker ps --filter name=zeroclaw --format "{{.Status}}"
```

If not running, fall back to Quick Run above.

### Single-shot task (recommended for quick test)

```bash
docker exec zeroclaw zeroclaw agent -m "your task here"
```

Replace `"your task here"` with the actual task. Ask user for task content if not provided.

### Interactive mode (multi-turn conversation)

```bash
docker exec -it zeroclaw zeroclaw agent
```

Type `/quit` to exit.

### HTTP API (for external/scripted access)

```bash
curl -X POST http://localhost:42617/webhook \
  -H "Content-Type: application/json" \
  -d '{"message": "your task here"}'
```

If pairing is enabled, add `-H "Authorization: Bearer {token}"`. Get the pairing code via `curl http://localhost:42617/paircode`.

### Model / provider override per task

```bash
docker exec zeroclaw zeroclaw agent -m "your task" -p anthropic --model claude-sonnet-4-20250514
```

## Debugging

```bash
# Stream logs
docker compose logs -f zeroclaw

# Recent logs only
docker compose logs --tail=100 zeroclaw

# Enter container shell
docker exec -it zeroclaw bash

# Agent status
docker exec zeroclaw zeroclaw status

# Gateway health
curl http://localhost:42617/health
```

"exec: bash: not found" → container uses distroless `Dockerfile`. Switch to `Dockerfile.debian` in `docker-compose.yml`.

## Modifying Container Config

Config lives at `/zeroclaw-data/.zeroclaw/config.toml` inside the Docker volume.

```bash
docker exec -u 0 zeroclaw sed -i 's/old_value/new_value/' /zeroclaw-data/.zeroclaw/config.toml
docker compose restart zeroclaw
```

**Do NOT use `docker cp`** — it changes file ownership to host user (UID 501), but the container runs as `nobody` (UID 65534), causing `Permission denied` restart loop.

Fix restart loop caused by wrong ownership:

```bash
docker compose stop zeroclaw
docker run --rm -v maxclaw_zeroclaw-data:/data alpine \
  sh -c "chown 65534:65534 /data/.zeroclaw/config.toml && chmod 644 /data/.zeroclaw/config.toml"
docker compose up -d
```

## Release

Use default `Dockerfile` (distroless) for production.

```bash
docker build -t maxclaw:release .
docker tag maxclaw:release {registry}/maxclaw:{version}
docker push {registry}/maxclaw:{version}
```

Ask user for registry URL and version tag before tagging/pushing.

## Stop & Cleanup

```bash
# Stop services
docker compose down
# Add --volumes ONLY if user explicitly wants to wipe zeroclaw-data

# Safe cleanup
docker image prune -f

# Full cleanup (destructive — confirm first)
docker system prune -af --volumes
```

## Configuration Reference

| Setting | Default | How to Change |
|---|---|---|
| Gateway port | 42617 | `HOST_PORT=xxxx` in `.env` |
| CPU limit | 2 cores | `deploy.resources.limits.cpus` in `docker-compose.yml` |
| Memory limit | 2 GB | `deploy.resources.limits.memory` in `docker-compose.yml` |
| Data volume | `zeroclaw-data` | Persists across `docker compose down` (not `--volumes`) |
| Agent config | `/zeroclaw-data/.zeroclaw/config.toml` | `docker exec -u 0 zeroclaw sed -i ...` then restart |

## Troubleshooting

| Symptom | Cause & Fix |
|---|---|
| Port 42617 in use | `lsof -i :42617` to find process; stop it or set `HOST_PORT=42618` in `.env` |
| Build stuck at "Compiling zeroclaw" | Normal first-time Rust build (10–30 min). Subsequent builds use cache. |
| Container exits immediately | Check `docker compose logs zeroclaw`. Common: missing `.env`, bad API key, config parse error. |
| Health check non-200 | Agent initializing; wait 10–30s and retry. If persistent, check logs for LLM errors. |
| Stale cache issues | Force full rebuild: `docker compose build --no-cache && docker compose up -d` |
| Web Dashboard changes not showing | `web/dist/` is embedded at compile time. Run `cd web && npm ci && npm run build` before `docker compose up -d --build`. |
| Config `Permission denied` restart loop | `docker cp` changed file owner. Use alpine container fix (see above). |
