# 架构与故障经验

## CDP 注入链路

完整流程：

1. `src-tauri/src/controller.rs` 在候选路径查找官方 `Notion.exe`：
   - `%LOCALAPPDATA%\Programs\Notion\Notion.exe`
   - `%ProgramFiles%\Notion\Notion.exe`
2. 校验可执行路径落在安装根下，并读取版本。
3. 若已有未启用 CDP 的 Notion，要求用户确认重启。
4. 直接启动 `Notion.exe`，参数只绑定 `127.0.0.1`。
5. 首选调试口 **9226**（占用则向后试至 +100），等待 `/json/version`。
6. 保存 port、browser ID、安装标识和 executable 到 `runtime.json`。
7. `src-tauri/src/injector.rs` 只接受同一 browser ID、回环 WebSocket，以及：
   - `https://app.notion.com/...` / `https://www.notion.so/...`
   - 标签栏 `file://.../tabs/index.html`
8. 为每个 target 开启 Runtime/Page、临时 bypass CSP。
9. `Page.addScriptToEvaluateOnNewDocument` 注册早期 payload。
10. `Runtime.evaluate` 立即应用当前页面。
11. 定期同步新 target；导航和重载由早期脚本覆盖。

安全边界不要放松：

- 不连接任意调试端口。
- 不接受非回环 WebSocket。
- 不向非白名单 URL 注入。
- 不按进程名粗暴结束所有 `Notion.exe`；必须比较官方安装路径的完整路径。

## Payload 生命周期

`payload.ts` 生成自包含 IIFE，状态保存在：

```text
window.__NOTION_BACKGROUND_STUDIO__
```

主要对象：

- `#notion-background-style`：普通 DOM 样式。
- `#notion-background-layer`：固定背景媒体层。
- `#notion-background-media`：图片或视频。
- `#notion-background-tile`：平铺模式。
- `#notion-background-overlay`：整页底色（Studio「底色」）。

重应用前先调用旧状态的 `cleanup()`，避免 observer/timer 重复、Blob URL 泄漏、
style/layer 重复，以及旧 revision 阻止新 CSS 生效。

修订号同时混入媒体 sha256、display 设置、媒体 kind、`BACKGROUND_CSS`、
以及占位的 `REVIEW_SHADOW_CSS`。

`build.rs` 从 `src/main/payload.ts` 提取 `BACKGROUND_CSS`、`REVIEW_SHADOW_CSS`
和 runtime 模板；改 payload 后需触发 Cargo 重编（`tauri dev` 监听 Rust 侧变更，
必要时 touch `src-tauri/src/lib.rs`）。

## 为什么媒体使用 base64 + Blob URL

Notion 页面不能稳定读取 Studio 回环 HTTP 媒体 URL。Rust 后端读取受管媒体、
限制内嵌大小（64 MB）、生成 data URL，payload 内解码为 Blob，再交给
`<img>` / `<video>`；cleanup 时 revoke。媒体加载失败时必须整体 cleanup。

## 媒体库与动态源

数据目录：

```text
%LOCALAPPDATA%/NotionBackgroundStudio
```

主要文件：`settings.json`、`library.json`、`runtime.json`、`media/`、`temporary/`。

- `origin: "api"`：保存随机 API 地址，轮播/刷新时重新拉取。
- `origin: "folder"`：只保存目录路径，应用时再挑选文件，不复制入库。
- 不覆盖当前媒体文件；新内容用 `<id>-<hash-prefix>.<ext>`，再删旧文件，避免 Windows 文件锁。

## 网络安全

远程媒体只允许无账号信息的 HTTP/HTTPS。每次请求和重定向都要校验 URL、
DNS 解析结果，拒绝 loopback / 私网等；限制体积、边长、重定向次数和类型。

## 设置扩展流程

新增显示设置时同时修改：

1. `src/shared/contracts.ts`（`DisplaySettings` + `DEFAULT_SETTINGS`）
2. `src-tauri/src/models.rs`（结构、默认值、patch、normalize）
3. `src/renderer/App.tsx`（控件、标签、预览变量）
4. `src/main/payload.ts`（`ROOT_PROPERTIES`、`setProp`、CSS）
5. 相关测试

透明度设置统一 clamp 到 0..1。不要让 UI 最小值和 normalize 最小值不一致。

## 已解决的典型故障

### 鼠标移动时背景图抖动

原因：`MutationObserver` 监听了整棵子树的 `class` 变化。Notion 悬停时会大量
切换 class，每次都 `install()`，重写背景 `top`/`height`，并对封面图做
`getBoundingClientRect()`，造成布局抖动。

处理：属性变化只响应 `documentElement` / `body`（主题等）；`childList` 才做
完整 heavy install。背景几何样式只在数值变化时写入；resize 走轻量路径。
去掉 `visualViewport` 的 `scroll` 监听。

### 标签栏与主页背景错位 / 只有一半有图

原因：标签栏是独立 Electron page，高度约 36px。旧逻辑用
`outerHeight - innerHeight` 做主页上移量；在 Windows 还原窗口时，
`outerHeight` 还包含系统边框（常见再多 ~100px），接缝就会错位。最大化时
边框变薄，delta 又接近 36，所以表现为「有时候」才出现。

处理：媒体高度仍统一用两边相同的 `outerHeight`（保证 `object-fit: cover`
裁切一致）；标签栏 `top: 0`；主页 `top: -tabChromeHeight`。
`tabChromeHeight` 仅在 `outer-inner` ≤ 48px 时采信，否则回退到 Notion
标签栏实测高度 36px。并监听 `resize` / `visualViewport` 以便最大化还原后立即重算。

### 滑杆到最低仍有黑边

常见残留：

- `.notion-table-view-header-row` 实心底 + 黑色 `box-shadow`
- 侧栏「新对话」`background: var(--c-bacPri)`（只改 `background-color` 不够）
- 整页 `#notion-background-overlay` 强度未归零
- Studio 旧 payload 把热补丁冲掉（改 CSS 后必须重编并「应用」）

### 色块底标记丢失

原因：React 协调会冲掉 `data-*`，但常保留已写入 style 的自定义属性。

处理：用 `--cbg-captured-fill` 是否存在做开关，不要只靠 `data-cbg-block-fill`。

### 壳层滑杆一高就变实心黑罩

原因：`color-mix(... opacity * 100% ...)` 在高值几乎不透明。

处理：侧栏 / 顶栏 / 表头用约 `* 28%` 雾度；侧栏实色钮约 `* 40%`。

### Studio 重建后背景消失

原因：`InjectorEngine` Drop / `stop()` 会移除注入；静默重连可能失败。

处理：启动时按 `runtime.json` 校验 browser ID 并恢复；失败打日志，引导用户再点「应用」。
改 payload 后务必确认注入 revision 已更新。

### 页面冻结（MutationObserver 环）

原因：`install()` 无条件改写 style `textContent`。

防护：`data-cbg-revision`；值不同才写；CSS 变量值不同才 `setProperty`；
DOM 更新按 `requestAnimationFrame` 合并。

### 深色主题出现浅色界面

原因：错误 fallback 到浅色 surface。

处理：从根 class / data theme / `prefers-color-scheme` 检测，设置
`--cbg-surface-color`。

### 透明化后正文 emoji 消失

原因：`img.notion-emoji` 的图案在 CSS 雪碧图上，`src` 只是 1×1 GIF。
`.notion-record-icon img { background: transparent }` 简写会清掉
`background-image`。大号页面图标还会停在 Notion 的 `opacity: 0` 淡入。

处理：只清非 emoji 图标的 `background-color`；`img.notion-emoji` 强制
`opacity: 1`，不要动它的 `background`。

### 灰色标注块看起来只剩一圈阴影

原因：标注块是圆角小卡片（`--c-graBacSec`）。色块底被 color-mix 变淡、
左边 emoji 被冲掉后，灰底像一块浮着的淡影；自带的 `border: 1px solid
transparent` 也没有跟着滑杆走。

处理：色块底、边框颜色走同一套 `--cbg-block-fill-opacity`，并去掉残留
`box-shadow`，让整张卡片一起淡。

### 过夜后背景在两张图之间来回闪

原因：payload 在 `await decode` **之前**抓取 `previous = window[STATE]` 快照。
页面被 Chromium 冻结（过夜/后台）时 decode 卡住，Rust 侧 evaluate 超时把
会话标坏并重连重发，于是多份注入脚本并发执行且抓到同一个旧快照。先醒的
装好并覆盖 `window[STATE]`；后醒的清理的还是旧快照，清不到前一份的
observer 和 4s 定时器——留下「孤儿安装器」每 4s 把背景改回自己那张图，
和最新一轮互抢 `#notion-background-media` 的 src，即双图闪烁。孤儿还会把
`style.dataset.cbgRevision` 改回旧值，插件以为没装上，每 1.2s 整包重灌，
孤儿越积越多。

处理：全局自增 `__NOTION_BACKGROUND_RUN_SEQ__` 运行序号。每轮开头领号；
decode 醒来后核对，已有更新轮次则撤销自己的 Blob 并放弃（不碰画面）；
`install()` 开头同样核对，遗留回调一律 no-op；清理改为读**当下**的
`window[STATE]` 而非进门快照；`REMOVE_RENDERER_PAYLOAD` 先推进序号再清理，
避免移除后在途轮次又把背景装回来。

### 换图后只有标题栏更新，正文要切页面才变

原因：Notion 会留一个 0×0 的 `/blank` 恢复页。那页上 `img.decode()` 即使图片
已经 `complete` 也可能永不返回。旧注入把所有 target 排成一队，这页卡住后正文
换不了图；切页面会拆掉恢复页，看起来像「切一下才生效」。

处理：没有可视面积的页跳过注入；各页并行换图；`decode()` 加超时，图片已经
加载完就继续。

## 调试策略

### 推荐

- 先 `npm run dev` 调试 Tauri Studio（Vite `5174`）。
- 只看 Studio UI 时用 `npm run dev:web`。
- 用一次性 `poc/*.py`（`uv run --with websockets`）连接 Notion CDP。
- 把 DOM、计算样式和截图作为证据。
- 对动态页面测试导航后、切换视图后、重载后状态。

### 避免

- 不用类名关键词大范围 `background: transparent`。
- 不根据截图颜色猜元素。
- 不依赖 StyleX 随机 hash 类名做唯一选择器。
- 不用整块 `opacity` 淡化色块（会连文字一起淡）。
- 不把探查脚本留在仓库里。

## 测试和发布

`npm run check` 包含 TypeScript 检查与 Cargo 测试。

payload 相关断言至少应覆盖：

- 关键稳定选择器仍存在；
- 标签栏 / 色块 / 表头规则存在；
- 修订哈希会因 CSS 变化而变；
- 不引入 backdrop blur 作为默认。

发布前：

1. 删除 `poc/` 一次性文件。
2. 跑 `npm run check`。
3. 在真实 Notion 完成页面矩阵验证。
4. 更新 package 与 lock 版本。
5. 跑 `npm run package:win`。
6. 查看 Git 完整 diff。
7. 用户明确同意后提交、推送。
