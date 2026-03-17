---
name: docker-dev-workflow
description: "Build, run, debug, and release ZeroClaw containers with Docker Compose. Use when the user wants to rebuild after code changes, check container logs, enter the container shell, verify service health, tag or push a release image, stop services, or clean up Docker resources. Triggers on: docker build, docker compose up, rebuild, restart container, check logs, debug container, release image, push image, container health, clean docker."
---

# Docker Dev Workflow

Execute Docker commands for ZeroClaw development — not just show them. Run via Shell tool, report results, surface errors. Confirm with the user before destructive operations (prune, push).

## Pre-flight Checks

Before any Docker operation, verify once per session:

1. **Repo root:** `ls Dockerfile docker-compose.yml` — if missing, `cd $(git rev-parse --show-toplevel)`
2. **Docker running:** `docker info > /dev/null 2>&1 && echo ok || echo "Docker not running"` — if not, tell user to start Docker Desktop
3. **`.env` exists:** `ls .env 2>/dev/null` — if missing and user wants to start a container, prompt to create it:
   ```
   API_KEY=<llm-provider-key>
   PROVIDER=openrouter   # openrouter | openai | anthropic | ollama
   ```

## Development Loop

After code changes, rebuild and restart in one step:

```bash
docker compose up -d --build
```

- First build: 10–30 min (Rust compilation). Warn the user.
- Subsequent builds: incremental — only changed `src/` files recompile. Dependencies cached via `--mount=type=cache` in Dockerfile.
- After start, verify: `curl -sf http://localhost:42617/health && echo healthy || echo "not ready yet"`

**Cache mechanism:** Dockerfile uses multi-stage build with Cargo registry/target caches. Only `Cargo.toml`/`Cargo.lock` changes trigger full dependency rebuild.

## Debugging

```bash
# Stream logs (Ctrl+C to stop)
docker compose logs -f zeroclaw

# Recent logs only
docker compose logs --tail=100 zeroclaw

# Enter container shell (Dockerfile.debian only)
docker exec -it zeroclaw bash

# Agent status
docker exec zeroclaw zeroclaw status

# Gateway health
curl http://localhost:42617/health
```

**"exec: bash: not found"** → container uses distroless `Dockerfile`. Switch to `Dockerfile.debian` in `docker-compose.yml` for debugging.

## Release

Use default `Dockerfile` (distroless) for production — smaller and more secure.

```bash
docker build -t maxclaw:release .
docker tag maxclaw:release <registry>/maxclaw:<version>
docker push <registry>/maxclaw:<version>
```

Ask user for registry URL and version tag before tagging/pushing.

## Other Operations

```bash
# Start without rebuild (image unchanged)
docker compose up -d

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

## Troubleshooting

| Symptom | Cause & Fix |
|---|---|
| Port 42617 in use | `lsof -i :42617` to find process; stop it or set `HOST_PORT=42618` in `.env` |
| Build stuck at "Compiling zeroclaw" | Normal first-time Rust build (10–30 min). Subsequent builds use cache. |
| Container exits immediately | Check `docker compose logs zeroclaw`. Common: missing `.env`, bad API key, config parse error. |
| Health check non-200 | Agent initializing; wait 10–30s and retry. If persistent, check logs for LLM errors. |
| Stale cache issues | Force full rebuild: `docker compose build --no-cache && docker compose up -d` |

For full command reference, see [commands.md](references/commands.md).
