# Background Studio 插件协议

见组织规范：与 Codex 共用 `pluginProtocol: 1`。

本插件：

- 启动：`Notion Background Studio.exe --plugin`
- Pipe：`\\.\pipe\background-studio-notion`
- `pluginId`：`notion`
- Release 产物：`NotionBackgroundStudio-<version>-plugin.zip`

完整协议说明见
[codex_desktop_background/docs/plugin-protocol.md](https://github.com/background-studio/codex_desktop_background/blob/main/docs/plugin-protocol.md)
或壳仓文档。

## 插件模式自动接管

协议版本仍是 `pluginProtocol: 1`，命令不变。`--plugin` 启动后会在后台监视官方 Notion，而不会自动打开它。

行为：

- 没有官方进程时等待用户照常启动
- 插件启动前已经存在的普通进程不会被自动关闭，等待壳通过现有 `apply` 手动重启接管
- 用户新启动的普通官方进程按完整可执行路径确认后，关闭并以本机调试参数重启，再自动注入上次背景
- 已有有效调试会话直接重连；带调试参数但端口尚未就绪时等待，不误杀
- 目标退出后清理失效 engine / runtime，重新等待
- `pause` / `restore` 立即暂停本次插件进程内的 watcher；`apply` 立即重新武装
- 带调试参数但端口 45 秒内未就绪时进入错误状态并等待进程退出，不会强杀该进程
- 已连接会话失联且目标仍在运行时，清掉失效会话并回到手动接管，不会继续显示 active
- 停用由壳结束插件进程，不改动当前 Notion

`status` 的 `message` 例如：

- `已启用，等待 Notion 启动`
- `Notion 已在运行，点立即接管可重启`
- `正在接管 Notion`
- `背景已自动应用`
- `暂停托管`
- `调试端口未能在 45 秒内就绪，等待 Notion 退出后重新接管`
- `请先从媒体库选择一张图片或一个视频。`

探测失败时 `phase` 为 `error`；下一次成功探测会恢复到当前等待/已有进程/暂停状态。

`paused` 在暂停托管或恢复官方外观后为 `true`。
