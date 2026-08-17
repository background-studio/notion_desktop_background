# Notion Background Studio

[![org](https://img.shields.io/badge/org-background--studio-0ea5e9)](https://github.com/background-studio)
[![release](https://img.shields.io/github/v/release/background-studio/notion_desktop_background)](https://github.com/background-studio/notion_desktop_background/releases)

属于 [Background Studio](https://github.com/background-studio) 组织。可独立安装，也可作为
[Background Studio 壳](https://github.com/background-studio/background-studio) 的插件
（`--plugin` + `*-plugin.zip`，见 [docs/plugin-protocol.md](./docs/plugin-protocol.md)）。

一个面向 Windows 官方 Notion 桌面应用的独立背景管理器。它通过本机回环
Chromium DevTools Protocol 动态加载背景，不修改 `app.asar`、应用签名、
登录状态或页面数据。

管理器采用 Tauri 2、Rust 和系统 WebView2。

> 非 Notion 官方产品。Notion 及相关商标归其权利人所有。

## 功能

- 导入本地图片、视频或整个文件夹
- 下载 HTTP/HTTPS 网络图片和视频并纳入受管媒体库
- 图片覆盖、适应、拉伸和平铺
- 透明度、模糊、缩放、焦点位置、遮罩颜色与强度
- 侧栏、内容区、菜单不透明度控制
- 顺序或随机轮播，自定义切换间隔与播放列表
- 实时预览、热更新、系统托盘、Windows 自启动
- 一键暂停或完整恢复官方外观
- 作为壳插件时，后台等待官方 Notion 启动并自动接管

支持的图片格式：PNG、JPEG、WebP、GIF、AVIF。

支持的视频容器：MP4、WebM、Ogg Video、QuickTime MOV。

## 开发

要求 Node.js 22 或更高版本、Rust stable、Visual Studio Build Tools 的
“使用 C++ 的桌面开发”工作负载，以及已安装的官方 Notion 桌面应用
（默认路径 `%LOCALAPPDATA%\Programs\Notion\Notion.exe`）。
Windows 10/11 还需 WebView2 Runtime。

```powershell
npm install
npm run check
npm run dev
```

只预览界面：

```powershell
npm run dev:web
```

构建 Windows 安装包：

```powershell
npm run package:win
```

NSIS 产物位于 `src-tauri/target/release/bundle/nsis/`。

## 发布

推送与应用版本一致的 `v*` 标签会触发 GitHub Actions，在 Windows runner 上执行
完整检查、构建 NSIS 安装包，并创建正式 GitHub Release：

```powershell
git tag v0.2.3
git push origin v0.2.3
```

工作流会核对 `package.json`、`src-tauri/Cargo.toml`、`tauri.conf.json` 和标签版本，
任一不一致都会停止发布。Release 同时上传 NSIS 与
`NotionBackgroundStudio-<version>-plugin.zip`。

维护 Notion 页面样式、CDP 注入或媒体流程前，请先阅读项目 Skill：
[`notion-background-development`](./.cursor/skills/notion-background-development/SKILL.md)。

## 插件模式

以 `--plugin` 启动时作为 Background Studio 壳的后台托管进程：

- 不创建托盘，也不写本应用的 Windows 自启动项
- 启用后不会自动打开 Notion，只等待用户照常启动官方客户端
- 识别到新的普通官方进程后，按完整可执行路径确认，关闭并以本机调试参数重启，然后自动注入上次背景
- 插件启动前已经在运行的普通进程不会被自动关闭；状态会提示通过壳的「立即接管 / 应用」手动重启
- 已有有效调试会话会直接重连
- 目标退出或调试会话失联后会清掉失效会话；进程还在则等待手动接管，进程退了则重新等待
- 带调试参数但 45 秒内端口未就绪时不会强杀，会提示错误并等进程退出
- 暂停或恢复官方外观会立即暂停本次插件进程内的自动接管；手动应用后重新武装
- 停用由壳结束插件进程，不改动当前 Notion

## 安全边界

- 仅连接 `127.0.0.1` 回环调试口
- 仅向 `https://app.notion.com/` / `https://www.notion.so/` 页面注入
- 不修改 Notion 安装资源；暂停和恢复必须能完整移除注入
- 应用背景时如 Notion 未开启调试口，需要重启一次 Notion

## 版本

当前版本：`0.2.3`。
