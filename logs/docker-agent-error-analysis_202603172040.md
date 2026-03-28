# Docker Agent 报错分析：Agent exceeded maximum tool iterations (10)

> 分析时间：2026-03-17 20:40 (UTC+8)

---

## 用户要求

用户在 Docker 中运行 ZeroClaw Agent 时，发送消息"编写一个python程序，找到1000到1500之间的质数，把结果写到一个结果文件中，并执行这个程序。然后把程序和结果文件用邮件发给我。"，Agent 返回错误：`[Error] Agent exceeded maximum tool iterations (10)`。

## 分析过程

### 1. Docker 容器状态

- 容器名称：`zeroclaw`
- 状态：`Up 2 hours (healthy)`
- 端口映射：`0.0.0.0:42617->42617/tcp`
- 容器本身运行正常，没有崩溃或重启

### 2. Docker 日志分析

通过 `docker compose logs --tail=300 zeroclaw` 获取完整日志，Agent 的行为轨迹如下：

| 迭代次数 | Agent 尝试的操作 | 结果 |
|---|---|---|
| 1 | 创建 Python 程序文件，尝试执行 | 被安全策略阻止 |
| 2 | 尝试使用 `python` 命令 | 被安全策略阻止 |
| 3 | 尝试使用"批准方式"执行 | 被安全策略阻止 |
| 4 | 检查 Python 是否可用 | 被安全策略阻止 |
| 5 | 再次编写程序并尝试执行 | 被安全策略阻止 |
| 6 | 尝试使用 shell 命令 | 被安全策略阻止 |
| 7 | 检查结果文件是否已创建 | 可能成功，但文件未生成 |
| 8 | 尝试使用 `cron_add` 执行 | 被安全策略阻止 |
| 9 | 手动创建结果文件 + 尝试 schedule | 被安全策略阻止 |
| 10 | 尝试多种 schedule 格式 | 被安全策略阻止，迭代耗尽 |

Agent 在 10 次工具调用后被强制终止，触发错误。

### 3. 根本原因分析

经过对 Docker 容器内配置文件和源代码的深入分析，确认有 **3 个根本原因**：

#### 原因一：Python 不在允许命令列表中（主因）

容器内配置文件 `/zeroclaw-data/.zeroclaw/config.toml` 中的安全策略：

```toml
[autonomy]
level = "supervised"
allowed_commands = [
    "git", "npm", "cargo", "ls", "cat", "grep",
    "find", "echo", "pwd", "wc", "head", "tail", "date",
]
```

`python`、`python3` 均不在允许列表中。Agent 每次尝试执行 Python 命令时，都被 `SecurityPolicy::is_command_allowed()` 拦截，返回 `"Command not allowed by security policy"`。

源代码 `src/security/policy.rs` 中的验证逻辑：
- `is_command_allowed()` 将命令拆分为子段，逐段检查是否在 `allowed_commands` 列表中
- 不在列表中的命令直接被拒绝
- 单元测试也明确验证了 `python3 exploit.py` 会被阻止

#### 原因二：max_tool_iterations = 10 偏低

配置文件中：

```toml
[agent]
max_tool_iterations = 10
```

源代码 `src/agent/loop_.rs` 中的逻辑：
- 默认值为 `DEFAULT_MAX_TOOL_ITERATIONS = 10`
- 值为 0 时也回退到默认值 10
- Agent 循环 `for iteration in 0..max_iterations`，超过后触发 `anyhow::bail!("Agent exceeded maximum tool iterations ({max_iterations})")`

对于复杂任务（写代码 + 执行 + 发邮件），10 次迭代可能不够。但本案例中即使增加迭代次数也无法解决问题，因为命令本身被禁止。

#### 原因三：邮件功能未配置

用户要求"用邮件发给我"，但容器配置中没有启用任何邮件发送功能：
- `browser.enabled = false`
- `http_request.enabled = false`
- 没有邮件相关的 tool 或 channel 配置
- Agent 即使执行了 Python 程序，也无法完成邮件发送任务

### 4. 因果链总结

```
用户发送复杂任务（写Python + 执行 + 发邮件）
  ↓
Agent 尝试 shell 执行 python 命令
  ↓
SecurityPolicy.is_command_allowed("python3 ...") → false
  ↓
Agent 反复尝试替代方案（cron, schedule 等）
  ↓
所有替代方案同样被安全策略阻止
  ↓
10 次迭代耗尽
  ↓
触发 "Agent exceeded maximum tool iterations (10)"
```

## 解决方案

### 方案一：将 Python 加入允许命令列表（推荐）

编辑容器内配置或宿主机配置文件，在 `allowed_commands` 中添加 `python3`：

```toml
[autonomy]
allowed_commands = [
    "git", "npm", "cargo", "ls", "cat", "grep",
    "find", "echo", "pwd", "wc", "head", "tail", "date",
    "python3", "python",
]
```

注意：还需要确保 Docker 容器内安装了 Python。当前使用的镜像可能没有 Python 环境。

### 方案二：增加 max_tool_iterations

对于需要多步操作的复杂任务，将迭代限制提高：

```toml
[agent]
max_tool_iterations = 20
```

### 方案三：配置邮件功能

如果需要邮件发送功能，需要配置 `http_request` 或集成邮件服务。

### 方案四：降低自治等级限制

将 `require_approval_for_medium_risk` 设为 `false`，或将 `level` 改为 `"autonomous"`，减少安全限制。但需权衡安全风险。

## 结论

这不是一个程序 bug，而是 **安全策略配置限制** 导致的预期行为。Agent 正确地遵守了安全策略，拒绝执行不在允许列表中的命令。错误信息 "Agent exceeded maximum tool iterations" 是 Agent 在无法完成任务时的安全终止机制。核心修复是将 `python3` 加入 `allowed_commands` 列表。
