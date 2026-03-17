# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Validate Commands

```bash
# Lint
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings

# Test (all unit + component + integration + system)
cargo test

# Run a single test by name
cargo test test_name_here

# Run a specific test suite
cargo test --test component
cargo test --test integration
cargo test --test system
cargo test --test live -- --ignored   # requires credentials

# Build
cargo build --release --locked
cargo build --profile release-fast    # faster build, needs 16GB+ RAM

# Full pre-PR validation (Docker-based: lint + test + build + security + docker-smoke)
./dev/ci.sh all

# Individual CI steps
./dev/ci.sh lint          # fmt + clippy
./dev/ci.sh test          # cargo test --locked
./dev/ci.sh build         # release build
./dev/ci.sh security      # cargo audit + cargo deny
./dev/ci.sh lint-delta    # strict clippy on changed lines only

# Dev fallback (no global install)
cargo run --release -- <subcommand>
```

Docs-only changes: run markdown lint and link-integrity checks. If touching bootstrap scripts: `bash -n install.sh`.

## Project Overview

ZeroClaw is a Rust-first autonomous agent runtime. Single binary, <5MB RAM, <10ms cold start. Dual-licensed MIT/Apache-2.0.

Workspace: `Cargo.toml` defines a workspace with members `"."` and `crates/robot-kit`. MSRV is 1.87.

## Architecture

### Trait-Driven Extension Model

Every major subsystem is a trait + factory. To add a new implementation: implement the trait, register it in the corresponding `mod.rs` factory function.

| Subsystem | Trait | Trait File | Factory/Registration |
|-----------|-------|-----------|---------------------|
| LLM Provider | `Provider` | `src/providers/traits.rs` | `src/providers/mod.rs` |
| Channel | `Channel` | `src/channels/traits.rs` | `src/channels/mod.rs` |
| Tool | `Tool` | `src/tools/traits.rs` | `src/tools/mod.rs` |
| Memory | `Memory` | `src/memory/traits.rs` | `src/memory/mod.rs` |
| Observer | `Observer` | `src/observability/traits.rs` | `src/observability/mod.rs` |
| Runtime | `RuntimeAdapter` | `src/runtime/traits.rs` | `src/runtime/mod.rs` |
| Peripheral | `Peripheral` | `src/peripherals/traits.rs` | `src/peripherals/mod.rs` |
| Sandbox | `Sandbox` | `src/security/traits.rs` | `src/security/sandbox/detect.rs` |

### Agent Orchestration (src/agent/)

- `agent.rs` — `Agent` struct built via `AgentBuilder` (builder pattern). Holds `Arc<dyn Provider>`, `Arc<dyn Memory>`, `Arc<dyn Observer>`, etc.
- `loop_.rs` — Main orchestration: `run()` → `process_message()` → LLM call → tool use loop (max 10 iterations) → response. Includes credential scrubbing and tool filtering per turn.
- `dispatcher.rs` — `ToolDispatcher` parses LLM function calls, invokes tools, collects results. Has `NativeToolDispatcher` and `XmlToolDispatcher` variants.
- `prompt.rs` — `SystemPromptBuilder` injects tool specs, memory context, identity, skills into system prompt.
- `classifier.rs` — Routes user messages to models by classification hints.

### Config System (src/config/)

- `schema.rs` — Monolithic config schema (`serde` + `schemars::JsonSchema`). Top-level struct with nested sections for all subsystems.
- `workspace.rs` — Config resolution: `ZEROCLAW_WORKSPACE` env → `active_workspace.toml` marker → `~/.zeroclaw/config.toml`. Supports env var overrides (e.g., `ZEROCLAW_API_KEY`).
- Format: TOML (`config.toml`).

### Gateway (src/gateway/)

Axum + Tower HTTP server. Key aspects:
- Rate limiting (sliding-window per IP), 64KB body limit, 30s timeout
- Endpoints: `POST /pair`, `GET /paircode`, `POST /webhook/{channel}`, `GET /ws`
- Idempotency store prevents double-processing of retried webhooks
- Pairing guard requires device authentication before webhook acceptance

### Security (src/security/)

- `policy.rs` — `SecurityPolicy` with `AutonomyLevel` (Supervised/Autonomous/Experimental), domain allowlist/blocklist
- `pairing.rs` — Device auth with constant-time OTP comparison
- `secrets.rs` — Encrypted credential storage (ChaCha20-Poly1305)
- `estop.rs` — Emergency stop (KillAll, NetworkKill, DomainBlock, ToolFreeze), OTP-gated resume
- `workspace_boundary.rs` — Prevents tool execution outside workspace
- `sandbox/` — Pluggable backends: Docker, Firejail, Bubblewrap, Landlock; auto-detected at runtime
- `prompt_guard.rs` — Prompt injection defense
- `leak_detector.rs` — Credential leakage detection in tool outputs

### Shared Patterns

- **Async-first on Tokio.** All traits use `async_trait`. Channels use `tokio::sync::mpsc`.
- **Arc-wrapped sharing.** Provider, Memory, Observer, SecurityPolicy passed as `Arc<dyn Trait>`.
- **Feature gates** for optional subsystems: `hardware`, `channel-matrix`, `channel-lark`, `channel-nostr`, `whatsapp-web`, `browser-native`, `sandbox-landlock`, `observability-otel`, `memory-postgres`, `rag-pdf`, `probe`. Default features: `observability-prometheus`, `channel-nostr`.

## Test Organization

```
tests/
  test_component.rs   → includes tests/component/   (unit-level)
  test_integration.rs → includes tests/integration/  (multi-subsystem)
  test_system.rs      → includes tests/system/       (end-to-end)
  test_live.rs        → includes tests/live/         (requires credentials, #[ignore])
  fixtures/           → test data
  support/            → test utilities
  manual/             → manual test scripts (e.g., test_dockerignore.sh)
```

Inline `#[cfg(test)]` modules exist within each subsystem as well.

## Risk Tiers

- **Low risk**: docs/chore/tests-only changes
- **Medium risk**: most `src/**` behavior changes without boundary/security impact
- **High risk**: `src/security/**`, `src/runtime/**`, `src/gateway/**`, `src/tools/**`, `.github/workflows/**`, access-control boundaries

When uncertain, classify as higher risk.

## Workflow

1. **Read before write** — inspect existing module, factory wiring, and adjacent tests before editing.
2. **One concern per PR** — avoid mixed feature+refactor+infra patches.
3. **Implement minimal patch** — no speculative abstractions, no config keys without a concrete use case.
4. **Validate by risk tier** — docs-only: lightweight checks. Code changes: full relevant checks.
5. **Document impact** — update PR notes for behavior, risk, side effects, and rollback.
6. **Queue hygiene** — stacked PR: declare `Depends on #...`. Replacing old PR: declare `Supersedes #...`.

Branch/commit/PR rules:
- Work from a non-`master` branch. Open a PR to `master`; do not push directly.
- Use conventional commit titles. Prefer small PRs (`size: XS/S/M`).
- Follow `.github/pull_request_template.md` fully.
- Never commit secrets, personal data, or real identity information (see `@docs/contributing/pr-discipline.md`). Use neutral placeholders: `user_a`, `test_user`, `example.com`.

## Anti-Patterns

- Do not add heavy dependencies for minor convenience.
- Do not silently weaken security policy or access constraints.
- Do not add speculative config/feature flags "just in case".
- Do not mix massive formatting-only changes with functional changes.
- Do not modify unrelated modules "while here".
- Do not bypass failing checks without explicit explanation.
- Do not hide behavior-changing side effects in refactor commits.
- Do not introduce cross-subsystem coupling (providers must not import channels internals, etc.).

## Linked References

- `docs/contributing/change-playbooks.md` — adding providers, channels, tools, peripherals; security/gateway changes; architecture boundaries
- `docs/contributing/pr-discipline.md` — privacy rules, superseded-PR attribution/templates, handoff template
- `docs/contributing/docs-contract.md` — docs system contract, i18n rules, locale parity
