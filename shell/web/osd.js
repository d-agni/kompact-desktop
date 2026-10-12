// @ts-check
// 12c: the on-screen level after a volume key (src-tauri/src/sound.rs): shown for 1.5 s, then
// faded and the window hidden (unmapped, it costs kompact nothing).
"use strict";

// @ts-ignore: injected by Tauri (app.withGlobalTauri).
const tauri = window.__TAURI__;
const box = /** @type {HTMLElement} */ (document.getElementById("osd"));
const bar = /** @type {HTMLElement} */ (document.getElementById("osd-bar"));
const SEGMENTS = 16;
for (let i = 0; i < SEGMENTS; i++) bar.append(document.createElement("i"));
let hideTimer = 0;
let fadeTimer = 0;

/** @param {{kind: string, volume: number, muted: boolean, waves: number} | null} s */
function show(s) {
  if (!s) return;
  box.classList.toggle("mic", s.kind === "mic");
  box.classList.toggle("muted", s.muted);
  box.dataset.waves = String(s.waves);
  const lit = s.muted ? 0 : Math.round(s.volume * SEGMENTS);
  [...bar.children].forEach((el, i) => el.classList.toggle("on", i < lit));
  box.classList.add("shown");
  clearTimeout(hideTimer);
  clearTimeout(fadeTimer);
  hideTimer = setTimeout(() => {
    box.classList.remove("shown");
    fadeTimer = setTimeout(() => tauri?.core.invoke("osd_hide"), 300);
  }, 1500);
}

if (tauri) {
  tauri.event.listen("kompact-osd", (/** @type {{payload: any}} */ e) => show(e.payload));
  tauri.core.invoke("osd_state").then(show);
}
