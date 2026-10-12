// @ts-check
// Wallpaper (6t-b; 10d-2: chosen in System Settings → Wallpaper). The shell says what to show
// (`wallpaper_state`: the gradient, a colour, or a picture served at
// kompact-wallpaper://localhost/image) and tells every page when it changes
// (`kompact-wallpaper-changed`). A picture that can't be loaded keeps what was there.
"use strict";

// @ts-ignore: injected by Tauri (app.withGlobalTauri).
const tauri = window.__TAURI__;
const el = /** @type {HTMLDivElement} */ (document.getElementById("wallpaper"));

/** @typedef {{kind: "gradient" | "color" | "image", color: string | null, image: string | null, version: number}} WallpaperState */

let shown = 0;
/** @param {WallpaperState} s */
function apply(s) {
  if (!s || s.version === shown) return;
  if (s.kind === "image") {
    // Load it first, so a bad file doesn't leave a blank screen.
    const url = `kompact-wallpaper://localhost/image?v=${s.version}`;
    const probe = new Image();
    probe.onload = () => {
      shown = s.version;
      el.classList.remove("gradient");
      el.style.backgroundColor = "";
      el.style.backgroundImage = `url(${JSON.stringify(url)})`;
      console.log(`wallpaper: image (${probe.naturalWidth}x${probe.naturalHeight})`);
    };
    probe.onerror = () => console.log("wallpaper: could not load the image, keeping what was there");
    probe.src = url;
  } else {
    shown = s.version;
    el.style.backgroundImage = "";
    el.classList.toggle("gradient", s.kind !== "color");
    el.style.backgroundColor = s.kind === "color" && s.color ? s.color : "";
    console.log(`wallpaper: ${s.kind === "color" ? s.color : "gradient"}`);
  }
}

if (tauri) {
  tauri.event.listen("kompact-wallpaper-changed", (/** @type {{payload: WallpaperState}} */ e) => apply(e.payload));
  tauri.core.invoke("wallpaper_state").then(apply);
} else {
  console.log("wallpaper: gradient");
}
