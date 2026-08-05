---
name: notion-background-development
description: Maintains Notion Background Studio, including CDP injection into Notion desktop, page and tab-bar transparency, block-fill tinting, media/slideshow behavior, debugging, packaging, and release verification. Use when changing this repository or investigating a Notion desktop UI surface that does not follow background settings.
---

# Notion Background Studio 开发

## 目标

维护 Windows 官方 Notion 桌面应用的可逆背景工具。通过本机 CDP 注入样式和媒体，
不修改 `app.asar`、应用签名、登录状态或页面数据。

动手前按需读取：

- 页面入口、DOM 特征、透明度归属：[windows-and-selectors.md](windows-and-selectors.md)
- 架构、故障经验、安全边界：[architecture-and-pitfalls.md](architecture-and-pitfalls.md)

## 不可破坏的原则

1. 不直接修改 Notion 安装资源；暂停和恢复必须能完整移除注入。
2. 不凭截图猜选择器。通过 Notion CDP 检查实际 DOM、计算样式和 StyleX 变量。
3. 一个视觉区域只保留一层有色背景。内部壳层透明，避免半透明叠加变暗。
4. 所有界面不透明度必须允许 `0`；不要添加最低 0.2 之类的兜底。
5. 不用宽泛选择器清空所有背景。先确认稳定入口、尺寸、层级和页面复用范围。
6. `backdrop-filter` 默认关闭；它会让相同数值的区域看起来深浅不同。
7. 动态组件必须首帧生效。不要依赖可见延迟的 MutationObserver 补丁来消除闪烁。
8. 一次性 CDP 探查脚本放在 `poc/`，验证完立即删除。
9. 修改 `BACKGROUND_CSS` 时必须进入修订哈希，确保现有会话热更新。
10. 更改共享设置时同步修改 contracts、默认值、规范化、UI、payload 和测试。
11. 壳层雾度用 `color-mix(... calc(opacity * 28%), transparent)`（侧栏实色钮约 `* 40%`），
    避免滑杆到高值变成实心黑罩。

## 代码入口

- `src-tauri/src/lib.rs`：Tauri/Rust 主后端、命令、轮播和共享状态。
- `src-tauri/src/host.rs`：托盘、窗口生命周期、退出恢复和 Windows 自启动。
- `src-tauri/src/controller.rs`：发现官方 `Notion.exe`、校验进程、启动 Notion、保存和恢复 CDP 会话。
- `src-tauri/src/injector.rs`：Rust CDP target 同步、早期脚本、运行时更新、暂停和移除。
- `src-tauri/src/media.rs`、`network.rs`、`preview.rs`、`settings.rs`：媒体、安全下载、预览和事务设置。
- `src-tauri/build.rs`、`src-tauri/src/payload.rs`：从 TypeScript 提取共享 payload 并生成 Rust 资源。
- `src/main/payload.ts`：Notion 页面背景层、CSS 变量、选择器、色块标记和清理逻辑。
- `src/shared/contracts.ts`、`src-tauri/src/models.rs`：前端和 Rust 对应的数据契约。
- `src/renderer/bridge.ts`：Tauri `invoke` 主桥。
- `src/renderer/App.tsx`：Studio 操作界面和设置控件。
- `src/main/*`：历史 Electron 实现；当前构建以 Tauri 为准。

## 标准开发流程

### 1. 建立基线

先看 Git 状态和当前版本，不覆盖用户已有改动。确认 Notion 和 Studio 是否已有运行实例。

```powershell
git status --short --branch
npm run check
```

开发要求 Node.js 22+、Rust stable 和 MSVC C++ Build Tools。JavaScript 使用 npm，
Rust 使用 Cargo。Vite 开发地址默认 `http://127.0.0.1:5174/`。

### 2. 复现并确定视觉层

先判断问题属于哪一类：

- 左侧边栏 / 「新对话」等侧栏实色钮：归 `sidebarOpacity`。
- 顶栏、主画布、数据库表头、AI 浮钮：归 `surfaceOpacity`。
- 弹出菜单 / 对话框：归 `menuOpacity`。
- 折叠块 / 高亮块色底：归 `blockFillOpacity`。
- 整页压在背景上的底色：归 `overlayColor` + `overlayOpacity`（Studio「画面 → 底色」）。
- 背景媒体本身：归 `opacity`、页面/空白页强度。

不要用子层再画一层相同透明色。应让外层统一打底、内部壳透明。

### 3. 用 CDP 检查 Notion

运行时端口记录在 `%LOCALAPPDATA%/NotionBackgroundStudio/runtime.json`。只连接：

- `127.0.0.1` 回环地址；
- browser ID 与状态文件一致的实例；
- page target 为 `https://app.notion.com/...`、`https://www.notion.so/...`，
  或标签栏 `file://.../renderer/tabs/index.html`。

探查内容至少包括：

- 元素标签、id、role、完整 class；
- `getBoundingClientRect()`；
- `backgroundColor`、`backgroundImage`、`boxShadow`、`backdropFilter`；
- 内联 `style` 里的 StyleX / Notion CSS 变量（如 `--c-bacPri`、`--c-greBacSec`）；
- `::before`、`::after`；
- 元素祖先链。

截图验证前后状态。不要把探查脚本或截图提交进仓库。

### 4. 选择实现方式

- 普通 DOM：在 `BACKGROUND_CSS` 增加精确规则。
- StyleX 实色钮：除 `background` 外一并压掉 `--c-bacPri` / `--c-bacInt`；侧栏用
  `data-cbg-solid-chrome` 标记实色按钮。
- 色块底：捕获 `--c-xxxBacSec` 原色到 `--cbg-captured-fill`，用
  `[style*="--cbg-captured-fill"]` 做开关（React 会冲掉 `data-*`）。
- 标签栏：独立 page，用 `notion-background-tab-chrome`；媒体高度按 `outerHeight`
  对齐，主页 `top: -(outerHeight - innerHeight)`（约 `-36px`）。
- 封面图：宽图靠近顶部时打 `data-cbg-cover` 并半透明显示（勿 `visibility:hidden`）。
- Agent 侧栏：`.chat_sidebar .test123` 等 `--c-bacPri` 实底层跟随 surface。
- Agent 输入框：`.chat_sidebar` 内 `--c-bacSec` 圆角卡片，需单独覆盖。
- 页面图标：清掉 `.notion-record-icon img` 的 `--c-bacPri` 占位黑底。

### 5. 保证首帧和可恢复性

`earlyPayloadFor()` 必须在 `documentElement` 出现时即可运行。

动态更新使用 `requestAnimationFrame` 合并到下一帧绘制前。

新增任何注入对象时，同时补齐 `cleanup()`：

- 移除 style、layer；
- 断开 observer、timer；
- 撤销 Blob URL；
- 删除根 class、CSS 变量、色块捕获变量和侧栏标记。

### 6. 验证

```powershell
npm run check
```

随后使用真实 Notion 验证：

1. 普通页面与侧栏。
2. 数据库列表视图（表头、行、新建）。
3. 带色块底的折叠块 / 高亮块。
4. 顶部 Electron 标签栏与主内容是否拼成一张连续背景。
5. 弹出菜单 / 对话框。
6. 深色、浅色主题。
7. 界面透明度为 `0`、中间值和 `1`；色块底色同测。
8. 导航、切换标签、重载时无黑底闪烁。
9. 暂停、恢复官方外观后不残留样式。

### 7. 版本和打包

当前首版目标 `0.1.0`。补丁修复递增 patch；功能或设置结构变化再考虑 minor。
同步修改 `package.json` 与 lock 根版本。

```powershell
npm run check
npm run package:win
```

安装包位于 `src-tauri/target/release/bundle/nsis/`。

### 8. 提交和传输

提交前检查完整 diff、测试结果、版本和临时文件。只在用户明确要求时提交或推送。
提交信息说明为什么改，而不是只罗列文件。

## 完成条件

- 目标页面视觉结果符合对应透明度控制；
- 没有透明度叠加、原生阴影或首帧闪烁；
- 标签栏与主页背景对齐连续；
- 真实 Notion 验证完成；
- `npm run check` 全部通过；
- 临时探查文件已删除；
- 恢复流程仍完整可逆。
