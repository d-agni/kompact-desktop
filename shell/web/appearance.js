// @ts-check
// 10d-3: light or dark, and the accent colour, from kompact's settings (System Settings →
// Appearance; kompact works out Auto by the clock). Sets `data-theme="light|dark"` and
// `data-buttons="left|right"` on <html> and `--accent` on it, before the page's own script, and
// again whenever kompact sends `settings`. Each page's CSS has its light colours under
// `:root[data-theme="light"]`.
"use strict";
(function () {
  // @ts-ignore: injected by Tauri (app.withGlobalTauri).
  const tauri = window.__TAURI__;
  /** macOS's accents, [light, dark]. */
  const ACCENTS = /** @type {Record<string, [string, string]>} */ ({
    blue: ["#007aff", "#0a84ff"],
    purple: ["#953d96", "#a550a7"],
    pink: ["#e8478f", "#f74f9e"],
    red: ["#e0383e", "#ff453a"],
    orange: ["#f7821b", "#ff9f0a"],
    yellow: ["#d9a400", "#ffd60a"],
    green: ["#28a745", "#32d74b"],
    graphite: ["#8c8c8c", "#98989d"],
  });
  let last = "";
  /** @param {{dark?: boolean, accent?: string, buttons?: string} | null} s */
  function apply(s) {
    if (!s) return;
    const dark = s.dark !== false;
    const accent = (ACCENTS[s.accent || ""] || ACCENTS.blue)[dark ? 1 : 0];
    const root = document.documentElement;
    root.dataset.theme = dark ? "dark" : "light";
    root.dataset.buttons = s.buttons === "right" ? "right" : "left";
    root.style.setProperty("--accent", accent);
    const sig = `${root.dataset.theme} ${accent} ${root.dataset.buttons}`;
    if (sig !== last) {
      last = sig;
      console.log(`appearance: ${sig}`);
    }
  }
  if (tauri) {
    tauri.event.listen("kompact-settings", (/** @type {{payload: any}} */ e) => apply(e.payload));
    tauri.core.invoke("kompact_settings_state").then(apply);
  }
})();
