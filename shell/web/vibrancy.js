// @ts-check
// 6m-a "vibrancy": show the blurred copy of the wallpaper (kompact-wallpaper://localhost/blurred)
// as this page's background, placed exactly where the wallpaper is on screen, so a see-through
// surface looks like frosted glass over it. kompact itself can't blur.
//
// The wallpaper page draws its image with `background-size: cover` centred on the monitor; we
// do the same sums for the whole monitor and offset by where this surface sits on it and where
// the element sits in the surface.
"use strict";

/**
 * @param {HTMLElement} el gets the background
 * @param {number | (() => {top: number, left: number})} top where this surface's top edge is on
 *   the monitor, in px (10d-1c: or a function giving its top-left corner, for the dock, which
 *   can move to another edge)
 * @returns {() => void} call again after `el` moved within the page (a no-op without an image)
 */
// eslint-disable-next-line no-unused-vars
function kompactVibrancy(el, top) {
  // @ts-ignore: injected by Tauri (app.withGlobalTauri).
  const tauri = window.__TAURI__;
  /** @type {HTMLImageElement | null} */
  let probe = null;
  let loaded = false;
  const place = () => {
    if (!loaded || !probe) return;
    const sw = window.screen.width, sh = window.screen.height;
    const scale = Math.max(sw / probe.naturalWidth, sh / probe.naturalHeight);
    const w = probe.naturalWidth * scale, h = probe.naturalHeight * scale;
    const r = el.getBoundingClientRect();
    const off = typeof top === "function" ? top() : { top, left: 0 };
    const pos = `${(sw - w) / 2 - off.left - r.left}px ${(sh - h) / 2 - off.top - r.top}px`;
    if (el.style.backgroundPosition === pos && el.classList.contains("vibrant")) return;
    el.style.backgroundImage = `url(${JSON.stringify(probe.src)})`;
    el.style.backgroundSize = `${w}px ${h}px`;
    el.style.backgroundPosition = pos;
    el.style.backgroundRepeat = "no-repeat";
    el.classList.add("vibrant");
  };
  const clear = () => {
    loaded = false;
    probe = null;
    el.style.backgroundImage = el.style.backgroundSize = el.style.backgroundPosition = el.style.backgroundRepeat = "";
    el.classList.remove("vibrant");
  };
  let version = 0;
  // 10d-2: the wallpaper can change at any time; a colour or the gradient has no blurred copy.
  /** @param {{kind: string, version: number}} s */
  const follow = (s) => {
    if (!s || s.version === version) return;
    version = s.version;
    if (s.kind !== "image") {
      clear();
      console.log("vibrancy: no picture, keeping the tint");
      return;
    }
    const next = new Image();
    next.onload = () => {
      if (s.version !== version) return;
      probe = next;
      loaded = true;
      el.classList.remove("vibrant");
      place();
      console.log(`vibrancy: blurred wallpaper ${next.naturalWidth}x${next.naturalHeight} on a ${window.screen.width}x${window.screen.height} screen`);
    };
    next.onerror = () => {
      clear();
      console.log("vibrancy: no blurred wallpaper, keeping the tint");
    };
    next.src = `kompact-wallpaper://localhost/blurred?v=${s.version}`;
  };
  window.addEventListener("resize", place);
  if (tauri) {
    tauri.event.listen("kompact-wallpaper-changed", (/** @type {{payload: any}} */ e) => follow(e.payload));
    tauri.core.invoke("wallpaper_state").then(follow);
  }
  return place;
}
