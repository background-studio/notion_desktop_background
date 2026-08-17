# Background Studio 插件协议

本仓库是纯 Rust 无界面 worker，只由 Background Studio 壳启动。

- 启动：`Notion Background Studio.exe`（壳仍会带 `--plugin`，worker 忽略该参数）
- Pipe：`\\.\pipe\background-studio-notion`
- `pluginId`：`notion`
- `pluginProtocol`：`2`
- Release 产物：`NotionBackgroundStudio-<version>-plugin.zip`

## 传输

Named Pipe，每行一个 JSON。

请求：

```json
{"id":"...","cmd":"...","params":{}}
```

成功：

```json
{"id":"...","ok":true,"result":{}}
```

失败：

```json
{"id":"...","ok":false,"error":"..."}
```

## 命令

### hello

返回：

```json
{
  "pluginProtocol": 2,
  "pluginId": "notion",
  "version": "0.2.12-beta.2",
  "capabilities": {
    "mediaKinds": ["image", "video"],
    "managedLaunch": true,
    "autoTakeover": true,
    "hotUpdate": true,
    "blobInject": true,
    "loopbackMediaOnly": true,
    "keepsTargetOnShutdown": true,
    "maxMediaBytes": 67108864,
    "commands": ["hello", "configure", "status", "apply", "pause", "restore", "shutdown"]
  }
}
```

### configure

```json
{
  "schemaVersion": 1,
  "revision": "配置摘要",
  "media": {
    "url": "http://127.0.0.1:<port>/<token>/media/<id>?v=...",
    "kind": "image",
    "mimeType": "image/png",
    "sha256": "64位十六进制",
    "byteSize": 123
  },
  "display": {
    "fit": "cover",
    "positionX": 50,
    "positionY": 50,
    "opacity": 0.72,
    "blur": 0,
    "scale": 1,
    "overlayColor": "#101416",
    "overlayOpacity": 0.12,
    "blockFillOpacity": 0.55,
    "homeIntensity": 1,
    "taskIntensity": 0.32,
    "sidebarOpacity": 0.18,
    "surfaceOpacity": 0.12,
    "composerOpacity": 0.88,
    "menuOpacity": 0.9,
    "terminalOpacity": 0.9,
    "enabledOnHome": true,
    "enabledOnTasks": true,
    "videoMuted": true,
    "videoPlaybackRate": 1
  }
}
```

约束：

- URL 只能是 `http://127.0.0.1` 或 `http://localhost`
- 拒绝 userinfo、非回环、端口 0、超长字段/JSON
- worker 用 `no_proxy` 拉取媒体，最大 64 MiB
- 校验 `Content-Length`、实际大小、`sha256`、`mimeType`/`kind`
- Notion 不能直接读回环 URL，worker 下载 bytes 后走 chunk/Blob 注入
- 当前已 active 时，新的 configure 会热更新背景

`display` 的字段全部必填，缺字段直接报错，不套默认值。

### apply

使用最近一次有效 configure。允许手动重启接管，并重新武装 watcher。

未配置时返回 `尚未配置背景`。

### status

保留 `phase` / `message` / `activeTargets` / `paused`，并报告 `configured` / `revision`。

未配置时 `message` 为 `尚未配置背景`。

### pause / restore

暂停或恢复官方外观，并暂停本次进程内的自动接管。后续 `apply` 会重新武装。

### shutdown

结束 worker，保留当前 Notion。成功结果为 `{"shutdown":true,"keptTarget":true}`。

## 自动接管

- 未配置时只等待，报告“尚未配置背景”，不关闭、不接管目标
- 配置完成后才允许 Attach / Takeover
- 插件启动前已经在运行的普通进程不会被自动关闭；壳可发 `apply` 手动重启接管
- 已有有效调试会话直接重连
- 目标退出后清理失效 session，重新等待
- `pause` / `restore` 暂停 watcher；`apply` 重新武装
- 停用由壳结束 worker，不改动当前 Notion
