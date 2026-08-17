# Notion Background Studio

[![org](https://img.shields.io/badge/org-background--studio-0ea5e9)](https://github.com/background-studio)
[![release](https://img.shields.io/github/v/release/background-studio/notion_desktop_background)](https://github.com/background-studio/notion_desktop_background/releases)

属于 [Background Studio](https://github.com/background-studio) 组织。本仓库是壳专用的纯 Rust 无界面 worker，不再提供独立安装器或本机 UI。

它通过本机回环 Chromium DevTools Protocol 动态加载背景，不修改 `app.asar`、应用签名、登录状态或页面数据。

> 非 Notion 官方产品。Notion 及相关商标归其权利人所有。

## 功能

- 由 Background Studio 壳配置媒体和显示参数
- 下载壳提供的回环媒体后，按 chunk/Blob 注入 Notion
- 图片覆盖、适应、拉伸和平铺
- 透明度、模糊、缩放、焦点位置、遮罩颜色与强度
- 侧栏、内容区、菜单、色块底不透明度
- 等待官方 Notion 启动并自动接管
- 一键暂停或完整恢复官方外观

支持的图片格式：PNG、JPEG、WebP、GIF、AVIF。

支持的视频容器：MP4、WebM、Ogg Video、QuickTime MOV。

## 开发

要求 Rust stable、Visual Studio Build Tools 的“使用 C++ 的桌面开发”工作负载，以及已安装的官方 Notion 桌面应用（默认路径 `%LOCALAPPDATA%\Programs\Notion\Notion.exe`）。

```powershell
cargo fmt --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml
cargo build --release --manifest-path src-tauri/Cargo.toml
```

产物是 `src-tauri/target/release/notion-background-studio.exe`。壳安装时使用的文件名仍是 `Notion Background Studio.exe`。

## 发布

推送与 `src-tauri/Cargo.toml` 版本一致的 `v*` 标签会触发 GitHub Actions：检查、`cargo test`、`cargo build --release`，然后只上传

`NotionBackgroundStudio-<version>-plugin.zip`

zip 内含 worker exe、`plugin.json`。不再生成 NSIS。

协议说明见 [docs/plugin-protocol.md](./docs/plugin-protocol.md)。

维护 Notion 页面样式、CDP 注入或媒体流程前，请先阅读项目 Skill：
[`notion-background-development`](./.cursor/skills/notion-background-development/SKILL.md)。

## 安全边界

- 仅连接 `127.0.0.1` 回环调试口
- 仅向 `https://app.notion.com/` / `https://www.notion.so/` 页面注入
- configure 只接受壳的回环媒体 URL，并校验大小与哈希
- 未配置时不接管、不关闭 Notion
- 不修改 Notion 安装资源；暂停和恢复必须能完整移除注入
