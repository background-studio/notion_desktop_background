# Notion 页面入口与选择器

Notion class 名和 StyleX hash 可能随版本变化。这里记录的是稳定入口和定位方法，
不是允许盲目复制的永久 API。每次 Notion 更新后都要用 CDP 复核。

## 全局窗口骨架

### Electron 顶部标签栏（含 Windows 最小化/最大化/关闭那一行）

- 独立 page：`file:///.../app.asar/.webpack/renderer/tabs/index.html`
- 典型高度约 `36px`；主内容在标签栏下方，二者 `innerHeight` 之和才是内容区总高。
- **不要**用主页的 `outerHeight - innerHeight` 当标签栏高度：Windows 还原窗口时
  该差值还含系统边框（常到 100px+），会导致标题行与正文背景错位。
- 根 class：`notion-background-tab-chrome`
- 处理：清 `.root` / `.hide-scrollbar` 及标签芯片实色底。
- 标签边缘的暗条是内联 `linear-gradient(var(--gradient-direction, ...))`，不是
  `box-shadow`；需在 Tab Bar 范围内同时清掉 `background-image`。
- 背景媒体：高度用两边相同的 `outerHeight`，`top: 0`，与主页共用同一张图。
- Notion 渲染页会拦截本机回环 HTTP 媒体请求，因此媒体仍需内嵌为 data URL。
  大 payload 只做一次实时 `evaluate`；early script 仅注册轻量透明样式，禁止重复发送整图。

### 主内容页

- URL：`https://app.notion.com/...` 或 `https://www.notion.so/...`
- 根 class：`notion-background-active`；空白恢复页额外走 `notion-background-task`
- 背景媒体：高度用 `outerHeight`，`top: -tabChromeHeight`（约 `-36px`；仅当
  `outer-inner ≤ 48` 时才采信该差值），与标签栏拼图。

### 左侧边栏

- 稳定入口：`.notion-sidebar`、`.notion-sidebar-container`、`nav.notion-sidebar-container`
- 透明度：`--cbg-sidebar-opacity`（雾度 `* 28%`）
- 内部不要再叠一层侧栏同色底。
- **收起后悬停弹出**：`.notion-sidebar` 内绝对定位底板子层用
  `background: var(--c-bacEle)`（常见实色 `#202020`）。只透明外层不够，
  必须在侧栏上下文压掉 `--c-bacEle`，并覆盖带该变量的子层背景。
- 底部「新对话」等实色钮：StyleX `background: var(--c-bacPri)`；
  标记 `data-cbg-solid-chrome="1"`，并压掉 `--c-bacPri` / `--c-bacInt`。

### 主画布与顶栏

- 稳定入口：`main.notion-frame`、`header`、`.notion-topbar`、`.notion-topbar-action-buttons`
- 透明度：`--cbg-surface-opacity`（雾度 `* 28%`）
- 内部内容区应透明：`.notion-page-content`、`.notion-scroller`、`.layout-content` 等。

### 封面图

- 主画布内靠近顶部的宽图：打 `data-cbg-cover="1"`，**允许显示**，
  按 `--cbg-surface-opacity` 做半透明（约 `0.45 + surface * 0.55`）。
- **禁止** `visibility:hidden` / `opacity:0` 藏封面：Notion 懒加载会停在
  1×1 GIF，封面和页面图标都会变成 `--c-bacPri` 黑块。
- 排除：`img.notion-emoji`、`.notion-record-icon img`（不要误标成封面）。

### 页面图标

- 大图标常是 `main.notion-frame .notion-record-icon img`，未加载时内联
  `background: var(--c-bacPri)` → 只清 `background-color`，选择器排除
  `img.notion-emoji`。
- 表格/侧栏/正文 emoji 走 `img.notion-emoji` + spritesheet `background-image`，
  不要写 `background: transparent` 简写，否则雪碧图被冲掉，只剩 1×1 GIF。

### Agent / AI 对话侧栏

- 外层：`.chat_sidebar.notion-print-ignore`（本身可能已透明）
- 实黑内层：`.chat_sidebar .test123` 以及带 `background: var(--c-bacPri)` 的子层
- 底部输入框：内联 `background-color: var(--c-bacSec)` + `box-shadow: var(--c-shaOutMd)`
  （圆角卡片，「使用 AI 处理各种任务…」），需单独覆盖 `--c-bacSec`，清阴影
- 跟随 `--cbg-surface-opacity`（雾度 `* 28%`），并覆盖 `--c-bacPri` / `--c-bacInt` / `--c-bacSec`

## 数据库 / Collection

### 表头与行

- 表头实底：`.notion-table-view-header-row`（常伴黑色 `box-shadow`，必须清掉）
- 增行 / 增列：`.notion-table-view-add-row`、`.notion-table-view-add-column`
- 单元格 / 行体：`.notion-table-view-cell`、`.notion-table-view-row`、
  `.notion-collection-view-body`、`.notion-collection-result-wrapper` → 透明
- 表头雾度跟随 `--cbg-surface-opacity`

### 行内快捷与 AI

- `.quickActionContainer`（如「打开」）
- `.notion-ai-button`
- 跟随 `--cbg-surface-opacity`（约 `* 40%`）

### 站点上线蓝条

- 文案类似「此页面已在 xxx.notion.site 上线」。
- 内联：`background-color: var(--c-bluBacSec)`。
- 选择器：`main.notion-frame [style*="--c-bluBacSec"]`
- 跟随 `--cbg-surface-opacity`（约 `* 55%`，保留淡蓝雾）。

### 数据库底部计数条

- 文案「计数 N」；容器 `.sticky-portal-target`（`bottom: 0`）。
- 实底常在子层 `.content-editable-void-no-select`（`background: var(--c-bacPri)`）
  以及带 `clip-path` 的包装层。
- 跟随 `--cbg-surface-opacity`；包装层直接透明。

## 色块底（折叠块 / 高亮块）

- 用户感知：绿「已完成」、红「未开始」、棕「进行中」等块底色。
- 内联样式特征：`background: var(--c-greBacSec)` / `--c-redBacSec` /
  `--c-yelBacSec` / `--c-graBacSec` 等。
- 实现：首次捕获原色写入 `--cbg-captured-fill`；CSS 用
  `.notion-page-content [style*="--cbg-captured-fill"]` +
  `color-mix(... var(--cbg-block-fill-opacity) ...)`。
- 不要标记 `--ca-xxxBacSecTra` 半透明描边层。
- 不要用整块 `opacity`，否则文字一起变淡。

## 弹出层

- `.notion-overlay-container [role="dialog"|"menu"|"listbox"]`
- `.notion-dropdown-menu`
- 透明度：`--cbg-menu-opacity`（菜单用 `* 100%`，需要时可单独收紧）

### 设置弹窗

- 稳定入口：`.notion-space-settings`（overlay 容器内的全屏 fixed 包装层）。
- 外层 `[role="dialog"].notion-dialog`（内联 `background: var(--c-bacEle)`）已被
  通用弹层规则按 `--cbg-menu-opacity` 打雾，无需另写。
- 内部两块实底需清透明：左侧导航 `[role="dialog"] > [role="presentation"] >
  div[style*="--c-bacSec"]`，右侧内容 `[role="tabpanel"] > div[style*="--c-bacPri"]`。
- 左栏子树里还有嵌套 `--c-bacSec` 实底：底部「购买 Notion AI」是 sticky 栏
  （`position: sticky; bottom: 0; background-color: var(--c-bacSec)`），
  用 `div[style*="--c-bacSec"] [style*="--c-bacSec"]` 一并清掉。
- `.notion-modal-underlay`（内联 `background: var(--ca-modUndBac)`，约 0.8 黑）盖住
  弹窗背后的壁纸。处理方式不是调透明度，而是在底罩上用 `--cbg-media-url` 重绘壁纸
  并叠 `(1 - opacity * route)` 表面雾 + 用户底色层，既盖住背后页面正文，又与主图层
  亮度一致。弹窗透明时看到的是干净壁纸，不会和底下页面文字叠字。
- `background-size` 不接受 `fill`/`tile`，重绘用单独映射的 `--cbg-bg-size`
  （fill → `100% 100%`，tile → `auto` + repeat）。
- 视频背景时 `background-image` 加载失败，底罩自然回落为表面色实底。

## 路由强度

| Studio 文案 | 设置字段 | 根 class | 用途 |
|---|---|---|---|
| 页面 | `homeIntensity` / `enabledOnHome` | `notion-background-home` | 正式 Notion 页 |
| 空白页 | `taskIntensity` / `enabledOnTasks` | `notion-background-task` | `/blank` 恢复页 |

层透明度：`opacity * routeIntensity`。空白页强度为 0 时只关掉 blank，不影响正式页。

## 透明度归属

- 背景媒体：`--cbg-opacity`
- 页面强度：`--cbg-home-intensity`
- 空白页强度：`--cbg-task-intensity`
- 左侧边栏：`--cbg-sidebar-opacity`
- 顶栏与页面：`--cbg-surface-opacity`
- 弹出菜单：`--cbg-menu-opacity`
- 色块底色：`--cbg-block-fill-opacity`
- 整页底色：`--cbg-overlay-color` + `--cbg-overlay-opacity`
- 壳层表面色：`--cbg-surface-color`（深/浅主题检测）

所有滑杆值范围均为 0 到 1（整页底色强度上限 0.9）。

Studio「界面层」文案应对齐 Notion：左侧边栏 / 顶栏与页面 / 弹出菜单 / 色块底色。
`composerOpacity`、`terminalOpacity` 为契约遗留字段，Notion UI 不展示；
未接线前不要假装它们控制 Notion 表面。

## 新页面定位步骤

1. 进入目标页面并截图。
2. 从异常区域中心用 `document.elementsFromPoint()` 获取元素栈。
3. 找到第一个非透明背景、渐变、shadow 或 backdrop-filter。
4. 看内联 `style` 是否走 `--c-*` / `--ca-*` 变量。
5. 沿祖先链确认是壳层、色块还是控件。
6. 检查 `::before`、`::after`。
7. 先在 CDP 临时设置样式并截图对比。
8. 再把精确规则写入 payload，补测试并删除探查文件。

## 视觉回归重点

- 标签栏与主内容是同一张背景，接缝处不错位。
- 侧栏、顶栏、表头在透明度为 0 时不应残留实心黑条。
- 色块调低后仍保留色相，文字清晰。
- 页面导航 / 切换数据库视图后不能恢复原生实底。
- 浅色主题不能继续使用深色 surface，深色主题不能回落到浅灰色。
