# Notion 页面入口与选择器

Notion class 名和 StyleX hash 可能随版本变化。这里记录的是稳定入口和定位方法，
不是允许盲目复制的永久 API。每次 Notion 更新后都要用 CDP 复核。

## 全局窗口骨架

### Electron 顶部标签栏

- 独立 page：`file:///.../app.asar/.webpack/renderer/tabs/index.html`
- 典型高度约 `36px`；主内容 `innerHeight` ≈ `outerHeight - 36`。
- 根 class：`notion-background-tab-chrome`
- 处理：清 `.root` / `.hide-scrollbar` 及标签芯片实色底。
- 背景媒体：高度用 `outerHeight`，`top: 0`，与主页共用同一张图。

### 主内容页

- URL：`https://app.notion.com/...` 或 `https://www.notion.so/...`
- 根 class：`notion-background-active`；空白恢复页额外走 `notion-background-task`
- 背景媒体：高度用 `outerHeight`，`top: -(outerHeight - innerHeight)`，与标签栏拼图。

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
  `background: var(--c-bacPri)` → 必须清成透明。
- 表格/侧栏 emoji 走 `img.notion-emoji` + spritesheet `background-image`，
  不要对内容区写宽泛的 `background-image: none`。

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
