import { createHash } from "node:crypto";
import { DisplaySettings, MediaKind } from "../shared/contracts.js";

const BACKGROUND_CSS = String.raw`
html.notion-background-active,
html.notion-background-active body,
html.notion-background-active .notion-app-inner,
html.notion-background-active .notion-cursor-listener,
html.notion-background-active .root,
html.notion-background-active .notion-dark-theme.root,
html.notion-background-active .notion-light-theme.root {
  background: transparent !important;
  background-color: transparent !important;
}

#notion-background-layer {
  position: fixed;
  inset: 0;
  z-index: 0;
  overflow: hidden;
  pointer-events: none;
  opacity: calc(var(--cbg-opacity) * var(--cbg-route-intensity));
  background-color: transparent;
  transition: opacity 220ms ease;
}

#notion-background-media,
#notion-background-tile {
  position: absolute;
  left: 0;
  width: 100%;
  transform: scale(var(--cbg-scale));
  filter: blur(var(--cbg-blur));
  transform-origin: center center;
}

#notion-background-media {
  display: block;
  object-fit: var(--cbg-fit);
  object-position: var(--cbg-position-x) var(--cbg-position-y);
}

#notion-background-tile {
  display: none;
  background-image: var(--cbg-media-url);
  background-repeat: repeat;
  background-position: var(--cbg-position-x) var(--cbg-position-y);
  background-size: auto;
}

html.notion-background-fit-tile #notion-background-media { display: none; }
html.notion-background-fit-tile #notion-background-tile { display: block; }

#notion-background-overlay {
  position: absolute;
  inset: 0;
  background: var(--cbg-overlay-color);
  opacity: var(--cbg-overlay-opacity);
}

/* Notion 色块底（折叠块/高亮块）：保留原色相，按滑杆透出背景图。
 * 用 --cbg-captured-fill 是否存在做开关，避免 React 冲掉 data-* 标记。 */
html.notion-background-active .notion-page-content [style*="--cbg-captured-fill"] {
  background: color-mix(
    in srgb,
    var(--cbg-captured-fill) calc(var(--cbg-block-fill-opacity) * 100%),
    transparent
  ) !important;
  background-color: color-mix(
    in srgb,
    var(--cbg-captured-fill) calc(var(--cbg-block-fill-opacity) * 100%),
    transparent
  ) !important;
}

html.notion-background-home { --cbg-route-intensity: var(--cbg-home-intensity); }
html.notion-background-task { --cbg-route-intensity: var(--cbg-task-intensity); }
html.notion-background-home.notion-background-home-disabled,
html.notion-background-task.notion-background-task-disabled { --cbg-route-intensity: 0; }

/*
 * 壳层（侧栏 / 共享顶栏 / 主画布）默认透出整图。
 * slider 只加很轻的雾（* 28%），避免 0.8 变成近乎实底黑罩。
 */
html.notion-background-active .notion-sidebar,
html.notion-background-active .notion-sidebar-container,
html.notion-background-active .notion-sidebar-switcher,
html.notion-background-active nav.notion-sidebar-container,
html.notion-background-active .notion-print-ignore {
  /* 收起悬停弹出时，底板子层用 --c-bacEle（实色 #202020），需一并压成雾 */
  --c-bacEle: color-mix(in srgb, var(--cbg-surface-color, #191919) calc(var(--cbg-sidebar-opacity) * 28%), transparent) !important;
  background: color-mix(in srgb, var(--cbg-surface-color, #191919) calc(var(--cbg-sidebar-opacity) * 28%), transparent) !important;
  background-color: color-mix(in srgb, var(--cbg-surface-color, #191919) calc(var(--cbg-sidebar-opacity) * 28%), transparent) !important;
  backdrop-filter: none !important;
  box-shadow: none !important;
}
html.notion-background-active .notion-sidebar *,
html.notion-background-active .notion-sidebar-container * {
  backdrop-filter: none !important;
  box-shadow: none !important;
}
/* 侧栏绝对定位底板：inline background: var(--c-bacEle) */
html.notion-background-active .notion-sidebar [style*="--c-bacEle"],
html.notion-background-active .notion-sidebar [style*="var(--c-bacEle)"] {
  background: color-mix(in srgb, var(--cbg-surface-color, #191919) calc(var(--cbg-sidebar-opacity) * 28%), transparent) !important;
  background-color: color-mix(in srgb, var(--cbg-surface-color, #191919) calc(var(--cbg-sidebar-opacity) * 28%), transparent) !important;
}

/* 带「共享」的页面顶栏 + 主画布外框 */
html.notion-background-active main.notion-frame,
html.notion-background-active header,
html.notion-background-active .notion-topbar,
html.notion-background-active .notion-topbar-action-buttons,
html.notion-background-active .notion-peek-renderer .notion-topbar {
  background: color-mix(in srgb, var(--cbg-surface-color, #191919) calc(var(--cbg-surface-opacity) * 28%), transparent) !important;
  background-color: color-mix(in srgb, var(--cbg-surface-color, #191919) calc(var(--cbg-surface-opacity) * 28%), transparent) !important;
  backdrop-filter: none !important;
  box-shadow: none !important;
}
html.notion-background-active main.notion-frame .notion-page-content,
html.notion-background-active main.notion-frame .notion-scroller,
html.notion-background-active main.notion-frame .layout-content,
html.notion-background-active main.notion-frame [class*="notion-page-block"],
html.notion-background-active main.notion-frame .whenContentEditable,
html.notion-background-active main.notion-frame .content-editable-void-no-select {
  background: transparent !important;
  background-color: transparent !important;
}

/* 右侧文章预览：稳定入口是 .notion-peek-renderer；其直接子壳层以内联
 * background: var(--c-bacEle) + --c-shaOutMd 盖住整块页面，必须只在该区域压掉。 */
html.notion-background-active .notion-peek-renderer > div[style*="var(--c-bacEle)"],
html.notion-background-active .notion-peek-renderer > div[style*="var(--c-bacEle)"] > .peek-top-hover-area {
  --c-bacPri: color-mix(in srgb, var(--cbg-surface-color, #191919) calc(var(--cbg-surface-opacity) * 28%), transparent) !important;
  --c-bacSec: color-mix(in srgb, var(--cbg-surface-color, #191919) calc(var(--cbg-surface-opacity) * 28%), transparent) !important;
  --c-bacEle: color-mix(in srgb, var(--cbg-surface-color, #191919) calc(var(--cbg-surface-opacity) * 28%), transparent) !important;
  --c-bacInt: color-mix(in srgb, var(--cbg-surface-color, #191919) calc(var(--cbg-surface-opacity) * 28%), transparent) !important;
  background: color-mix(in srgb, var(--cbg-surface-color, #191919) calc(var(--cbg-surface-opacity) * 28%), transparent) !important;
  background-color: color-mix(in srgb, var(--cbg-surface-color, #191919) calc(var(--cbg-surface-opacity) * 28%), transparent) !important;
  box-shadow: none !important;
  backdrop-filter: none !important;
}
html.notion-background-active .notion-peek-renderer .notion-page-content,
html.notion-background-active .notion-peek-renderer .notion-scroller,
html.notion-background-active .notion-peek-renderer .layout-content,
html.notion-background-active .notion-peek-renderer [class*="notion-page-block"],
html.notion-background-active .notion-peek-renderer .whenContentEditable,
html.notion-background-active .notion-peek-renderer .content-editable-void-no-select {
  background: transparent !important;
  background-color: transparent !important;
  box-shadow: none !important;
  backdrop-filter: none !important;
}
html.notion-background-active .notion-peek-renderer [style*="--c-bluBacSec"] {
  background: color-mix(in srgb, var(--c-bluBacSec) calc(var(--cbg-surface-opacity) * 55%), transparent) !important;
  background-color: color-mix(in srgb, var(--c-bluBacSec) calc(var(--cbg-surface-opacity) * 55%), transparent) !important;
  box-shadow: none !important;
  backdrop-filter: none !important;
}
html.notion-background-active .notion-peek-renderer img[data-cbg-cover="1"],
html.notion-background-active .notion-peek-renderer .notion-record-icon img {
  background: transparent !important;
  background-color: transparent !important;
}

/*
 * 页面封面：允许显示，按顶栏/画布滑杆做半透明。
 * 绝不能 visibility:hidden / opacity:0 —— Notion 懒加载会永久停在 1x1 占位图，
 * 连带页面图标也变成 --c-bacPri 黑块。
 */
html.notion-background-active main.notion-frame img[data-cbg-cover="1"] {
  opacity: calc(0.45 + var(--cbg-surface-opacity) * 0.55) !important;
  visibility: visible !important;
  background: transparent !important;
  background-color: transparent !important;
}

/* 页面图标/封面占位：清掉 Notion 的 --c-bacPri 实心底，避免未加载时出现黑方块 */
html.notion-background-active main.notion-frame .notion-record-icon img,
html.notion-background-active main.notion-frame img[data-cbg-cover="1"] {
  background: transparent !important;
  background-color: transparent !important;
}

/* 侧栏 Agent / AI 对话面板（.chat_sidebar 内层仍写死 --c-bacPri） */
html.notion-background-active .chat_sidebar,
html.notion-background-active .chat_sidebar .test123,
html.notion-background-active .chat_sidebar [style*="--c-bacPri"],
html.notion-background-active .chat_sidebar [style*="background: var(--c-bacPri)"],
html.notion-background-active .chat_sidebar [style*="background-color: var(--c-bacPri)"] {
  --c-bacPri: color-mix(in srgb, var(--cbg-surface-color, #191919) calc(var(--cbg-surface-opacity) * 28%), transparent) !important;
  --c-bacInt: color-mix(in srgb, var(--cbg-surface-color, #191919) calc(var(--cbg-surface-opacity) * 28%), transparent) !important;
  background: color-mix(in srgb, var(--cbg-surface-color, #191919) calc(var(--cbg-surface-opacity) * 28%), transparent) !important;
  background-color: color-mix(in srgb, var(--cbg-surface-color, #191919) calc(var(--cbg-surface-opacity) * 28%), transparent) !important;
  box-shadow: none !important;
  backdrop-filter: none !important;
}

/* Agent 底部输入框：走 --c-bacSec + 外阴影，不是 bacPri */
html.notion-background-active .chat_sidebar [style*="--c-bacSec"],
html.notion-background-active .chat_sidebar [style*="background-color: var(--c-bacSec)"],
html.notion-background-active .chat_sidebar [style*="background: var(--c-bacSec)"] {
  --c-bacSec: color-mix(in srgb, var(--cbg-surface-color, #191919) calc(var(--cbg-surface-opacity) * 28%), transparent) !important;
  background: color-mix(in srgb, var(--cbg-surface-color, #191919) calc(var(--cbg-surface-opacity) * 28%), transparent) !important;
  background-color: color-mix(in srgb, var(--cbg-surface-color, #191919) calc(var(--cbg-surface-opacity) * 28%), transparent) !important;
  box-shadow: none !important;
  backdrop-filter: none !important;
}

/*
 * 数据库表头 / 增行等仍带实心底 + 黑色 box-shadow，slider 到 0 也消不掉。
 * 跟主画布同一套 surface 雾度；为 0 时整行透出背景。
 */
html.notion-background-active .notion-table-view-header-row,
html.notion-background-active .notion-table-view-add-row,
html.notion-background-active .notion-table-view-add-column,
html.notion-background-active .notion-table-view-frozen-column-repositioner {
  background: color-mix(in srgb, var(--cbg-surface-color, #191919) calc(var(--cbg-surface-opacity) * 28%), transparent) !important;
  background-color: color-mix(in srgb, var(--cbg-surface-color, #191919) calc(var(--cbg-surface-opacity) * 28%), transparent) !important;
  box-shadow: none !important;
  backdrop-filter: none !important;
}
html.notion-background-active .notion-table-view-header-cell,
html.notion-background-active .notion-table-view-row,
html.notion-background-active .notion-table-view-cell,
html.notion-background-active .notion-table-view,
html.notion-background-active .notion-collection-view-body,
html.notion-background-active .notion-collection-result-wrapper {
  background: transparent !important;
  background-color: transparent !important;
  box-shadow: none !important;
  backdrop-filter: none !important;
}

/* 侧栏底部「新对话」等实色按钮（StyleX 走 --c-bacPri，需一起压掉） */
html.notion-background-active .notion-sidebar [role="button"][data-cbg-solid-chrome="1"] {
  --c-bacPri: color-mix(in srgb, var(--cbg-surface-color, #191919) calc(var(--cbg-sidebar-opacity) * 40%), transparent) !important;
  --c-bacInt: color-mix(in srgb, var(--cbg-surface-color, #191919) calc(var(--cbg-sidebar-opacity) * 40%), transparent) !important;
  background: color-mix(in srgb, var(--cbg-surface-color, #191919) calc(var(--cbg-sidebar-opacity) * 40%), transparent) !important;
  background-color: color-mix(in srgb, var(--cbg-surface-color, #191919) calc(var(--cbg-sidebar-opacity) * 40%), transparent) !important;
  background-image: none !important;
  box-shadow: none !important;
  backdrop-filter: none !important;
}
/* 行内「打开」快捷条、右下角 AI 浮钮 */
html.notion-background-active .quickActionContainer,
html.notion-background-active .notion-ai-button,
html.notion-background-active .notion-ai-button > * {
  background: color-mix(in srgb, var(--cbg-surface-color, #191919) calc(var(--cbg-surface-opacity) * 40%), transparent) !important;
  background-color: color-mix(in srgb, var(--cbg-surface-color, #191919) calc(var(--cbg-surface-opacity) * 40%), transparent) !important;
  background-image: none !important;
  box-shadow: none !important;
  backdrop-filter: none !important;
}

/* 站点上线蓝条（「此页面已在 xxx.notion.site 上线」） */
html.notion-background-active main.notion-frame [style*="--c-bluBacSec"] {
  background: color-mix(in srgb, var(--c-bluBacSec) calc(var(--cbg-surface-opacity) * 55%), transparent) !important;
  background-color: color-mix(in srgb, var(--c-bluBacSec) calc(var(--cbg-surface-opacity) * 55%), transparent) !important;
  box-shadow: none !important;
  backdrop-filter: none !important;
}

/* 数据库底部计数条（「计数 N」sticky 底栏） */
html.notion-background-active main.notion-frame .sticky-portal-target,
html.notion-background-active main.notion-frame .sticky-portal-target [style*="clip-path"] {
  background: transparent !important;
  background-color: transparent !important;
  box-shadow: none !important;
}
html.notion-background-active main.notion-frame .sticky-portal-target .content-editable-void-no-select,
html.notion-background-active main.notion-frame .sticky-portal-target [style*="--c-bacPri"] {
  --c-bacPri: color-mix(in srgb, var(--cbg-surface-color, #191919) calc(var(--cbg-surface-opacity) * 28%), transparent) !important;
  background: color-mix(in srgb, var(--cbg-surface-color, #191919) calc(var(--cbg-surface-opacity) * 28%), transparent) !important;
  background-color: color-mix(in srgb, var(--cbg-surface-color, #191919) calc(var(--cbg-surface-opacity) * 28%), transparent) !important;
  box-shadow: none !important;
  backdrop-filter: none !important;
}

/* 顶部 Electron 标签栏：清掉每个标签芯片的实色底，露出同一张背景。 */
html.notion-background-tab-chrome,
html.notion-background-tab-chrome body,
html.notion-background-tab-chrome .root,
html.notion-background-tab-chrome .root.notion-dark-theme,
html.notion-background-tab-chrome .root.notion-light-theme,
html.notion-background-tab-chrome .hide-scrollbar {
  background: transparent !important;
  background-color: transparent !important;
}
html.notion-background-tab-chrome .root *,
html.notion-background-tab-chrome .hide-scrollbar * {
  background-color: transparent !important;
  box-shadow: none !important;
  border-color: transparent !important;
}
/* Notion 在标签边缘用内联 linear-gradient 画暗色过渡；它不是 box-shadow。 */
html.notion-background-tab-chrome [style*="linear-gradient"][style*="--gradient-direction"] {
  background: transparent !important;
  background-image: none !important;
}

/* 弹出菜单、对话框按菜单透明度打底。 */
html.notion-background-active .notion-overlay-container [role="dialog"],
html.notion-background-active .notion-overlay-container [role="menu"],
html.notion-background-active .notion-overlay-container [role="listbox"],
html.notion-background-active .notion-overlay-container .notion-dropdown-menu {
  background: color-mix(in srgb, var(--cbg-surface-color, #191919) calc(var(--cbg-menu-opacity) * 100%), transparent) !important;
  backdrop-filter: none !important;
  box-shadow: none !important;
}

html.notion-background-dark #notion-background-layer {
  background-color: transparent;
}

@media (prefers-reduced-motion: reduce) {
  #notion-background-layer { transition: none; }
}
`;

const REVIEW_SHADOW_STYLE_ID = "notion-background-review-shadow-style";
const REVIEW_SHADOW_CSS = String.raw`
/* Notion MVP 暂不注入 Shadow DOM 审阅样式；保留占位以兼容修订哈希与清理逻辑。 */
:host { background-color: transparent !important; }
`;

export interface PayloadInput {
  mediaUrl: string;
  mediaKind: MediaKind;
  display: DisplaySettings;
  revision: string;
}

export function buildRendererPayload(input: PayloadInput) {
  const revision = createHash("sha256")
    .update(input.revision)
    .update(BACKGROUND_CSS)
    .update(REVIEW_SHADOW_CSS)
    .digest("hex");
  const serialized = JSON.stringify({ ...input, revision }).replace(/</g, "\\u003c");
  const css = JSON.stringify(BACKGROUND_CSS);
  const reviewShadowCss = JSON.stringify(REVIEW_SHADOW_CSS);
  const reviewShadowStyleId = JSON.stringify(REVIEW_SHADOW_STYLE_ID);
  return String.raw`((config, cssText, reviewShadowCssText, reviewShadowStyleId) => {
    const STATE = "__NOTION_BACKGROUND_STUDIO__";
    const STYLE_ID = "notion-background-style";
    const LAYER_ID = "notion-background-layer";
    const REVIEW_HOST_SELECTOR = "diffs-container";
    const ROOT_CLASSES = [
      "notion-background-active", "notion-background-home", "notion-background-task",
      "notion-background-home-disabled", "notion-background-task-disabled",
      "notion-background-fit-tile", "notion-background-dark", "notion-background-tab-chrome"
    ];
    const ROOT_PROPERTIES = [
      "--cbg-opacity", "--cbg-blur", "--cbg-scale", "--cbg-fit",
      "--cbg-position-x", "--cbg-position-y", "--cbg-overlay-color",
      "--cbg-overlay-opacity", "--cbg-home-intensity", "--cbg-task-intensity",
      "--cbg-route-intensity", "--cbg-sidebar-opacity", "--cbg-surface-opacity",
      "--cbg-composer-opacity", "--cbg-menu-opacity", "--cbg-terminal-opacity",
      "--cbg-block-fill-opacity", "--cbg-media-url", "--cbg-surface-color"
    ];

    const previous = window[STATE];
    if (previous?.cleanup) {
      previous.cleanup();
    } else {
      if (previous?.observer) previous.observer.disconnect();
      if (previous?.timer) clearInterval(previous.timer);
      previous?.layer?.remove();
      if (previous?.blobUrl) URL.revokeObjectURL(previous.blobUrl);
    }
    let scheduled = null;
    let shadowPatch = null;

    const blobUrl = (() => {
      const comma = config.mediaUrl.indexOf(",");
      if (!config.mediaUrl.startsWith("data:") || comma < 0) return config.mediaUrl;
      const binary = atob(config.mediaUrl.slice(comma + 1));
      const bytes = new Uint8Array(binary.length);
      for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index);
      const mime = /^data:([^;,]+)/.exec(config.mediaUrl)?.[1] || "application/octet-stream";
      return URL.createObjectURL(new Blob([bytes], { type: mime }));
    })();

    const installReviewShadowStyle = (host, shadow = host?.shadowRoot) => {
      if (!shadow) return false;
      let shadowStyle = shadow.getElementById(reviewShadowStyleId);
      if (!shadowStyle) {
        shadowStyle = document.createElement("style");
        shadowStyle.id = reviewShadowStyleId;
      }
      if (shadowStyle.dataset.cbgRevision !== config.revision) {
        shadowStyle.textContent = reviewShadowCssText;
        shadowStyle.dataset.cbgRevision = config.revision;
      }
      shadow.appendChild(shadowStyle);
      return true;
    };

    const isTabChrome = () => location.protocol === "file:" && location.href.toLowerCase().includes("/tabs/index.html");

    const syncFullWindowMedia = (layer, media, tile) => {
      const fullH = Math.max(Number(window.outerHeight) || 0, Number(window.innerHeight) || 0, 1);
      const viewH = Math.max(Number(window.innerHeight) || 0, 1);
      const shiftY = isTabChrome() ? 0 : -Math.max(0, fullH - viewH);
      layer.style.position = "fixed";
      layer.style.inset = "0";
      layer.style.overflow = "hidden";
      layer.style.zIndex = "0";
      for (const node of [media, tile]) {
        if (!node) continue;
        node.style.position = "absolute";
        node.style.left = "0";
        node.style.width = "100%";
        node.style.height = fullH + "px";
        node.style.top = shiftY + "px";
      }
    };

    const markNativeCovers = () => {
      document.querySelectorAll("main.notion-frame img, .notion-peek-renderer img").forEach((img) => {
        if (img.id === "notion-background-media") return;
        if (img.classList?.contains("notion-emoji")) return;
        if (img.closest?.(".notion-record-icon")) return;
        const rect = img.getBoundingClientRect();
        const wide = rect.width > Math.max(320, window.innerWidth * 0.45);
        const tall = rect.height > 120;
        const nearTop = rect.y < 160;
        if (wide && tall && nearTop) {
          img.setAttribute("data-cbg-cover", "1");
          img.removeAttribute("data-cbg-cover-hidden");
        }
      });
    };

    const isOpaqueFill = (bg) => {
      if (!bg || bg === "transparent") return false;
      if (bg === "rgba(0, 0, 0, 0)" || bg === "rgb(0, 0, 0, 0)") return false;
      const slash = bg.match(/\/\s*([0-9.]+)\s*\)/);
      if (slash && Number(slash[1]) < 0.08) return false;
      const rgba = bg.match(/rgba?\(([^)]+)\)/);
      if (rgba) {
        const parts = rgba[1].split(",").map((s) => s.trim());
        if (parts.length === 4 && Number(parts[3]) < 0.08) return false;
        return true;
      }
      return /oklab|oklch|color\(/i.test(bg);
    };

    const markSolidSidebarChrome = () => {
      document.querySelectorAll('.notion-sidebar [role="button"]').forEach((el) => {
        if (el.getAttribute("data-cbg-solid-chrome") === "1") return;
        // 只标记实色底；半透明选中态留给 Notion。
        if (isOpaqueFill(getComputedStyle(el).backgroundColor)) {
          el.setAttribute("data-cbg-solid-chrome", "1");
        }
      });
    };

    const markBlockFills = () => {
      document.querySelectorAll(".notion-page-content [style*='background']").forEach((el) => {
        const style = el.getAttribute("style") || "";
        // 半透明 Tra 描边层不参与；清掉误标
        if (/--ca-\w*Bac\w*Tra/.test(style)) {
          el.style.removeProperty("--cbg-captured-fill");
          return;
        }
        // Notion 色块走 --c-xxxBacSec / BacPri
        if (!/--c-[a-z]{3}Bac(?:Sec|Pri|Ter)\b/.test(style)) return;
        if (el.style.getPropertyValue("--cbg-captured-fill")) return;
        const captured = getComputedStyle(el).backgroundColor;
        if (!isOpaqueFill(captured)) return;
        el.style.setProperty("--cbg-captured-fill", captured);
      });
    };

    const restoreNativeCovers = () => {
      document.querySelectorAll('img[data-cbg-cover="1"], img[data-cbg-cover-hidden="1"]').forEach((img) => {
        img.removeAttribute("data-cbg-cover");
        img.removeAttribute("data-cbg-cover-hidden");
      });
      document.querySelectorAll("[data-cbg-solid-chrome]").forEach((el) => {
        el.removeAttribute("data-cbg-solid-chrome");
      });
      document.querySelectorAll(".notion-page-content [style*='--cbg-captured-fill']").forEach((el) => {
        el.style.removeProperty("--cbg-captured-fill");
      });
    };

    const cleanup = () => {
      const state = window[STATE];
      state?.observer?.disconnect();
      if (state?.timer) clearInterval(state.timer);
      if (scheduled) cancelAnimationFrame(scheduled);
      if (shadowPatch?.prototype.attachShadow === shadowPatch.wrapped) {
        shadowPatch.prototype.attachShadow = shadowPatch.original;
      }
      restoreNativeCovers();
      document.getElementById(LAYER_ID)?.remove();
      document.getElementById(STYLE_ID)?.remove();
      document.querySelectorAll("diffs-container").forEach((host) => {
        host.shadowRoot?.getElementById(reviewShadowStyleId)?.remove();
      });
      document.documentElement?.classList.remove(...ROOT_CLASSES);
      for (const property of ROOT_PROPERTIES) document.documentElement?.style.removeProperty(property);
      if (state?.blobUrl) URL.revokeObjectURL(state.blobUrl);
      delete window[STATE];
      return true;
    };

    const patchAttachShadow = () => {
      const prototype = Element.prototype;
      const original = prototype.attachShadow;
      const wrapped = function(init) {
        const shadow = original.call(this, init);
        if (this.localName === REVIEW_HOST_SELECTOR) {
          queueMicrotask(() => installReviewShadowStyle(this, shadow));
          requestAnimationFrame(() => installReviewShadowStyle(this, shadow));
        }
        return shadow;
      };
      prototype.attachShadow = wrapped;
      return { prototype, original, wrapped };
    };
    shadowPatch = patchAttachShadow();

    const detectAppearance = () => {
      const root = document.documentElement;
      const classText = ((root?.className || "") + " " + (document.body?.className || ""))
        .toLowerCase()
        .replace(/\bnotion-background-[a-z-]+\b/g, "");
      if (/\b(?:dark|notion-dark-theme|theme-dark)\b/.test(classText)) return "dark";
      if (/\b(?:light|notion-light-theme|theme-light)\b/.test(classText)) return "light";
      const dataTheme = (
        root?.getAttribute("data-theme") || root?.getAttribute("data-appearance") ||
        document.body?.getAttribute("data-theme") || ""
      ).toLowerCase();
      if (dataTheme.includes("dark")) return "dark";
      if (dataTheme.includes("light")) return "light";
      try {
        return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
      } catch {}
      return "light";
    };

    const install = () => {
      const root = document.documentElement;
      if (!root) return false;

      const setClass = (name, on) => {
        if (root.classList.contains(name) !== on) root.classList.toggle(name, on);
      };
      const setProp = (name, value) => {
        if (root.style.getPropertyValue(name) !== value) root.style.setProperty(name, value);
      };

      const dark = detectAppearance() === "dark";
      setClass("notion-background-dark", dark);
      setProp("--cbg-surface-color", dark ? "#191919" : "#ffffff");

      let style = document.getElementById(STYLE_ID);
      if (!style) {
        style = document.createElement("style");
        style.id = STYLE_ID;
        (document.head || root).appendChild(style);
      }
      if (style.dataset.cbgRevision !== config.revision) {
        style.textContent = cssText;
        style.dataset.cbgRevision = config.revision;
      }

      let layer = document.getElementById(LAYER_ID);
      if (!layer && document.body) {
        layer = document.createElement("div");
        layer.id = LAYER_ID;
        const media = document.createElement(config.mediaKind === "video" ? "video" : "img");
        media.id = "notion-background-media";
        media.setAttribute("aria-hidden", "true");
        if (config.mediaKind === "video") {
          media.autoplay = true;
          media.loop = true;
          media.muted = Boolean(config.display.videoMuted);
          media.defaultMuted = Boolean(config.display.videoMuted);
          media.playsInline = true;
          media.playbackRate = Number(config.display.videoPlaybackRate) || 1;
        }
        media.src = blobUrl;
        // 媒体加载失败不要拆掉整页注入；否则 Tab Bar 会只剩透明底+系统黑底。
        media.addEventListener("error", () => {
          try { console.warn("[notion-background] media load failed", media.currentSrc || media.src); } catch {}
        });
        const tile = document.createElement("div");
        tile.id = "notion-background-tile";
        const overlay = document.createElement("div");
        overlay.id = "notion-background-overlay";
        layer.append(media, tile, overlay);
        document.body.prepend(layer);
        if (config.mediaKind === "video") media.play().catch(() => undefined);
      }
      if (layer) {
        const mediaNode = document.getElementById("notion-background-media");
        if (mediaNode && mediaNode.getAttribute("src") !== blobUrl) {
          mediaNode.setAttribute("src", blobUrl);
          if (config.mediaKind === "video") mediaNode.play?.().catch(() => undefined);
        }
        syncFullWindowMedia(
          layer,
          mediaNode,
          document.getElementById("notion-background-tile"),
        );
      }
      markNativeCovers();
      markSolidSidebarChrome();
      markBlockFills();

      setClass("notion-background-active", true);
      setClass("notion-background-tab-chrome", isTabChrome());
      setClass("notion-background-fit-tile", config.display.fit === "tile" && config.mediaKind === "image");
      setClass("notion-background-home-disabled", !config.display.enabledOnHome);
      setClass("notion-background-task-disabled", !config.display.enabledOnTasks);
      setProp("--cbg-opacity", String(config.display.opacity));
      setProp("--cbg-blur", config.display.blur + "px");
      setProp("--cbg-scale", String(config.display.scale));
      setProp("--cbg-fit", config.display.fit === "tile" ? "cover" : config.display.fit);
      setProp("--cbg-position-x", config.display.positionX + "%");
      setProp("--cbg-position-y", config.display.positionY + "%");
      setProp("--cbg-overlay-color", config.display.overlayColor);
      setProp("--cbg-overlay-opacity", String(config.display.overlayOpacity));
      setProp("--cbg-block-fill-opacity", String(config.display.blockFillOpacity));
      setProp("--cbg-home-intensity", String(config.display.homeIntensity));
      setProp("--cbg-task-intensity", String(config.display.taskIntensity));
      setProp("--cbg-sidebar-opacity", String(config.display.sidebarOpacity));
      setProp("--cbg-surface-opacity", String(config.display.surfaceOpacity));
      setProp("--cbg-composer-opacity", String(config.display.composerOpacity));
      setProp("--cbg-menu-opacity", String(config.display.menuOpacity));
      setProp("--cbg-terminal-opacity", String(config.display.terminalOpacity));
      setProp("--cbg-media-url", 'url("' + String(blobUrl).replace(/["\\\n\r]/g, "") + '")');

      // Notion 页面统一按“页面”强度走 home 通道；空白恢复页走 task 通道便于单独关掉。
      const blank = /\/blank(?:\?|$)/.test(location.pathname + location.search);
      setClass("notion-background-home", !blank);
      setClass("notion-background-task", blank);
      // Re-apply the resolved opacity inline after route variables/classes settle.
      // Electron can leave the stylesheet declaration at its initial value when
      // the large payload creates the layer before the Notion document finishes mounting.
      if (layer) {
        layer.style.setProperty("opacity", "calc(var(--cbg-opacity) * var(--cbg-route-intensity))");
      }
      return true;
    };

    const scheduleInstall = () => {
      if (scheduled) return;
      scheduled = requestAnimationFrame(() => { scheduled = null; install(); });
    };
    const observer = new MutationObserver(scheduleInstall);
    observer.observe(document.documentElement, {
      childList: true,
      subtree: true,
      attributes: true,
      attributeFilter: ["class", "data-theme", "data-appearance"],
    });
    const timer = setInterval(install, 4000);
    window[STATE] = { revision: config.revision, cleanup, observer, timer, layer: null, blobUrl };
    install();
    window[STATE].layer = document.getElementById(LAYER_ID);
    return { installed: true, revision: config.revision, mediaKind: config.mediaKind };
  })(${serialized}, ${css}, ${reviewShadowCss}, ${reviewShadowStyleId})`;
}

export const REMOVE_RENDERER_PAYLOAD = String.raw`(() => {
  const state = window.__NOTION_BACKGROUND_STUDIO__;
  if (state?.cleanup) return state.cleanup();
  document.getElementById("notion-background-layer")?.remove();
  document.getElementById("notion-background-style")?.remove();
  document.documentElement?.classList.remove(
    "notion-background-active", "notion-background-home", "notion-background-task",
    "notion-background-home-disabled", "notion-background-task-disabled",
    "notion-background-fit-tile", "notion-background-tab-chrome"
  );
  delete window.__NOTION_BACKGROUND_STUDIO__;
  return true;
})()`;

export function earlyPayloadFor(payload: string, revision: string) {
  const safeRevision = JSON.stringify(revision);
  return String.raw`(() => {
    const revision = ${safeRevision};
    const run = () => {
      if (!document.documentElement) return false;
      try { ${payload}; return true; } catch { return false; }
    };
    if (!run()) {
      const observer = new MutationObserver(() => {
        if (run()) observer.disconnect();
      });
      observer.observe(document.documentElement || document, { childList: true, subtree: true });
      setTimeout(() => observer.disconnect(), 30000);
    }
    return revision;
  })()`;
}
