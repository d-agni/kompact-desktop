// @ts-check
// The dock (6m-d, 6m-d2): Launchpad, one icon per app (pinned, then other running apps), a
// separator and Trash. Built from `dock_items` (installed apps + pinned ids) and kompact's window
// list (`kompact-windows`, 6m-c); redrawn only when what it shows changes. Clicks on apps go to
// `dock_click`, which decides launch / bring forward / cycle (src-tauri/src/dock.rs, tested).
//
// 6m-d2, like macOS:
// - icons grow under the pointer (Magnification) and show their name above them;
// - an app with several windows shows them above its icon when hovered (cards; a click brings
//   one forward, × closes it); dots under the icon count its windows (up to three);
// - scrolling over an icon cycles its windows; middle click opens a new window;
// - pinned icons can be dragged into a new order, or up off the dock to unpin them;
// - the dock hides while the focused window is maximized or full screen (or always, see its
//   right-click menu) and comes back while the pointer is at the bottom edge.
// The surface is DOCK_SURFACE_HEIGHT tall but takes the pointer only where the dock is
// (`dock_surface` sets the input region), so the see-through part never blocks a window.
"use strict";

/** @typedef {{id: string, name: string, icon: boolean}} App */
/** @typedef {{id: number, app_id: string, title: string, name: string, desktop_id: string | null,
 *   workspace: number, minimized: boolean, focused: boolean}} Win */
/** @typedef {{key: string, desktopId: string | null, name: string, pinned: boolean,
 *   launching: boolean, wins: Win[]}} AppTile */
/** @typedef {{el: HTMLElement, lift: HTMLElement, cx: number, app: AppTile | null}} Tile */

// @ts-ignore: injected by Tauri (app.withGlobalTauri).
const tauri = window.__TAURI__;
/** @param {string} id */
const $ = (id) => /** @type {HTMLElement} */ (document.getElementById(id));
const dock = $("dock");
const label = $("label");
const previews = $("previews");

// 10d-1b: from the Size and Magnification settings (`applyPrefs`); dock.css follows --icon.
let ICON = 50; // an icon's size at rest
let TILE = 58; // a tile's width at rest (dock.css: icon + 8)
let MAX_SCALE = 1.5; // the icon under the pointer grows from 50 to 75 px
let REACH = 2.6 * TILE; // how far from the pointer icons still grow
let BAND = 34; // how far above the bar magnified icons reach
let SLACK = 64; // how much wider each side the magnified bar can get
const PREVIEW_DELAY = 450;
const EDGE = 3; // a hidden dock comes back while the pointer is this close to the bottom edge
const REVEAL_DELAY = 120;
const REVEAL_LINGER = 1200; // a revealed dock goes again this long after the pointer leaves the edge
/** Mouse moves this soon after GTK said the pointer left were queued in WebKit before it left. */
const STALE_MS = 400;

/** @type {Map<string, App>} */
let apps = new Map();
/** @type {string[]} */
let pinned = [];
/** @type {Win[]} */
let windows = [];
let activeWs = 1;
/** The focused window is maximized or full screen (kompact-shell works it out). */
let fills = false;
/** @type {{hide: "smart" | "never" | "always", magnify: boolean, follow: boolean, size: number,
 *   zoom: number, dots: boolean, bounce: boolean, position: "bottom" | "left" | "right"}} */
let prefs = { hide: "smart", magnify: true, follow: true, size: 50, zoom: 150, dots: true, bounce: true, position: "bottom" };
/** 10d-1c: the edge the dock is on. On the left or right the icons run down the screen: the
 * "main" axis (magnification, order, `Tile.cx`) is y there, x at the bottom. */
let SIDE = "bottom";
const vertical = () => SIDE !== "bottom";
/** Where this surface's corner is on its monitor; it always touches the bottom-right one. */
const surfaceOffset = () => ({ top: window.screen.height - window.innerHeight, left: window.screen.width - window.innerWidth });
/** From the screen edge to the icon's middle across the dock: the bar 4 px in, its 1 px border,
 * the icon 9 px in (dock.css). */
const iconInset = () => 4 + 1 + 9 + ICON / 2;
let trashFull = false;
/** Apps clicked to launch, shown as running until a window appears (at most 10 s). */
const launching = new Map();
/** Running apps that aren't pinned, in the order they first appeared (stays put on focus). */
/** @type {string[]} */
let runningOrder = [];
/** @type {AppTile[]} */
let current = [];
let drawn = "";
/** @type {Tile[]} */
let tiles = [];
/** The bar at rest (no magnification), in page pixels. */
let base = { left: 0, top: 0, width: 0, height: 0 };

let pointerX = /** @type {number | null} */ (null);
/** The pointer is on the dock: the bar, the band above it, or the previews. */
let over = false;
/** Shown over a hidden dock because the pointer reached the bottom edge. */
let revealed = false;
/** Our right-click menu is up (drawn by the overlay). */
let menuOpen = false;
/** @type {Tile | null} */
let hovered = null;
/** @type {string | null} the app whose windows are showing */
let preview = null;
let revealTimer = 0;
/** What GTK last said: the pointer is on the dock surface (see kompactDockPointer). */
let gtkIn = true;
let leftAt = 0;
let leaveTimer = 0;
let previewTimer = 0;
let lastWheel = 0;
/** @type {{tile: Tile, x: number, y: number, moved: boolean, remove: boolean} | null} */
let drag = null;
let suppressClick = false;

/** @param {string} cmd @param {Record<string, unknown>} [args] @returns {Promise<any>} */
function invoke(cmd, args) {
  if (!tauri) return Promise.resolve(undefined);
  return tauri.core.invoke(cmd, args).catch((/** @type {unknown} */ e) => console.log(`dock: ${cmd} failed: ${e}`));
}

/** A stable colour per app for its letter badge. @param {string} s */
function hue(s) {
  let h = 0;
  for (const c of s) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return h % 360;
}

/** @param {string} key @param {string} name */
function badge(key, name) {
  const b = document.createElement("div");
  b.className = "icon badge";
  const h = hue(key);
  b.style.background = `linear-gradient(160deg, hsl(${h} 55% 55%), hsl(${h} 55% 38%))`;
  b.textContent = (name.trim()[0] || "?").toUpperCase();
  return b;
}

/** @param {string} id a desktop id, or one of the dock's own (@trash) */
function iconUrl(id) {
  return `kompact-icon://localhost/${encodeURIComponent(id)}`;
}

/** @param {string | null} desktopId @param {string} name */
function icon(desktopId, name) {
  const app = desktopId ? apps.get(desktopId) : undefined;
  if (!desktopId || !app || !app.icon) return badge(desktopId || name, name);
  const img = document.createElement("img");
  img.className = "icon";
  img.alt = "";
  img.decoding = "async";
  img.draggable = false;
  img.src = iconUrl(desktopId);
  img.addEventListener("error", () => img.replaceWith(badge(desktopId, name)), { once: true });
  return img;
}

/** @param {Win} w */
const keyOf = (w) => w.desktop_id || `app:${w.app_id}`;

/** What the dock shows, as data. @returns {AppTile[]} */
function model() {
  const now = Date.now();
  for (const [id, t] of launching) if (now - t > 10000) launching.delete(id);
  /** @type {Map<string, Win[]>} */
  const byApp = new Map();
  for (const w of windows) {
    const list = byApp.get(keyOf(w));
    if (list) list.push(w);
    else byApp.set(keyOf(w), [w]);
  }
  for (const k of byApp.keys()) launching.delete(k);
  runningOrder = runningOrder.filter((k) => byApp.has(k));
  for (const k of byApp.keys()) if (!runningOrder.includes(k)) runningOrder.push(k);
  /** @type {AppTile[]} */
  const out = pinned.map((id) => ({
    key: id,
    desktopId: id,
    name: apps.get(id)?.name || id,
    pinned: true,
    launching: launching.has(id),
    wins: byApp.get(id) || [],
  }));
  for (const k of runningOrder) {
    if (pinned.includes(k)) continue;
    const wins = /** @type {Win[]} */ (byApp.get(k));
    out.push({ key: k, desktopId: wins[0].desktop_id, name: wins[0].name, pinned: false, launching: false, wins });
  }
  return out;
}

/** @param {string} name @param {string} cls @param {AppTile | null} app @returns {Tile} */
function makeTile(name, cls, app) {
  const el = document.createElement("button");
  el.className = `tile ${cls}`;
  el.setAttribute("aria-label", name);
  el.dataset.name = name;
  const lift = document.createElement("div");
  lift.className = "lift";
  el.append(lift);
  /** @type {Tile} */
  const t = { el, lift, cx: 0, app };
  el.addEventListener("mouseenter", () => hoverTile(t));
  el.addEventListener("mouseleave", () => {
    if (hovered === t) hovered = null;
    clearTimeout(previewTimer);
    placeLabel();
  });
  return t;
}

/** @param {number} n */
function dots(n) {
  const d = document.createElement("div");
  d.className = "dots";
  for (let i = 0; i < Math.min(n, 3); i++) d.append(document.createElement("i"));
  return d;
}

function render() {
  const m = model();
  current = m;
  sendIcons();
  if (preview) fillPreview();
  const sig =
    JSON.stringify(m.map((t) => [t.key, t.pinned, t.launching, Math.min(t.wins.length, 3), t.desktopId, t.name])) +
    apps.size + trashFull;
  // Not while an icon is being dragged: the drop redraws.
  if (sig === drawn || drag) return;
  drawn = sig;

  /** @type {HTMLElement[]} */
  const out = [];
  tiles = [];
  const lp = makeTile("Launchpad", "launchpad", null);
  const lpIcon = document.createElement("div");
  lpIcon.className = "icon badge";
  for (const c of ["#ff6159", "#ffbd2e", "#28c941", "#0a84ff", "#bf5af2", "#ff9f0a", "#64d2ff", "#ff375f", "#30d158"]) {
    const i = document.createElement("i");
    i.style.background = c;
    lpIcon.append(i);
  }
  lp.lift.append(lpIcon);
  lp.el.addEventListener("click", () => invoke("toggle_overlay", { mode: "launchpad" }));
  tiles.push(lp);
  out.push(lp.el);

  for (const a of m) {
    const cls = [a.wins.length || a.launching ? "running" : "", a.launching ? "launching" : "", a.pinned ? "pinned" : ""];
    const t = makeTile(a.name, cls.join(" "), a);
    t.lift.append(icon(a.desktopId, a.name));
    t.el.append(dots(a.launching && !a.wins.length ? 1 : a.wins.length));
    t.el.dataset.key = a.key;
    // The second click of a double click would minimize what the first brought up.
    t.el.addEventListener("click", (e) => e.detail < 2 && clickApp(a));
    t.el.addEventListener("auxclick", (e) => {
      // Middle click: a new window, as Ubuntu's dock does.
      if (e.button === 1 && a.desktopId) invoke("dock_open", { desktopId: a.desktopId });
    });
    t.el.addEventListener("contextmenu", (e) => {
      e.preventDefault();
      e.stopPropagation();
      if (a.desktopId) openMenu(t.cx, a.desktopId);
    });
    t.el.addEventListener("wheel", (e) => {
      e.preventDefault();
      const now = Date.now();
      if (!a.desktopId || !a.wins.length || now - lastWheel < 250) return;
      lastWheel = now;
      invoke("dock_cycle", { desktopId: a.desktopId, forward: e.deltaY > 0 });
    }, { passive: false });
    if (a.pinned) t.el.addEventListener("pointerdown", (e) => startDrag(e, t));
    tiles.push(t);
    out.push(t.el);
  }

  const sep = document.createElement("div");
  sep.className = "sep";
  out.push(sep);
  const trash = makeTile("Trash", "trash", null);
  const trashImg = document.createElement("img");
  trashImg.className = "icon";
  trashImg.alt = "";
  trashImg.draggable = false;
  trashImg.src = iconUrl(trashFull ? "@trash-full" : "@trash");
  trashImg.addEventListener("error", () => trashImg.replaceWith(badge("trash", "T")), { once: true });
  trash.lift.append(trashImg);
  trash.el.addEventListener("click", () => invoke("dock_open_trash"));
  trash.el.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    e.stopPropagation();
    openMenu(trash.cx, "@trash");
  });
  tiles.push(trash);
  out.push(trash.el);

  dock.replaceChildren(...out);
  const hoveredKey = hovered?.app?.key;
  hovered = (hoveredKey && tiles.find((t) => t.app?.key === hoveredKey)) || null;
  measure();
  layout();
  sendIcons();
  const running = m.filter((t) => t.wins.length).length;
  console.log(`dock: ${m.length} apps (${running} running, ${windows.length} windows), trash ${trashFull ? "full" : "empty"}`);
}

// 11a: kompact shrinks a minimized window into its icon (and grows it back out): tell it where
// each window's icon is at rest, in this surface's coordinates, whenever that changes.
let iconsSig = "";
function sendIcons() {
  const at = new Map(tiles.filter((t) => t.app).map((t) => [t.app?.key, Math.round(t.cx)]));
  // The icon's middle across the dock (10d-1c: from whichever edge it's on).
  const cross = Math.round(
    SIDE === "bottom" ? window.innerHeight - iconInset() : SIDE === "left" ? iconInset() : window.innerWidth - iconInset(),
  );
  /** @type {number[][]} */
  const icons = [];
  for (const a of current) {
    const along = at.get(a.key);
    if (along === undefined) continue;
    for (const w of a.wins) icons.push(vertical() ? [w.id, cross, along] : [w.id, along, cross]);
  }
  const sig = JSON.stringify([ICON, SIDE, icons]);
  if (sig === iconsSig || !tiles.length) return;
  iconsSig = sig;
  invoke("dock_icons", { size: ICON, icons: icons.slice(0, 256) });
}

/** Where everything is at rest, for magnification and the input region. */
function measure() {
  dock.classList.add("still");
  for (const t of tiles) {
    t.el.style.width = t.el.style.height = "";
    t.lift.style.transform = "";
  }
  const v = vertical();
  const start = v ? dock.offsetTop : dock.offsetLeft;
  for (const t of tiles) t.cx = start + (v ? t.el.offsetTop + t.el.offsetHeight / 2 : t.el.offsetLeft + t.el.offsetWidth / 2);
  base = { left: dock.offsetLeft, top: dock.offsetTop, width: dock.offsetWidth, height: dock.offsetHeight };
  void dock.offsetWidth; // apply "still" before it's removed
  dock.classList.remove("still");
}

/** @param {AppTile} tile */
function clickApp(tile) {
  // The tile's closure is from the last redraw, which minimizing or focusing doesn't cause: use
  // the app as it is now.
  const a = current.find((x) => x.key === tile.key) || tile;
  if (suppressClick) return console.log(`dock: click on ${a.name} ignored (end of a drag)`);
  const min = a.wins.filter((w) => w.minimized).length;
  console.log(`dock: click on ${a.name} (${a.wins.length} windows, ${min} minimized)`);
  if (a.desktopId) {
    if (!a.wins.length) {
      launching.set(a.desktopId, Date.now());
      render();
    }
    invoke("dock_click", { desktopId: a.desktopId });
  } else {
    // An app we have no desktop file for (Settings): as `click_target` in dock.rs, in front →
    // minimize its focused window, else bring its last window forward.
    const front = a.wins.find((w) => w.focused && !w.minimized && w.workspace === activeWs);
    const w = front || a.wins.filter((x) => !x.minimized).pop() || a.wins[a.wins.length - 1];
    if (w) invoke("window_action", { action: front ? "minimize" : "activate", id: w.id });
  }
  closePreview();
}

/** Right-click: a menu drawn by the overlay, next to the icon at `along` (page px on the main
 * axis): centred above it, or (10d-1c) beside it on a dock at the side.
 * @param {number} along @param {string} desktopId an app, or "@dock" / "@trash" */
function openMenu(along, desktopId) {
  menuOpen = true;
  closePreview();
  hovered = null;
  layout();
  // Screen (monitor) coordinates: the surface's corner plus the page's.
  const off = surfaceOffset();
  const x = vertical() ? 0 : along + off.left;
  const y = vertical() ? along + off.top : 0;
  invoke("show_dock_menu", { desktopId, x, y });
}
// Between the icons: the dock's own settings.
dock.addEventListener("contextmenu", (e) => {
  e.preventDefault();
  openMenu(vertical() ? e.clientY : e.clientX, "@dock");
});
document.addEventListener("contextmenu", (e) => e.preventDefault());

// ---- Magnification, the label, the input region ----

let frame = 0;
function schedule() {
  if (!frame) {
    frame = requestAnimationFrame(() => {
      frame = 0;
      layout();
    });
  }
}

function layout() {
  const x = over && prefs.magnify && !preview && !drag && !menuOpen && visible() ? pointerX : null;
  for (const t of tiles) {
    let s = 1;
    if (x !== null) {
      const d = Math.abs(x - t.cx);
      if (d < REACH) s = 1 + (MAX_SCALE - 1) * Math.cos(((d / REACH) * Math.PI) / 2);
    }
    const w = s === 1 ? "" : `${(TILE * s).toFixed(1)}px`;
    const style = t.el.style;
    if ((vertical() ? style.height : style.width) !== w) {
      if (vertical()) style.height = w;
      else style.width = w;
      t.lift.style.transform = s === 1 ? "" : `scale(${s.toFixed(3)})`;
    }
  }
  placeLabel();
  placeVibrancy();
  updateSurface();
}

function placeLabel() {
  const t = hovered;
  if (!t || preview || drag?.moved || menuOpen || !visible()) {
    label.hidden = true;
    return;
  }
  label.textContent = t.el.dataset.name || "";
  label.hidden = false;
  // Above the icon (10d-1c: beside it) as it will be once it has grown (the transition may be
  // under way).
  const r = t.lift.getBoundingClientRect();
  const s = t.lift.style.transform ? parseFloat(t.lift.style.transform.slice(6)) : 1;
  const w = label.offsetWidth;
  const h = label.offsetHeight;
  if (SIDE === "bottom") {
    const iconTop = r.bottom - ICON * s;
    const cx = r.left + r.width / 2;
    label.style.left = `${Math.max(4, Math.min(window.innerWidth - w - 4, cx - w / 2))}px`;
    label.style.top = `${iconTop - h - 10}px`;
  } else {
    const cy = r.top + r.height / 2;
    const x = SIDE === "left" ? r.left + ICON * s + 10 : r.right - ICON * s - 10 - w;
    label.style.left = `${x}px`;
    label.style.top = `${Math.max(4, Math.min(window.innerHeight - h - 4, cy - h / 2))}px`;
  }
}

/** Hidden by the setting / a maximized window (it may still show while the pointer is on it). */
function hiddenByPolicy() {
  return prefs.hide === "always" || (prefs.hide === "smart" && fills);
}

function visible() {
  return !hiddenByPolicy() || revealed || over || menuOpen || drag !== null || preview !== null;
}

let shownBefore = true;
function update() {
  const show = visible();
  dock.classList.toggle("hidden", !show);
  if (show !== shownBefore) {
    shownBefore = show;
    console.log(`dock: ${show ? "shown" : "hidden"} (hide ${prefs.hide}, focused window fills the screen: ${fills})`);
  }
  schedule();
}

let surfaceSig = "";
/** Tell kompact-shell what to reserve and where to take the pointer. */
function updateSurface() {
  const W = window.innerWidth;
  const H = window.innerHeight;
  const reserve = !hiddenByPolicy();
  /** @type {number[][]} */
  let rects;
  // The strip along the dock's edge (10d-1c: whichever it is).
  const edge = SIDE === "bottom" ? [0, H - EDGE, W, EDGE] : SIDE === "left" ? [0, 0, EDGE, H] : [W - EDGE, 0, EDGE, H];
  if (drag) {
    rects = [[0, 0, W, H]];
  } else if (!visible()) {
    rects = [edge];
  } else {
    const grow = over && prefs.magnify;
    const band = grow ? BAND : 0;
    const slack = grow ? SLACK : 0;
    const r = preview ? previews.getBoundingClientRect() : null;
    if (SIDE === "bottom") {
      const top = base.top - band;
      const left = Math.max(0, base.left - slack);
      rects = [[left, top, Math.min(W, base.left + base.width + slack) - left, H - top]];
      if (r) rects.push([r.left, r.top, r.width, base.top - r.top]);
    } else {
      const top = Math.max(0, base.top - slack);
      const h = Math.min(H, base.top + base.height + slack) - top;
      if (SIDE === "left") {
        const right = base.left + base.width + band;
        rects = [[0, top, right, h]];
        if (r) rects.push([base.left + base.width, r.top, r.right - (base.left + base.width), r.height]);
      } else {
        const left = base.left - band;
        rects = [[left, top, W - left, h]];
        if (r) rects.push([r.left, r.top, base.left - r.left, r.height]);
      }
    }
    // Shown over a maximized window: the whole edge still keeps it up.
    if (hiddenByPolicy()) rects.push(edge);
  }
  rects = rects.map((r) => r.map(Math.round));
  // 10d-1b: the bar (icon + 14) with 6 px around it, as `DockPrefs::zone` in dock.rs.
  const zone = ICON + 20;
  const sig = JSON.stringify([reserve, rects, zone]);
  if (sig === surfaceSig) return;
  surfaceSig = sig;
  invoke("dock_surface", { reserve, rects, zone });
}

/** @param {number} x @param {number} y */
function inDock(x, y) {
  const r = preview ? previews.getBoundingClientRect() : null;
  if (SIDE === "bottom") {
    if (y >= base.top - BAND && x >= base.left - SLACK && x <= base.left + base.width + SLACK) return true;
    return !!r && x >= r.left && x <= r.right && y >= r.top && y <= base.top;
  }
  const along = y >= base.top - SLACK && y <= base.top + base.height + SLACK;
  if (SIDE === "left") {
    const edge = base.left + base.width;
    if (along && x <= edge + BAND) return true;
    return !!r && y >= r.top && y <= r.bottom && x >= edge && x <= r.right;
  }
  if (along && x >= base.left - BAND) return true;
  return !!r && y >= r.top && y <= r.bottom && x <= base.left && x >= r.left;
}

/** @param {number | null} x @param {number | null} y */
function onPointer(x, y) {
  if (drag) return;
  const inside = x !== null && y !== null && visible() && inDock(x, y);
  // On the main axis (10d-1c: y on a dock at the side).
  pointerX = inside ? (vertical() ? y : x) : null;
  if (inside !== over) {
    over = inside;
    if (inside) {
      clearTimeout(leaveTimer);
      if (preview) clearTimeout(previewTimer);
      void checkTrash();
    } else {
      hovered = null;
      clearTimeout(previewTimer);
      if (preview) previewTimer = setTimeout(closePreview, 300);
      clearTimeout(leaveTimer);
      leaveTimer = setTimeout(endReveal, 500);
    }
    update();
  } else if (inside) {
    schedule();
  }
  atEdge(x, y);
}

function endReveal() {
  revealed = false;
  update();
}

/** A hidden dock comes back while the pointer rests at its edge.
 * @param {number | null} x @param {number | null} y */
function atEdge(x, y) {
  const on =
    x !== null &&
    y !== null &&
    (SIDE === "bottom" ? y >= window.innerHeight - EDGE : SIDE === "left" ? x <= EDGE : x >= window.innerWidth - EDGE);
  const edge = on && hiddenByPolicy();
  if (!edge) {
    clearTimeout(revealTimer);
    revealTimer = 0;
    return;
  }
  if (revealed) {
    // Still at the edge: keep it up (it goes REVEAL_LINGER after the pointer leaves the edge).
    if (!over) {
      clearTimeout(leaveTimer);
      leaveTimer = setTimeout(endReveal, REVEAL_LINGER);
    }
    return;
  }
  if (revealTimer) return;
  revealTimer = setTimeout(() => {
    revealTimer = 0;
    revealed = true;
    console.log("dock: revealed (pointer at the bottom edge)");
    clearTimeout(leaveTimer);
    if (!over) leaveTimer = setTimeout(endReveal, REVEAL_LINGER);
    update();
  }, REVEAL_DELAY);
}

/** Not a move WebKit had queued from before GTK said the pointer left. */
function trusted() {
  return gtkIn || Date.now() - leftAt >= STALE_MS;
}

document.addEventListener("mousemove", (e) => {
  if (!trusted()) return;
  gtkIn = true;
  onPointer(e.clientX, e.clientY);
});
document.documentElement.addEventListener("mouseleave", () => onPointer(null, null));
document.addEventListener("mouseout", (e) => {
  if (!e.relatedTarget) onPointer(null, null);
});
// kompact-shell calls this on GTK's enter/leave-notify (the wl_pointer enter/leave from
// kompact). WebKit doesn't always turn a leave into a DOM event, and a mouse move it had queued
// can arrive after this, which left icons magnified and the dock up over maximized windows.
// @ts-ignore: our own global.
window.kompactDockPointer = (/** @type {boolean} */ inside, /** @type {number} */ x, /** @type {number} */ y) => {
  if (inside) {
    gtkIn = true;
    onPointer(x, y);
    return;
  }
  gtkIn = false;
  leftAt = Date.now();
  clearTimeout(revealTimer);
  revealTimer = 0;
  if (over && !drag) console.log("dock: pointer left (GTK)");
  onPointer(null, null);
};

// ---- Window previews ----

/** @param {Tile} t */
function hoverTile(t) {
  if (!trusted()) return;
  hovered = t;
  clearTimeout(previewTimer);
  const many = (t.app?.wins.length || 0) >= 2;
  if (preview && preview !== t.app?.key) {
    // Moving along the dock with previews up: show this app's at once, or none.
    if (many && t.app) openPreview(t.app.key);
    else closePreview();
  } else if (many && !preview && t.app) {
    const key = t.app.key;
    previewTimer = setTimeout(() => hovered === t && over && openPreview(key), PREVIEW_DELAY);
  }
  schedule();
}

/** @param {string} key */
function openPreview(key) {
  preview = key;
  fillPreview();
  if (!preview) return;
  const a = current.find((x) => x.key === key);
  console.log(`dock: windows of ${a?.name} (${a?.wins.length})`);
  requestThumbs();
}

// 6m-d3: pictures of the windows, from kompact (unless KOMPACT_THUMBNAILS=off), asked for when a
// preview opens and every 2 s while it stays open.
/** @type {Map<number, number>} window id → version of its latest thumbnail */
const thumbVersions = new Map();
let thumbTimer = 0;
function requestThumbs() {
  clearTimeout(thumbTimer);
  const a = current.find((x) => x.key === preview);
  if (!a) return;
  invoke("request_thumbs", { ids: a.wins.map((w) => w.id) });
  thumbTimer = setTimeout(requestThumbs, 2000);
}

/** @param {HTMLElement} thumb @param {number} id */
function setShot(thumb, id) {
  const v = thumbVersions.get(id);
  if (v === undefined) return;
  let img = /** @type {HTMLImageElement | null} */ (thumb.querySelector("img.shot"));
  if (!img) {
    img = document.createElement("img");
    img.className = "shot";
    img.alt = "";
    img.draggable = false;
    img.addEventListener("error", () => img?.remove());
    img.addEventListener("load", () => thumb.classList.add("has-shot"));
    thumb.prepend(img);
  }
  img.src = `kompact-thumb://localhost/${id}?v=${v}`;
}

/** @param {{id: number, version: number}} t */
function onThumb(t) {
  thumbVersions.set(t.id, t.version);
  const thumb = /** @type {HTMLElement | null} */ (previews.querySelector(`.card[data-id="${t.id}"] .thumb`));
  if (thumb) setShot(thumb, t.id);
}

function fillPreview() {
  const a = current.find((x) => x.key === preview);
  if (!a || a.wins.length < 2) return closePreview();
  const cards = a.wins.map((w) => {
    const c = document.createElement("div");
    c.className = `card${w.focused ? " focused" : ""}${w.minimized ? " min" : ""}`;
    c.dataset.id = String(w.id);
    const thumb = document.createElement("div");
    thumb.className = "thumb";
    const h = hue(a.key);
    thumb.style.background = `linear-gradient(160deg, hsl(${h} 30% 34%), hsl(${h} 30% 18%))`;
    thumb.append(icon(a.desktopId, a.name));
    setShot(thumb, w.id);
    const tag = w.minimized ? "Minimized" : w.workspace !== activeWs ? `Desktop ${w.workspace}` : "";
    if (tag) {
      const t = document.createElement("span");
      t.className = "tag";
      t.textContent = tag;
      thumb.append(t);
    }
    const x = document.createElement("button");
    x.className = "x";
    x.textContent = "×";
    x.setAttribute("aria-label", "Close");
    x.addEventListener("click", (e) => {
      e.stopPropagation();
      invoke("window_action", { action: "close", id: w.id });
    });
    const title = document.createElement("div");
    title.className = "title";
    title.textContent = w.title || a.name;
    c.append(thumb, x, title);
    c.addEventListener("click", () => {
      invoke("window_action", { action: "activate", id: w.id });
      closePreview();
    });
    c.addEventListener("auxclick", (e) => {
      if (e.button === 1) invoke("window_action", { action: "close", id: w.id });
    });
    return c;
  });
  previews.replaceChildren(...cards);
  previews.hidden = false;
  label.hidden = true;
  // Centred above the app's icon, kept on screen.
  const t = tiles.find((x) => x.app?.key === a.key);
  const w = previews.offsetWidth;
  if (SIDE === "bottom") {
    const cx = t ? t.cx : window.innerWidth / 2;
    previews.style.left = `${Math.max(8, Math.min(window.innerWidth - w - 8, cx - w / 2))}px`;
    previews.style.top = "";
    previews.style.bottom = `${window.innerHeight - base.top + 10}px`;
  } else {
    // 10d-1c: beside the icon, kept on screen.
    const cy = t ? t.cx : window.innerHeight / 2;
    const h = previews.offsetHeight;
    previews.style.bottom = "";
    previews.style.top = `${Math.max(8, Math.min(window.innerHeight - h - 8, cy - h / 2))}px`;
    previews.style.left = `${SIDE === "left" ? base.left + base.width + 10 : base.left - 10 - w}px`;
  }
  update();
}

function closePreview() {
  clearTimeout(previewTimer);
  clearTimeout(thumbTimer);
  if (!preview) return;
  preview = null;
  previews.hidden = true;
  previews.replaceChildren();
  update();
}

// ---- Dragging pinned icons ----

/** @param {PointerEvent} e @param {Tile} t */
function startDrag(e, t) {
  if (e.button !== 0) return;
  drag = { tile: t, x: e.clientX, y: e.clientY, moved: false, remove: false };
}

document.addEventListener("pointermove", (e) => {
  if (!drag) return;
  const { tile } = drag;
  if (!drag.moved) {
    if (Math.hypot(e.clientX - drag.x, e.clientY - drag.y) < 6) return;
    drag.moved = true;
    closePreview();
    tile.el.classList.add("dragging");
    console.log(`dock: dragging ${tile.app?.name}`);
    layout();
  }
  // Slide into the pinned icons where the pointer is.
  const mine = tiles.filter((t) => t.app?.pinned && t !== tile);
  const v = vertical();
  const along = v ? e.clientY : e.clientX;
  const before = mine.find((t) => along < (v ? t.el.offsetTop + base.top : t.el.offsetLeft + base.left) + TILE / 2);
  if (before && tile.el.nextElementSibling !== before.el) dock.insertBefore(tile.el, before.el);
  else if (!before && mine.length) {
    const last = mine[mine.length - 1].el;
    if (last.nextElementSibling !== tile.el) last.after(tile.el);
  }
  const slot = (v ? dock.offsetTop + tile.el.offsetTop : dock.offsetLeft + tile.el.offsetLeft) + TILE / 2;
  // Dragged well away from the dock's edge: off the dock.
  drag.remove =
    SIDE === "bottom" ? e.clientY < base.top - 70 : SIDE === "left" ? e.clientX > base.left + base.width + 70 : e.clientX < base.left - 70;
  tile.el.classList.toggle("remove", drag.remove);
  tile.el.style.transform = v
    ? `translate(${e.clientX - drag.x}px, ${along - slot}px)`
    : `translate(${along - slot}px, ${e.clientY - drag.y}px)`;
});

document.addEventListener("pointerup", () => {
  if (!drag) return;
  const d = drag;
  drag = null;
  if (!d.moved) return;
  // The click that follows the drop isn't one.
  suppressClick = true;
  setTimeout(() => (suppressClick = false), 0);
  d.tile.el.classList.remove("dragging", "remove");
  d.tile.el.style.transform = "";
  const id = d.tile.app?.desktopId;
  if (d.remove && id) {
    console.log(`dock: dragged ${id} off the dock`);
    invoke("set_pinned", { desktopId: id, pinned: false });
  } else {
    const order = [...dock.querySelectorAll(".tile.pinned")].map((el) => /** @type {HTMLElement} */ (el).dataset.key);
    if (JSON.stringify(order) !== JSON.stringify(pinned)) {
      console.log(`dock: new order ${order.join(" ")}`);
      pinned = /** @type {string[]} */ (order);
      invoke("set_pin_order", { order });
    }
  }
  drawn = "";
  render();
  update();
});

// ---- Data ----

/** @param {any} msg */
function onWindows(msg) {
  windows = Array.isArray(msg?.windows) ? msg.windows : [];
  for (const id of thumbVersions.keys()) if (!windows.some((w) => w.id === id)) thumbVersions.delete(id);
  activeWs = typeof msg?.workspace === "number" ? msg.workspace : 1;
  const wasFills = fills;
  fills = msg?.focused_fills === true;
  // A window of an app we don't know yet (installed since start): reload the app list.
  if (windows.some((w) => w.desktop_id && !apps.has(w.desktop_id)) && Date.now() - loadedAt > 10000) void loadItems();
  render();
  if (fills !== wasFills) update();
}

let loadedAt = 0;

async function loadItems() {
  loadedAt = Date.now();
  const items = await invoke("dock_items");
  if (!items) return;
  apps = new Map(items.apps.map((/** @type {App} */ a) => [a.id, a]));
  pinned = items.pinned;
  render();
}

async function loadPrefs() {
  const p = await invoke("dock_prefs");
  if (!p) return;
  prefs = { ...prefs, ...p };
  applyPrefs();
  console.log(
    `dock: settings hide=${prefs.hide} magnify=${prefs.magnify} size=${ICON} zoom=${prefs.zoom}% dots=${prefs.dots} bounce=${prefs.bounce} position=${SIDE}`,
  );
  update();
}

/** 10d-1b: sizes from the settings; the bar is measured again if the size changed. */
function applyPrefs() {
  const size = Math.max(32, Math.min(64, Number(prefs.size) || 50));
  MAX_SCALE = Math.max(1.1, Math.min(2, (Number(prefs.zoom) || 150) / 100));
  document.body.classList.toggle("no-dots", !prefs.dots);
  document.body.classList.toggle("no-bounce", !prefs.bounce);
  const side = prefs.position === "left" || prefs.position === "right" ? prefs.position : "bottom";
  const resized = size !== ICON || side !== SIDE;
  SIDE = side;
  document.documentElement.dataset.side = side;
  ICON = size;
  TILE = ICON + 8;
  REACH = 2.6 * TILE;
  // How far magnified icons rise above the bar, and how much wider it gets each side (the
  // cosine bump over REACH adds about 1.65 × TILE × (MAX_SCALE − 1); with room to spare).
  BAND = Math.ceil(ICON * (MAX_SCALE - 1)) + 9;
  SLACK = Math.ceil((MAX_SCALE - 1) * TILE * 2.2) + 2;
  document.documentElement.style.setProperty("--icon", `${ICON}px`);
  if (resized) {
    measure();
    placeVibrancy();
    sendIcons();
  }
}

async function checkTrash() {
  const full = (await invoke("dock_trash_full")) === true;
  if (full !== trashFull) {
    trashFull = full;
    render();
  }
}

// @ts-ignore: from vibrancy.js. Where this surface is on the monitor changes with its edge.
const placeVibrancy = kompactVibrancy(dock, surfaceOffset);
// Slides and magnification move the bar: place the frosted background again once they settle.
dock.addEventListener("transitionend", () => placeVibrancy());
window.addEventListener("resize", () => {
  measure();
  update();
});

if (tauri) {
  tauri.event.listen("kompact-windows", (/** @type {{payload: any}} */ e) => onWindows(e.payload));
  // "Keep in Dock", a drag or the dock's settings changed.
  tauri.event.listen("kompact-dock-changed", () => {
    void loadItems();
    void loadPrefs();
  });
  tauri.event.listen("kompact-overlay-closed", () => {
    if (!menuOpen) return;
    menuOpen = false;
    clearTimeout(leaveTimer);
    leaveTimer = setTimeout(() => {
      revealed = false;
      update();
    }, 500);
    update();
  });
  tauri.event.listen("kompact-thumb", (/** @type {{payload: any}} */ e) => onThumb(e.payload));
  invoke("compositor_state").then(onWindows);
}
void loadItems();
void loadPrefs();
void checkTrash();
// Drop a "launching" dot that never got a window.
setInterval(() => {
  if (launching.size) render();
}, 5000);
console.log("dock: page ready");
