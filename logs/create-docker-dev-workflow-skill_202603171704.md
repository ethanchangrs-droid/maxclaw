# 任务日志：创建 Docker Dev Workflow Skill

**版本：** V1.0_202603171704
**日期：** 2026-03-17 17:04 (UTC+8)

---

## 用户要求

参照文档 `MaxClaw_Docker_Dev_Workflow_V1.0_202603171601.md`，将第四步开始的内容创建为当前项目可用的 Cursor Skill。

## 任务计划

1. 阅读源文档，提取第四步（开发循环）至第六步（发布）以及附录（加速技巧、常用命令、配置参考）的内容
2. 参照 Cursor Skill 创建规范（`~/.cursor/skills-cursor/create-skill/SKILL.md`），设计 skill 结构
3. 在 `.cursor/skills/docker-dev-workflow/` 下创建 SKILL.md 和参考文档
4. 记录任务日志

## 执行过程

### 1. 分析源文档

源文档第四步开始包含以下内容：
- 第四步：开发循环 — `docker compose up -d --build`，首次构建耗时说明，增量编译缓存
- 第五步：调试验证 — 查看日志、进入容器、检查状态、健康检查
- 第六步：打标签并发布 — 生产镜像构建、标签、推送
- 加速技巧：多阶段构建 + Cargo 缓存机制说明
- 常用命令速查：日常构建、启动、停止、日志、进入容器、清理
- 关键端口与配置：Gateway 端口、CPU/内存限制、数据持久化

### 2. Skill 设计

- **名称：** `docker-dev-workflow`
- **位置：** `.cursor/skills/docker-dev-workflow/`（项目级 skill）
- **结构：**
  ```
  docker-dev-workflow/
  ├── SKILL.md              # 主指令文件
  └── references/
      └── commands.md       # 命令速查参考
  ```

### 3. 创建文件

**SKILL.md** 包含以下章节：
- Pre-flight Checks — 会话级环境验证
- Development Loop — 日常开发构建循环
- Debugging — 日志查看、容器调试、健康检查
- Release — 生产镜像构建与发布流程
- Other Operations — 启动、停止、清理等辅助操作
- Configuration Reference — 端口/资源/持久化配置表
- Troubleshooting — 常见问题排查表

**references/commands.md** 包含按场景分类的命令速查。

## 结果

- 创建文件：`.cursor/skills/docker-dev-workflow/SKILL.md`（约 100 行，符合 500 行上限要求）
- 创建文件：`.cursor/skills/docker-dev-workflow/references/commands.md`
- Skill 遵循 Cursor Skill 创建规范：YAML frontmatter、第三人称描述、触发关键词、渐进式信息披露
- Description 包含 WHAT（功能）和 WHEN（触发场景）
