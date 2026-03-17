# 修复 Docker 构建编译错误

**日期：** 2026-03-17 18:08 (UTC+8)

---

## 用户要求

修复 Docker 构建（`docker compose up -d --build`）时出现的 3 个 Rust 编译错误。

---

## 错误分析

### 原始错误

| 错误 | 位置 | 信息 |
|---|---|---|
| E0432 | src/main.rs:79 | `unresolved import zeroclaw::rag` |
| E0432 | src/main.rs:113-114 | `unresolved imports zeroclaw::ChannelCommands` 等 9 个枚举 |
| E0282 | src/agent/loop_.rs:3244 | `type annotations needed` for `rag.len()` |

### 根因分析

**源代码本身没有错误。** 问题出在 `Dockerfile.debian` 的构建缓存策略上。

与默认 `Dockerfile` 对比发现两个缺陷：

1. **缺少指纹清除步骤**：默认 `Dockerfile` 在编译前会清除 `zeroclawlabs` 的缓存指纹（`.fingerprint`, `deps`, `incremental`），而 `Dockerfile.debian` 没有这一步。Docker 的 `--mount=type=cache` 让 `target/` 目录在构建间持久化，cargo 因 Docker COPY 保留宿主机原始 mtime 而跳过了 lib 重编译，导致二进制引用的是依赖缓存阶段的空 lib。

2. **运行时 GLIBC 版本不匹配**：构建阶段用 `rust:1.94-slim`（基于 Debian trixie, GLIBC 2.39），运行时却用 `debian:bookworm-slim`（GLIBC 2.36），导致编译出的二进制无法在运行时容器中执行。

---

## 修复内容

### 修复 1：添加指纹清除 + touch lib.rs

```dockerfile
# 修改前
RUN touch src/main.rs
RUN --mount=type=cache,... \
    cargo build --release --locked && ...

# 修改后
RUN touch src/main.rs src/lib.rs
RUN --mount=type=cache,... \
    rm -rf target/release/.fingerprint/zeroclawlabs-* \
           target/release/deps/zeroclawlabs-* \
           target/release/incremental/zeroclawlabs-* && \
    cargo build --release --locked && ...
```

### 修复 2：运行时镜像升级为 trixie

```dockerfile
# 修改前
FROM debian:bookworm-slim AS runtime

# 修改后
FROM debian:trixie-slim AS runtime
```

---

## 执行结果

| 项目 | 状态 |
|---|---|
| Docker 构建 | 成功（exit_code: 0，耗时 ~34 秒，利用缓存） |
| 容器状态 | healthy，正常运行 |
| Gateway | 监听 0.0.0.0:42617 |
| 配对码 | 已生成 |

---

## 修改的文件

- `Dockerfile.debian`（第 68 行、第 69-77 行、第 96 行）
