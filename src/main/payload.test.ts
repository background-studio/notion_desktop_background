import { describe, expect, it } from "vitest";
import { DEFAULT_SETTINGS } from "../shared/contracts.js";
import { buildRendererPayload, earlyPayloadFor, REMOVE_RENDERER_PAYLOAD } from "./payload.js";

describe("renderer payload", () => {
  it("contains Notion shell selectors and an inert decorative layer", () => {
    const payload = buildRendererPayload({
      mediaUrl: "http://127.0.0.1:9444/token/media/id",
      mediaKind: "video",
      display: DEFAULT_SETTINGS.display,
      revision: "revision-1",
    });
    expect(payload).toContain("notion-background-layer");
    expect(payload).toContain("pointer-events: none");
    expect(payload).toContain(".notion-app-inner");
    expect(payload).toContain(".notion-cursor-listener");
    expect(payload).toContain("main.notion-frame");
    expect(payload).toContain(".notion-sidebar");
    expect(payload).toContain("--c-bacEle");
    expect(payload).toContain(".notion-topbar");
    expect(payload).toContain("syncFullWindowMedia");
    expect(payload).toContain("resolveTabChromeHeight");
    expect(payload).toContain("DEFAULT_TAB_CHROME_HEIGHT");
    expect(payload).toContain("MAX_PLAUSIBLE_TAB_CHROME_HEIGHT");
    expect(payload).toContain('window.addEventListener("resize"');
    expect(payload).toContain("visualViewport");
    expect(payload).toContain("tab-chrome-align-v2");
    expect(payload).not.toContain("shiftY = isTabChrome() ? 0 : -Math.max(0, fullH - viewH)");
    expect(payload).toContain("data-cbg-cover");
    expect(payload).toContain("markNativeCovers");
    expect(payload).toContain(".chat_sidebar");
    expect(payload).toContain("--c-bacSec");
    expect(payload).toContain('.notion-peek-renderer > div[style*="var(--c-bacEle)"]');
    expect(payload).toContain(".peek-top-hover-area");
    expect(payload).toContain(".notion-peek-renderer .notion-scroller");
    expect(payload).toContain(".notion-record-icon img:not(.notion-emoji)");
    expect(payload).toContain("img.notion-emoji");
    expect(payload).toContain("border-color: color-mix");
    expect(payload).not.toContain("main.notion-frame .notion-record-icon img,");
    expect(payload).toContain("isTabChrome");
    expect(payload).toContain("notion-background-home");
    expect(payload).toContain("notion-background-task");
    expect(payload).toContain("media.playbackRate");
    expect(payload).toContain("notion-dark-theme");
    expect(payload).toContain("__NOTION_BACKGROUND_STUDIO__");
    expect(payload).toContain("requestAnimationFrame");
    expect(payload).toContain("await Promise.race([");
    expect(payload).toContain("preparedMedia.decode()");
    expect(payload).toContain("background media decode timeout");
    expect(payload).toContain("Decode before touching the visible layer");
    expect(payload.indexOf("preparedMedia.decode()")).toBeLessThan(payload.indexOf("previous.cleanup()"));
    expect(payload.indexOf("layer.style.opacity = String")).toBeLessThan(payload.indexOf("document.body.prepend(layer)"));
    expect(payload).toContain("transition: none !important");
    expect(payload).not.toContain("transition: opacity");
    expect(payload).toContain('layer.style.setProperty("opacity", "calc(var(--cbg-opacity) * var(--cbg-route-intensity))")');
    expect(payload.indexOf('setClass("notion-background-home", !blank)')).toBeLessThan(
      payload.indexOf('layer.style.setProperty("opacity", "calc(var(--cbg-opacity) * var(--cbg-route-intensity))")'),
    );
    expect(payload).not.toContain("}, 200)");
    expect(payload).not.toContain("backdrop-filter: blur");
    expect(payload).not.toContain("body > #root");
    expect(payload).not.toContain("MainContentViewport");
  });

  it("serializes media URLs instead of interpolating executable source", () => {
    const payload = buildRendererPayload({
      mediaUrl: "http://127.0.0.1/media/\";window.pwned=true;//",
      mediaKind: "image",
      display: DEFAULT_SETTINGS.display,
      revision: "safe",
    });
    expect(payload).toContain(JSON.stringify("http://127.0.0.1/media/\";window.pwned=true;//"));
    expect(payload).not.toContain('src = "http://127.0.0.1/media/"');
  });

  it("keeps cleanup and early payload reversible", () => {
    expect(REMOVE_RENDERER_PAYLOAD).toContain("cleanup");
    expect(REMOVE_RENDERER_PAYLOAD).toContain("__NOTION_BACKGROUND_STUDIO__");
    const early = earlyPayloadFor("window.test = true", "revision-1");
    expect(early).toContain("revision-1");
    expect(early).toContain("MutationObserver");
    expect(early).toContain('document.documentElement?.localName !== "html"');
  });
});
