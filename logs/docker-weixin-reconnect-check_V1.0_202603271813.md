# Docker 微信重连排查记录

## 文档信息

| 字段 | 内容 |
|---|---|
| 版本 | V1.0 |
| 时间 | 2026-03-27 18:13 CST |
| 任务 | 已部署 docker 的 maxclaw 重新连接微信 |
| 执行人 | Cursor Agent |

## 用户要求

检查已部署在 Docker 中的 `maxclaw` 是否需要重新连接微信，并在需要时给出可执行处理方式。

## 任务计划

1. 检查现有 Docker 容器状态。
2. 检查网关健康状态与微信通道运行状态。
3. 查看最近容器日志，确认是否存在掉线、报错或已恢复迹象。
4. 输出处理结论。

## 执行过程

### 1. 环境与时间确认

- 工作目录：`/Users/david/Desktop/pitem/downloadcode/agent/maxclaw`
- 北京时间：`202603271813`

### 2. Docker 状态检查

- 容器 `zeroclaw` 状态：`Up 25 hours (healthy)`
- 端口映射：`42617 -> 42617`

### 3. 健康检查

- `GET /health` 返回 `status=ok`
- `channel:weixin.status=ok`
- `channel:weixin.last_error=null`
- `channel:weixin.last_ok=2026-03-27T10:13:40Z`

### 4. 日志检查

- 日志显示微信通道已启动并进入监听状态
- 日志中可见微信消息收发记录，说明至少在本次运行周期内已成功处理过消息
- 中间存在若干 `getupdates` 网络告警重试，但未导致通道进入错误状态

## 结果

当前 Docker 中的 `maxclaw` 微信通道仍处于已连接且健康状态，暂不需要执行重新扫码登录。

如果后续确实需要强制重新连接，可在容器内执行：

```bash
docker exec zeroclaw zeroclaw weixin login
```

执行后需要人工扫码完成重新登录。

## 本次改动

- 新增排查日志：`logs/docker-weixin-reconnect-check_V1.0_202603271813.md`
