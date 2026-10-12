// @ts-check
// Wallpaper: `?image=<file:// URI>` (from KOMPACT_WALLPAPER) shows that image, scaled to cover;
// without it, or if the image can't be loaded, the CSS gradient stays.
"use strict";

const image = new URLSearchParams(location.search).get("image");
const el = /** @type {HTMLDivElement} */ (document.getElementById("wallpaper"));

if (image) {
  // Load it first, so a bad path keeps the gradient instead of a blank screen.
  const probe = new Image();
  probe.onload = () => {
    el.classList.remove("gradient");
    el.style.backgroundImage = `url(${JSON.stringify(image)})`;
    console.log(`wallpaper: image ${image} (${probe.naturalWidth}x${probe.naturalHeight})`);
  };
  probe.onerror = () => console.log(`wallpaper: could not load ${image}, keeping the gradient`);
  probe.src = image;
} else {
  console.log("wallpaper: gradient");
}
