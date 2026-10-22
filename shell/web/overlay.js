// @ts-check
// The overlay page (6m-a): Launchpad, Spotlight and the logo menu (6m-b), one at a time. The shell shows the surface
// and calls `window.kompactOverlay.open(mode)`; the page asks for the apps (`list_apps`) and
// reports back with `launch {id}` or `close_overlay`.
"use strict";

/** @typedef {{id: string, name: string, comment: string, keywords: string, icon: boolean}} App */
/** @typedef {"launchpad" | "spotlight" | "menu" | "dockmenu" | "spaces" | "select" | "status" | "auth" | "switcher"} Mode */

const $ = (/** @type {string} */ id) => /** @type {HTMLElement} */ (document.getElementById(id));
const views = { launchpad: $("launchpad"), spotlight: $("spotlight"), menu: $("menu"), dockmenu: $("dockmenu"), spaces: $("spaces"), select: $("select"), status: $("status"), auth: $("auth"), switcher: $("switcher") };
const lpSearch = /** @type {HTMLInputElement} */ ($("lp-search"));
const lpGrid = $("lp-grid");
const lpEmpty = $("lp-empty");
const spSearch = /** @type {HTMLInputElement} */ ($("sp-search"));
const spResults = $("sp-results");

/** @type {Mode | null} */
let mode = null;
/** @type {App[]} */
let apps = [];

/** @param {string} cmd @param {Record<string, unknown>} [args] @returns {Promise<any>} */
function invoke(cmd, args) {
  // @ts-ignore: injected by Tauri (app.withGlobalTauri).
  const tauri = window.__TAURI__;
  if (!tauri) {
    console.log(`overlay: no Tauri IPC (${cmd})`);
    return Promise.resolve(undefined);
  }
  return tauri.core.invoke(cmd, args).catch((/** @type {unknown} */ e) => {
    console.log(`overlay: ${cmd} failed: ${e}`);
  });
}

/** Lower is better; -1 = no match. Name prefix, then a word in the name, then anywhere in the
 * name, then the comment or keywords.
 * @param {App} app @param {string} q lower-case query */
function score(app, q) {
  if (!q) return 0;
  const name = app.name.toLowerCase();
  if (name.startsWith(q)) return 0;
  if (name.split(/\s+/).some((w) => w.startsWith(q))) return 1;
  if (name.includes(q)) return 2;
  if (app.keywords.toLowerCase().includes(q) || app.comment.toLowerCase().includes(q) || app.id.toLowerCase().includes(q)) return 3;
  return -1;
}

/** @param {string} q */
function matching(q) {
  q = q.trim().toLowerCase();
  return apps
    .map((app, i) => ({ app, i, s: score(app, q) }))
    .filter((x) => x.s >= 0)
    .sort((a, b) => a.s - b.s || a.i - b.i)
    .map((x) => x.app);
}

/** A stable colour per app for its letter badge. @param {string} s */
function hue(s) {
  let h = 0;
  for (const c of s) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return h % 360;
}

/** A rounded-square letter badge, for apps without a usable icon. @param {App} app */
function badge(app) {
  const b = document.createElement("div");
  b.className = "icon badge";
  const h = hue(app.id);
  b.style.background = `linear-gradient(160deg, hsl(${h} 55% 55%), hsl(${h} 55% 38%))`;
  b.textContent = (app.name.trim()[0] || "?").toUpperCase();
  return b;
}

/** @param {App} app */
function iconFor(app) {
  if (!app.icon) return badge(app);
  const img = document.createElement("img");
  img.className = "icon";
  img.alt = "";
  img.decoding = "async";
  img.src = `kompact-icon://localhost/${encodeURIComponent(app.id)}`;
  img.addEventListener("error", () => img.replaceWith(badge(app)), { once: true });
  return img;
}

/** @param {App} app */
function launch(app) {
  console.log(`overlay: launch ${app.id}`);
  invoke("launch", { id: app.id });
}

function close() {
  invoke("close_overlay");
}

// ---- Launchpad ----

/** @type {App[]} */
let lpShown = [];
let lpSelected = 0;

function lpRender() {
  const q = lpSearch.value.trim();
  lpShown = matching(q);
  lpSelected = Math.min(lpSelected, Math.max(0, lpShown.length - 1));
  lpGrid.replaceChildren(
    ...lpShown.map((app, i) => {
      const li = document.createElement("li");
      li.setAttribute("role", "option");
      li.title = app.comment || app.name;
      if (i === lpSelected && q) li.className = "selected";
      const name = document.createElement("div");
      name.className = "name";
      name.textContent = app.name;
      li.append(iconFor(app), name);
      li.addEventListener("click", () => launch(app));
      return li;
    }),
  );
  lpEmpty.hidden = lpShown.length > 0;
}

/** How many icons fit in a row of the grid right now. */
function lpColumns() {
  return Math.max(1, getComputedStyle(lpGrid).gridTemplateColumns.split(" ").filter(Boolean).length);
}

/** @param {number} i */
function lpSelect(i) {
  if (lpShown.length === 0) return;
  lpSelected = Math.max(0, Math.min(lpShown.length - 1, i));
  lpGrid.querySelectorAll("li").forEach((li, j) => li.classList.toggle("selected", j === lpSelected));
  lpGrid.children[lpSelected]?.scrollIntoView({ block: "nearest" });
}

lpSearch.addEventListener("input", () => {
  lpSelected = 0;
  lpRender();
});

/** @param {KeyboardEvent} e */
function lpKey(e) {
  switch (e.key) {
    case "Escape":
      // Like Launchpad: Esc clears a search first, then closes.
      if (lpSearch.value) {
        lpSearch.value = "";
        lpSelected = 0;
        lpRender();
      } else close();
      break;
    case "ArrowRight": lpSelect(lpSelected + 1); break;
    case "ArrowLeft": lpSelect(lpSelected - 1); break;
    case "ArrowDown": lpSelect(lpSelected + lpColumns()); break;
    case "ArrowUp": lpSelect(lpSelected - lpColumns()); break;
    case "Enter":
      if (lpShown[lpSelected]) launch(lpShown[lpSelected]);
      break;
    default:
      return;
  }
  e.preventDefault();
}

// ---- Spotlight ----

/** One selectable result row: an app, or the calculator's answer.
 * @typedef {{kind: "app", app: App} | {kind: "calc", value: string}} Hit */
/** @type {Hit[]} */
let spHits = [];
let spSelected = 0;
/** Bumped on every keystroke, so a slow `calculate` answer for old text is dropped. */
let spSeq = 0;
const SP_MAX_APPS = 8;

async function spUpdate() {
  const text = spSearch.value;
  const seq = ++spSeq;
  if (!text.trim()) {
    spHits = [];
    spRender();
    return;
  }
  const value = /** @type {string | null | undefined} */ (await invoke("calculate", { text }));
  if (seq !== spSeq) return;
  /** @type {Hit[]} */
  const hits = [];
  if (value) hits.push({ kind: "calc", value });
  for (const app of matching(text).slice(0, SP_MAX_APPS)) hits.push({ kind: "app", app });
  spHits = hits;
  spSelected = 0;
  spRender();
}

function spRender() {
  const text = spSearch.value.trim();
  spResults.hidden = !text;
  if (!text) return spResults.replaceChildren();
  /** @type {HTMLElement[]} */
  const out = [];
  let section = "";
  spHits.forEach((hit, i) => {
    const head = hit.kind === "calc" ? "Calculator" : i === 0 || (i === 1 && spHits[0].kind === "calc") ? "Top Hit" : "Applications";
    if (head !== section) {
      section = head;
      const h = document.createElement("div");
      h.className = "sp-head";
      h.textContent = head;
      out.push(h);
    }
    const row = document.createElement("div");
    row.className = "sp-row" + (hit.kind === "calc" ? " calc" : "") + (i === spSelected ? " selected" : "");
    const label = document.createElement("span");
    label.className = "label";
    const kind = document.createElement("span");
    kind.className = "kind";
    if (hit.kind === "calc") {
      const b = document.createElement("div");
      b.className = "icon badge";
      b.style.background = "linear-gradient(160deg, #ff9f0a, #d9770a)";
      b.textContent = "=";
      label.textContent = `= ${hit.value}`;
      kind.textContent = "Calculator";
      row.append(b, label, kind);
    } else {
      label.textContent = hit.app.name;
      kind.textContent = hit.app.comment ? hit.app.comment : "Application";
      kind.style.maxWidth = "45%";
      kind.style.overflow = "hidden";
      kind.style.textOverflow = "ellipsis";
      kind.style.whiteSpace = "nowrap";
      row.append(iconFor(hit.app), label, kind);
      row.addEventListener("click", () => launch(hit.app));
    }
    row.addEventListener("mousemove", () => spSelect(i));
    out.push(row);
  });
  if (spHits.length === 0) {
    const none = document.createElement("div");
    none.className = "sp-none";
    none.textContent = "No Results";
    out.push(none);
  }
  spResults.replaceChildren(...out);
}

/** @param {number} i */
function spSelect(i) {
  if (spHits.length === 0 || i === spSelected) return;
  spSelected = Math.max(0, Math.min(spHits.length - 1, i));
  spResults.querySelectorAll(".sp-row").forEach((r, j) => r.classList.toggle("selected", j === spSelected));
  spResults.querySelectorAll(".sp-row")[spSelected]?.scrollIntoView({ block: "nearest" });
}

spSearch.addEventListener("input", () => void spUpdate());

/** @param {KeyboardEvent} e */
function spKey(e) {
  switch (e.key) {
    case "Escape":
      if (spSearch.value) {
        spSearch.value = "";
        void spUpdate();
      } else close();
      break;
    case "ArrowDown": spSelect(spSelected + 1); break;
    case "ArrowUp": spSelect(spSelected - 1); break;
    case "Enter": {
      const hit = spHits[spSelected];
      if (hit && hit.kind === "app") launch(hit.app);
      break;
    }
    default:
      return;
  }
  e.preventDefault();
}

// ---- Logo menu (6m-b) ----

const mnList = $("mn-list");
const mnAbout = $("mn-about");
const mnConfirm = $("mn-confirm");
const cfOk = $("cf-ok");
const cfError = $("cf-error");
/** @type {HTMLLIElement[]} */
const mnItems = Array.from(mnList.querySelectorAll("li[data-act]"));
let mnSelected = -1;
/** @type {"list" | "about" | "confirm"} */
let mnView = "list";
/** @type {"restart" | "shutdown" | "logout"} */
let cfAction = "restart";

const CONFIRM = {
  restart: { title: "Are you sure you want to restart your computer now?", ok: "Restart" },
  shutdown: { title: "Are you sure you want to shut down your computer now?", ok: "Shut Down" },
  logout: { title: "Are you sure you want to quit all applications and log out now?", ok: "Log Out" },
};

/** @param {"list" | "about" | "confirm"} v */
function mnShow(v) {
  mnView = v;
  mnList.hidden = v !== "list";
  mnAbout.hidden = v !== "about";
  mnConfirm.hidden = v !== "confirm";
}

/** @param {number} i */
function mnSelect(i) {
  mnSelected = i;
  mnItems.forEach((li, j) => li.classList.toggle("selected", j === i));
}

/** Next enabled item from `from` in direction `dir`, wrapping. @param {number} from @param {number} dir */
function mnStep(from, dir) {
  const n = mnItems.length;
  for (let k = 1; k <= n; k++) {
    const i = (((from + dir * k) % n) + n) % n;
    if (!mnItems[i].classList.contains("disabled")) return i;
  }
  return -1;
}

async function mnAboutShow() {
  mnShow("about");
  /** @type {Record<string, string> | undefined} */
  const info = await invoke("system_info");
  if (!info) return;
  $("ab-version").textContent = info.version;
  const facts = $("ab-facts");
  facts.replaceChildren();
  for (const [k, label] of [["os", "System"], ["cpu", "Processor"], ["gpu", "Graphics"], ["memory", "Memory"], ["kernel", "Kernel"]]) {
    if (!info[k]) continue;
    const dt = document.createElement("dt");
    dt.textContent = label;
    const dd = document.createElement("dd");
    dd.textContent = info[k];
    facts.append(dt, dd);
  }
  /** @type {HTMLElement | null} */ (mnAbout.querySelector(".btn"))?.focus();
}

/** @param {"restart" | "shutdown" | "logout"} action */
function mnConfirmShow(action) {
  cfAction = action;
  $("cf-title").textContent = CONFIRM[action].title;
  $("cf-text").textContent = "Any unsaved work in open apps may be lost.";
  cfOk.textContent = CONFIRM[action].ok;
  cfError.hidden = true;
  mnShow("confirm");
  cfOk.focus();
}

async function mnConfirmRun() {
  console.log(`menu: ${cfAction} confirmed`);
  // @ts-ignore: injected by Tauri (app.withGlobalTauri).
  const tauri = window.__TAURI__;
  try {
    if (cfAction === "logout") await tauri.core.invoke("window_action", { action: "logout" });
    else await tauri.core.invoke("power", { action: cfAction });
  } catch (e) {
    cfError.textContent = String(e);
    cfError.hidden = false;
  }
}

/** @param {string | undefined} act */
function mnRun(act) {
  console.log(`menu: ${act}`);
  switch (act) {
    case "about": void mnAboutShow(); break;
    // 10c: the System Settings window (the shell opens it).
    case "settings": close(); invoke("open_settings"); break;
    case "launchpad":
    case "spotlight": invoke("show_overlay", { mode: act }); break;
    case "restart-shell": invoke("restart_shell"); break;
    // 8d: kompact runs the lock client.
    case "lock": close(); invoke("window_action", { action: "lock" }); break;
    // 12i-2: no confirmation, as on macOS; the screen locks before it sleeps (power.rs).
    case "sleep":
      invoke("power", { action: "sleep" });
      close();
      break;
    case "restart":
    case "shutdown":
    case "logout": mnConfirmShow(act); break;
    case "confirm": void mnConfirmRun(); break;
    case "close": close(); break;
  }
}

// 12i-2: Sleep shows only with KOMPACT_SLEEP=on.
invoke("power_options").then((/** @type {{sleep?: boolean}} */ o) => {
  const item = $("mn-sleep");
  item.hidden = !o?.sleep;
  item.classList.toggle("disabled", !o?.sleep);
});

mnItems.forEach((li, i) => {
  li.addEventListener("mousemove", () => {
    if (!li.classList.contains("disabled") && i !== mnSelected) mnSelect(i);
  });
  li.addEventListener("mouseleave", () => mnSelect(-1));
  li.addEventListener("click", () => {
    if (!li.classList.contains("disabled")) mnRun(li.dataset.act);
  });
});
for (const b of document.querySelectorAll("#menu .btn")) {
  b.addEventListener("click", () => mnRun(/** @type {HTMLElement} */ (b).dataset.act));
}

/** @param {KeyboardEvent} e */
function mnKey(e) {
  if (e.key === "Escape") {
    close();
  } else if (mnView !== "list") {
    // Dialogs: Enter is the default button; Tab and Space work on the focused button natively.
    if (e.key !== "Enter") return;
    mnRun(mnView === "about" ? "close" : "confirm");
  } else if (e.key === "ArrowDown") {
    mnSelect(mnStep(mnSelected, 1));
  } else if (e.key === "ArrowUp") {
    mnSelect(mnStep(mnSelected < 0 ? 0 : mnSelected, -1));
  } else if (e.key === "Enter") {
    if (mnSelected >= 0) mnRun(mnItems[mnSelected].dataset.act);
  } else {
    return;
  }
  e.preventDefault();
}

// ---- Dock menu (6m-d) ----

const dmList = $("dm-list");
/** @type {{li: HTMLLIElement, run: () => void}[]} */
let dmItems = [];
let dmSelected = -1;

/** @param {number} i */
function dmSelect(i) {
  dmSelected = i;
  dmItems.forEach((it, j) => it.li.classList.toggle("selected", j === i));
}

async function dmOpen() {
  dmList.replaceChildren();
  dmItems = [];
  dmSelected = -1;
  /** @type {{desktop_id: string, name: string, x: number, pinned: boolean,
   *   windows: {id: number, title: string, minimized: boolean, focused: boolean}[],
   *   y: number, side: string, zone: number,
   *   prefs: {hide: "smart" | "never" | "always", magnify: boolean, follow: boolean, position?: string}} | null} */
  const info = await invoke("dock_menu_info");
  if (!info) return close();
  const running = info.windows.length > 0;
  /** @type {HTMLElement[]} */
  const out = [];
  /** @param {string} text @param {string} cls @param {() => void} run */
  const item = (text, cls, run) => {
    const li = document.createElement("li");
    li.setAttribute("role", "menuitem");
    li.className = cls;
    li.textContent = text;
    const i = dmItems.length;
    li.addEventListener("mousemove", () => i !== dmSelected && dmSelect(i));
    li.addEventListener("mouseleave", () => dmSelect(-1));
    li.addEventListener("click", run);
    dmItems.push({ li, run });
    out.push(li);
  };
  const sep = () => {
    const li = document.createElement("li");
    li.className = "sep";
    li.setAttribute("role", "separator");
    out.push(li);
  };
  const place = () => {
    dmList.replaceChildren(...out);
    const w = dmList.offsetWidth;
    const h = dmList.offsetHeight;
    // Clear of the dock (10d-1c: on whichever edge, as big as it is now).
    const gap = (info.zone || 70) + 6;
    const st = dmList.style;
    if (info.side === "left" || info.side === "right") {
      // Beside the icon, kept on screen; `y` is on the monitor, this page may start lower.
      const y = info.y - (window.screen.height - window.innerHeight);
      st.bottom = "auto";
      st.top = `${Math.max(8, Math.min(window.innerHeight - h - 8, y - h / 2))}px`;
      st.left = `${info.side === "left" ? gap : window.innerWidth - gap - w}px`;
    } else {
      // Centred above the icon, kept on screen.
      st.top = "auto";
      st.bottom = `${gap}px`;
      st.left = `${Math.max(8, Math.min(window.innerWidth - w - 8, info.x - w / 2))}px`;
    }
  };
  // 6m-d2: right-click between the icons: the dock's own settings, as on macOS.
  if (info.desktop_id === "@dock") {
    /** @param {"smart" | "never" | "always"} hide @param {boolean} magnify */
    const set = async (hide, magnify) => {
      await invoke("set_dock_prefs", { prefs: { ...info.prefs, hide, magnify } });
      close();
    };
    const p = info.prefs;
    item("Hide When a Window Is Maximized", p.hide === "smart" ? "check" : "nocheck", () => set("smart", p.magnify));
    item("Always Hide", p.hide === "always" ? "check" : "nocheck", () => set("always", p.magnify));
    item("Never Hide", p.hide === "never" ? "check" : "nocheck", () => set("never", p.magnify));
    sep();
    item("Magnification", p.magnify ? "check" : "nocheck", () => set(p.hide, !p.magnify));
    sep();
    // 10d-1c: Position on Screen.
    for (const [pos, name] of [["left", "Left"], ["bottom", "Bottom"], ["right", "Right"]]) {
      item(`Position on ${name}`, (p.position || "bottom") === pos ? "check" : "nocheck", async () => {
        await invoke("set_dock_prefs", { prefs: { ...info.prefs, position: pos } });
        close();
      });
    }
    sep();
    // 10d-1: the rest of the Dock settings, as on macOS.
    item("Dock Settings…", "nocheck", () => {
      close();
      invoke("open_settings", { pane: "dock" });
    });
    place();
    console.log(`dock menu: settings (hide ${p.hide}, magnify ${p.magnify})`);
    return;
  }
  if (info.desktop_id === "@trash") {
    item("Open", "nocheck", () => {
      invoke("dock_open_trash");
      close();
    });
    sep();
    item("Empty Trash", "nocheck", () => {
      invoke("dock_empty_trash");
      close();
    });
    place();
    console.log("dock menu: trash");
    return;
  }
  // Its windows, like macOS: the front one ticked, minimized ones dimmed.
  for (const w of info.windows) {
    const label = w.title || info.name;
    item(label, (w.focused ? "check" : "nocheck") + (w.minimized ? " min" : ""), () => {
      invoke("window_action", { action: "activate", id: w.id });
      close();
    });
  }
  if (running) sep();
  item("Keep in Dock", info.pinned ? "check" : "nocheck", async () => {
    await invoke("set_pinned", { desktopId: info.desktop_id, pinned: !info.pinned });
    close();
  });
  item(running ? "New Window" : "Open", "nocheck", () => {
    invoke("dock_open", { desktopId: info.desktop_id });
    close();
  });
  if (running) {
    sep();
    item("Quit", "nocheck", () => {
      // kompact asks each window to close, as the app's own close button would.
      for (const w of info.windows) invoke("window_action", { action: "close", id: w.id });
      close();
    });
  }
  place();
  console.log(`dock menu: ${info.name}, ${info.windows.length} windows, ${info.pinned ? "pinned" : "not pinned"}`);
}

/** @param {KeyboardEvent} e */
function dmKey(e) {
  const n = dmItems.length;
  if (e.key === "Escape") close();
  else if (e.key === "ArrowDown" && n) dmSelect((dmSelected + 1) % n);
  else if (e.key === "ArrowUp" && n) dmSelect(dmSelected <= 0 ? n - 1 : dmSelected - 1);
  else if (e.key === "Enter" && dmSelected >= 0) dmItems[dmSelected].run();
  else return;
  e.preventDefault();
}

// ---- All views ----

// ---- Workspace overview (6e) ----
// All nine workspaces as miniatures of the screen: the wallpaper, a menu-bar strip, and each
// workspace's windows where they are, bottom to top, as their thumbnails (6m-d3,
// unless KOMPACT_THUMBNAILS=off) or else their app icon on a tile. Positions come from kompact's
// `layout` answer, asked for on open; the window list from `kompact-windows`.

const WS_COUNT = 9; // kompact's workspace::COUNT
const WS_WIDTH = 232; // .ws .mini in overlay.css
const PANEL = 28; // PANEL_HEIGHT
const wsGrid = $("ws-grid");

/** @typedef {{id: number, workspace: number, minimized: boolean, focused: boolean, name: string, title: string, app_id: string,
 *   desktop_id: string | null}} Win */
/** @typedef {{id: number, x: number, y: number, w: number, h: number}} Placed */
/** @typedef {{x: number, y: number, w: number, h: number}} Rect */

/** @type {Win[]} */
let wsWindows = [];
let wsActive = 1;
/** @type {Rect} */
let wsOutput = { x: 0, y: 0, w: window.screen.width || 1440, h: window.screen.height || 900 };
/** @type {Map<number, Placed>} */
let wsPlaces = new Map();
/** @type {Map<number, number>} window id → version of its latest thumbnail */
const wsShots = new Map();
let wsSelected = 0;

/** A window's box in a miniature `scale` times the size of `output`, in px (left, top, w, h).
 * @param {Placed} p @param {Rect} output @param {number} scale */
function miniBox(p, output, scale) {
  return [(p.x - output.x) * scale, (p.y - output.y) * scale, Math.max(2, p.w * scale), Math.max(2, p.h * scale)];
}

/** The windows to draw on workspace `n`, bottom to top (minimized ones aren't on screen).
 * @param {Win[]} windows @param {number} n */
function windowsOn(windows, n) {
  return windows.filter((w) => w.workspace === n && !w.minimized);
}

/** 10d-2: what the wallpaper is, for the overview's miniatures.
 * @type {{kind: string, color: string | null, version: number} | null} */
let wsWallpaper = null;
// @ts-ignore: injected by Tauri (app.withGlobalTauri).
if (window.__TAURI__) {
  // @ts-ignore
  const t = window.__TAURI__;
  t.event.listen("kompact-wallpaper-changed", (/** @type {{payload: any}} */ e) => (wsWallpaper = e.payload));
  t.core.invoke("wallpaper_state").then((/** @type {any} */ s) => (wsWallpaper = s));
}

function wsRender() {
  const scale = WS_WIDTH / wsOutput.w;
  const height = Math.round(wsOutput.h * scale);
  // 10d-2: the wallpaper as it is now (`wsWallpaper`, kept up to date above).
  const wallpaper = wsWallpaper?.kind === "image" ? `url("kompact-wallpaper://localhost/small?v=${wsWallpaper.version}")` : "";
  const wallColor = wsWallpaper?.kind === "color" ? wsWallpaper.color || "" : "";
  const cards = [];
  for (let n = 1; n <= WS_COUNT; n++) {
    const card = document.createElement("button");
    card.className = `ws${n === wsActive ? " current" : ""}${n - 1 === wsSelected ? " selected" : ""}`;
    card.setAttribute("role", "option");
    card.setAttribute("aria-selected", String(n === wsActive));
    const mini = document.createElement("div");
    mini.className = "mini";
    mini.style.height = `${height}px`;
    if (wallpaper) mini.style.backgroundImage = wallpaper;
    else if (wallColor) mini.style.background = wallColor;
    const bar = document.createElement("div");
    bar.className = "bar";
    bar.style.height = `${Math.max(2, Math.round(PANEL * scale))}px`;
    const wins = windowsOn(wsWindows, n);
    for (const w of wins) {
      const p = wsPlaces.get(w.id);
      if (!p) continue;
      const [left, top, bw, bh] = miniBox(p, wsOutput, scale);
      const el = document.createElement("div");
      el.className = `win${w.focused ? " focused" : ""}`;
      el.dataset.id = String(w.id);
      el.style.cssText = `left:${left}px;top:${top}px;width:${bw}px;height:${bh}px`;
      el.title = w.name;
      if (w.desktop_id) {
        const icon = document.createElement("img");
        icon.className = "icon";
        icon.alt = "";
        icon.src = `kompact-icon://localhost/${encodeURIComponent(w.desktop_id)}`;
        icon.addEventListener("error", () => icon.remove(), { once: true });
        el.append(icon);
      }
      wsShot(el, w.id);
      mini.append(el);
    }
    // The menu bar is drawn over the windows, as on screen.
    mini.append(bar);
    const name = document.createElement("div");
    name.className = "name";
    name.textContent = `Desktop ${n}`;
    if (wins.length) {
      const count = document.createElement("span");
      count.className = "count";
      count.textContent = ` · ${wins.length}`;
      name.append(count);
    }
    card.append(mini, name);
    card.setAttribute("aria-label", `Desktop ${n}, ${wins.length} windows`);
    card.addEventListener("click", () => wsGo(n));
    cards.push(card);
  }
  wsGrid.replaceChildren(...cards);
}

/** Put window `id`'s latest thumbnail into its tile `el` (if there is one yet).
 * @param {HTMLElement} el @param {number} id */
function wsShot(el, id) {
  const v = wsShots.get(id);
  if (v === undefined) return;
  let shot = /** @type {HTMLImageElement | null} */ (el.querySelector("img.shot"));
  if (!shot) {
    shot = document.createElement("img");
    shot.className = "shot";
    shot.alt = "";
    shot.addEventListener("load", () => el.classList.add("has-shot"));
    shot.addEventListener("error", () => shot?.remove(), { once: true });
    el.append(shot);
  }
  shot.src = `kompact-thumb://localhost/${id}?v=${v}`;
}

/** @param {number} n */
function wsGo(n) {
  console.log(`overlay: desktop ${n} chosen`);
  if (n !== wsActive) invoke("window_action", { action: "workspace", id: n });
  close();
}

/** @param {any} msg a `windows` message (enriched by kompact-shell) */
function wsOnWindows(msg) {
  if (!msg || !Array.isArray(msg.windows)) return;
  wsWindows = msg.windows;
  for (const id of wsShots.keys()) if (!wsWindows.some((w) => w.id === id)) wsShots.delete(id);
  if (typeof msg.workspace === "number") wsActive = msg.workspace;
  if (mode === "spaces") wsRender();
}

/** @param {any} msg kompact's `layout` answer */
function wsOnLayout(msg) {
  if (!msg || !Array.isArray(msg.windows)) return;
  const o = msg.output;
  if (o && o.w > 0 && o.h > 0) wsOutput = { x: o.x, y: o.y, w: o.w, h: o.h };
  wsPlaces = new Map(msg.windows.map((/** @type {Placed} */ p) => [p.id, p]));
  if (mode === "spaces") wsRender();
}

/** @param {{id: number, version: number}} t */
function wsOnThumb(t) {
  wsShots.set(t.id, t.version);
  if (mode !== "spaces") return;
  // Just that window's picture, not the whole grid (each redraw would fetch every picture).
  const el = wsGrid.querySelector(`.win[data-id="${t.id}"]`);
  if (el instanceof HTMLElement) wsShot(el, t.id);
}

async function wsOpen() {
  wsSelected = wsActive - 1;
  wsRender();
  const state = await invoke("compositor_state");
  wsOnWindows(state);
  wsSelected = wsActive - 1;
  wsRender();
  invoke("window_action", { action: "layout" });
  const ids = wsWindows.filter((w) => !w.minimized).map((w) => w.id);
  if (ids.length) invoke("request_thumbs", { ids });
  console.log(`overlay: desktops open (on ${wsActive}, ${wsWindows.length} windows)`);
}

/** @param {number} i */
function wsSelect(i) {
  wsSelected = (i + WS_COUNT) % WS_COUNT;
  wsRender();
}

/** @param {KeyboardEvent} e */
function wsKey(e) {
  if (e.key === "Escape") {
    e.preventDefault();
    close();
  } else if (e.key >= "1" && e.key <= "9") {
    e.preventDefault();
    wsGo(Number(e.key));
  } else if (e.key === "Enter" || e.key === " ") {
    e.preventDefault();
    wsGo(wsSelected + 1);
  } else if (e.key === "ArrowRight") wsSelect(wsSelected + 1);
  else if (e.key === "ArrowLeft") wsSelect(wsSelected - 1);
  else if (e.key === "ArrowDown") wsSelect(wsSelected + 3);
  else if (e.key === "ArrowUp") wsSelect(wsSelected - 3);
}

// @ts-ignore: injected by Tauri (app.withGlobalTauri).
const wsEvents = window.__TAURI__?.event;
if (wsEvents) {
  wsEvents.listen("kompact-windows", (/** @type {{payload: any}} */ e) => wsOnWindows(e.payload));
  wsEvents.listen("kompact-layout", (/** @type {{payload: any}} */ e) => wsOnLayout(e.payload));
  wsEvents.listen("kompact-thumb", (/** @type {{payload: any}} */ e) => wsOnThumb(e.payload));
}

// ---- 16b: the window switcher (Alt+Tab, the dock's button) ----
// kompact keeps the list, the selection and the keyboard (`kompact/src/switcher.rs`); this page
// draws what its `switcher` message says, using the window list (`wsWindows`) and thumbnails
// (`wsShots`) the overview keeps up to date. The pointer selects (`switcher-select`) and picks
// (`switcher-pick`); a click outside the box cancels.

const swBox = $("sw-box");
/** @type {{windows: number[], selected: number, sticky: boolean} | null} */
let swMsg = null;
/** The ids the cards were drawn for, so a selection change only moves the highlight. */
let swDrawn = "";
/** The pointer has really moved since the switcher opened: only then does it select (cards
 * appearing under a still pointer would otherwise change what letting go of Alt picks). */
let swMoved = false;
/** @type {{x: number, y: number} | null} where the pointer was first seen */
let swFirst = null;

/** @param {Win} w */
function swIcon(w) {
  if (w.desktop_id) {
    const img = document.createElement("img");
    img.className = "sw-icon";
    img.alt = "";
    img.draggable = false;
    img.src = `kompact-icon://localhost/${encodeURIComponent(w.desktop_id)}`;
    img.addEventListener("error", () => img.replaceWith(swBadge(w)), { once: true });
    return img;
  }
  return swBadge(w);
}

/** @param {Win} w */
function swBadge(w) {
  const name = w.name || w.app_id || "?";
  const b = document.createElement("div");
  b.className = "sw-icon badge";
  const h = hue(w.app_id || name);
  b.style.background = `linear-gradient(160deg, hsl(${h} 55% 55%), hsl(${h} 55% 38%))`;
  b.textContent = (name.trim()[0] || "?").toUpperCase();
  return b;
}

function swRender() {
  if (!swMsg) return;
  const ids = swMsg.windows;
  const sig = ids.join(",");
  if (sig !== swDrawn) {
    swDrawn = sig;
    const cards = ids.map((id) => {
      const w = wsWindows.find((x) => x.id === id);
      const card = document.createElement("div");
      card.className = "sw-card";
      card.dataset.id = String(id);
      card.setAttribute("role", "option");
      const thumb = document.createElement("div");
      thumb.className = "sw-thumb";
      if (w) {
        const h = hue(w.app_id || String(id));
        thumb.style.background = `linear-gradient(160deg, hsl(${h} 30% 34%), hsl(${h} 30% 18%))`;
        thumb.append(swIcon(w));
        if (w.minimized) {
          const tag = document.createElement("span");
          tag.className = "sw-tag";
          tag.textContent = "Minimized";
          thumb.append(tag);
        }
      }
      swShot(thumb, id);
      const title = document.createElement("div");
      title.className = "sw-title";
      title.textContent = (w && (w.title || w.name || w.app_id)) || `Window ${id}`;
      card.append(thumb, title);
      card.setAttribute("aria-label", title.textContent);
      card.addEventListener("mousemove", () => {
        if (!swMoved || !swMsg || swMsg.windows[swMsg.selected] === id) return;
        swMsg.selected = swMsg.windows.indexOf(id);
        swHighlight();
        invoke("window_action", { action: "switcher-select", id });
      });
      card.addEventListener("click", () => invoke("window_action", { action: "switcher-pick", id }));
      return card;
    });
    swBox.replaceChildren(...cards);
  }
  swHighlight();
}

function swHighlight() {
  const sel = swMsg ? swMsg.windows[swMsg.selected] : -1;
  for (const c of swBox.children) {
    const on = c instanceof HTMLElement && c.dataset.id === String(sel);
    c.classList.toggle("selected", on);
    c.setAttribute("aria-selected", String(on));
  }
}

/** Window `id`'s latest thumbnail into its card's picture box (if there is one yet).
 * @param {HTMLElement} thumb @param {number} id */
function swShot(thumb, id) {
  const v = wsShots.get(id);
  if (v === undefined) return;
  let img = /** @type {HTMLImageElement | null} */ (thumb.querySelector("img.shot"));
  if (!img) {
    img = document.createElement("img");
    img.className = "shot";
    img.alt = "";
    img.draggable = false;
    img.addEventListener("load", () => thumb.classList.add("has-shot"));
    img.addEventListener("error", () => img?.remove(), { once: true });
    thumb.prepend(img);
  }
  img.src = `kompact-thumb://localhost/${id}?v=${v}`;
}

/** @param {any} msg kompact's `switcher` message */
function swOnMessage(msg) {
  if (!msg || msg.open !== true || !Array.isArray(msg.windows)) {
    swMsg = null;
    swDrawn = "";
    return;
  }
  const fresh = !swMsg;
  swMsg = { windows: msg.windows, selected: Number(msg.selected) || 0, sticky: msg.sticky === true };
  if (mode === "switcher") swRender();
  if (fresh) invoke("request_thumbs", { ids: swMsg.windows });
}

async function swOpen() {
  swMoved = false;
  swFirst = null;
  if (!swMsg) swOnMessage(await invoke("switcher_state"));
  if (!swMsg) return;
  // A window that opened since the last list (titles and icons come from it).
  if (swMsg.windows.some((id) => !wsWindows.some((w) => w.id === id))) {
    wsOnWindows(await invoke("compositor_state"));
    swDrawn = "";
  }
  swRender();
  console.log(`overlay: switcher open, ${swMsg.windows.length} windows${swMsg.sticky ? " (dock button)" : ""}`);
}

/** @param {{id: number, version: number}} t */
function swOnThumb(t) {
  if (mode !== "switcher") return;
  const thumb = swBox.querySelector(`.sw-card[data-id="${t.id}"] .sw-thumb`);
  if (thumb instanceof HTMLElement) swShot(thumb, t.id);
}

views.switcher.addEventListener("mousemove", (e) => {
  if (swMoved) return;
  if (!swFirst) swFirst = { x: e.screenX, y: e.screenY };
  else if (Math.abs(e.screenX - swFirst.x) + Math.abs(e.screenY - swFirst.y) > 2) swMoved = true;
}, { capture: true });

if (wsEvents) {
  wsEvents.listen("kompact-switcher", (/** @type {{payload: any}} */ e) => swOnMessage(e.payload));
  // After the overview's listener, which keeps `wsShots`.
  wsEvents.listen("kompact-thumb", (/** @type {{payload: any}} */ e) => swOnThumb(e.payload));
}

// ---- 9c: screenshot region (Shift+Print) ----

const selRect = $("sel-rect");
const selSize = $("sel-size");
/** Where the drag started (page px = logical px on this monitor), or null. @type {{x: number, y: number} | null} */
let selFrom = null;
/** Smaller than this (either way) is a click, not a region: cancelled. */
const SEL_MIN = 4;

/** The rectangle between two points. @param {{x: number, y: number}} a @param {{x: number, y: number}} b */
function selBox(a, b) {
  const x = Math.round(Math.min(a.x, b.x));
  const y = Math.round(Math.min(a.y, b.y));
  return { x, y, w: Math.round(Math.max(a.x, b.x)) - x, h: Math.round(Math.max(a.y, b.y)) - y };
}

function selOpen() {
  selFrom = null;
  selRect.hidden = true;
  console.log("overlay: select open");
}

/** @param {MouseEvent} e */
function selDown(e) {
  if (e.button !== 0) return;
  selFrom = { x: e.clientX, y: e.clientY };
  selMove(e);
  selRect.hidden = false;
}

/** @param {MouseEvent} e */
function selMove(e) {
  if (!selFrom) return;
  const b = selBox(selFrom, { x: e.clientX, y: e.clientY });
  Object.assign(selRect.style, { left: `${b.x}px`, top: `${b.y}px`, width: `${b.w}px`, height: `${b.h}px` });
  selSize.textContent = `${b.w} × ${b.h}`;
}

/** @param {MouseEvent} e */
function selUp(e) {
  if (!selFrom || e.button !== 0) return;
  const b = selBox(selFrom, { x: e.clientX, y: e.clientY });
  selFrom = null;
  selRect.hidden = true;
  if (b.w < SEL_MIN || b.h < SEL_MIN) {
    console.log("overlay: select cancelled (no region dragged)");
    close();
    return;
  }
  // The shell hides this surface first; kompact waits until it's gone before it captures.
  invoke("select_region", b);
}

views.select.addEventListener("mousedown", selDown);
views.select.addEventListener("mousemove", selMove);
views.select.addEventListener("mouseup", selUp);

document.addEventListener("keydown", (e) => {
  if (mode === "select") {
    if (e.key === "Escape") close();
    return;
  }
  if (mode === "spaces") wsKey(e);
  else if (mode === "launchpad") lpKey(e);
  else if (mode === "spotlight") spKey(e);
  else if (mode === "menu") mnKey(e);
  else if (mode === "dockmenu") dmKey(e);
  else if (mode === "status") stKey(e);
  else if (mode === "auth" && e.key === "Escape") auCancel();
});

// ---- 12a: menu-bar status menus ----

const stMenu = $("st-menu");
/** Which status menu is up, and the screen x of its item's right edge. */
let stWhich = "";
let stX = 0;

async function stOpen() {
  const t = await invoke("status_target");
  if (!t) return close();
  [stWhich, stX] = t;
  stMenu.replaceChildren();
  stMenu.classList.remove("wide");
  if (stWhich === "network") await netOpen();
  else if (stWhich === "bluetooth") await btOpen();
  else if (stWhich === "sound") await sdOpen();
  else if (stWhich === "center") await ncOpen();
  else if (stWhich === "control") await ccOpen();
  else if (stWhich === "input") await inOpen();
  console.log(`overlay: ${stWhich} menu open`);
}

/** Under its item, right edges lined up, kept on screen. */
function stPlace() {
  const w = stMenu.offsetWidth;
  // The page may not start at the monitor's left (it does: the overlay covers the monitor).
  stMenu.style.left = `${Math.max(8, Math.min(window.innerWidth - w - 8, stX - w))}px`;
}

/** @param {KeyboardEvent} e */
function stKey(e) {
  if (e.key !== "Escape") return;
  // A password being typed: Esc gives it up first.
  if (netAsking) {
    netAsking = "";
    netRender();
    return;
  }
  close();
}

/** @param {string} cls @param {string | Node} [content] */
function stEl(cls, content) {
  const el = document.createElement("div");
  el.className = cls;
  if (content !== undefined) el.append(content);
  return el;
}

/** An on/off switch for a menu row. @param {boolean} on @param {(on: boolean) => void} change */
function stSwitch(on, change) {
  const b = document.createElement("button");
  b.className = `st-switch${on ? " on" : ""}`;
  b.setAttribute("role", "switch");
  b.setAttribute("aria-checked", String(on));
  b.addEventListener("click", (e) => {
    e.stopPropagation();
    change(!on);
  });
  return b;
}

// ---- 12a: Network (NetworkManager, src-tauri/src/network.rs) ----

/** @typedef {{ssid: string, strength: number, bars: number, security: string, active: boolean, known: boolean}} Net */
/** @typedef {{iface: string, kind: string, state: string, ip4: string | null, connection: string | null}} NetDevice */
/** @typedef {{wifi_enabled: boolean, wifi_hardware: boolean, devices: NetDevice[], networks: Net[], known: {uuid: string, name: string, ssid: string | null}[]}} NetState */
/** @type {NetState | null} */
let net = null;
/** The network a password is being typed for. */
let netAsking = "";
/** A join under way, and how the last one went. */
let netJoining = "";
/** @type {{ssid: string, ok: boolean, message: string} | null} */
let netResult = null;

async function netOpen() {
  netAsking = "";
  netResult = null;
  net = (await invoke("network_state")) || null;
  netRender();
  void invoke("network_scan");
}

/** Wi-Fi bars as an icon. @param {number} bars */
function netBars(bars) {
  const ns = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(ns, "svg");
  svg.setAttribute("viewBox", "0 0 16 16");
  svg.setAttribute("class", "st-bars");
  const parts = ["M8 13.2l1.6-1.9a2.4 2.4 0 0 0-3.2 0z", "M4.6 9.2a5.2 5.2 0 0 1 6.8 0l-1 1.2a3.6 3.6 0 0 0-4.8 0z", "M2.4 6.6a8.6 8.6 0 0 1 11.2 0l-1 1.2a7 7 0 0 0-9.2 0z"];
  parts.forEach((d, i) => {
    const p = document.createElementNS(ns, "path");
    p.setAttribute("d", d);
    if (i > bars) p.setAttribute("class", "dim");
    svg.append(p);
  });
  return svg;
}

function netLock() {
  const s = document.createElement("span");
  s.className = "st-lock";
  s.textContent = "🔒";
  s.title = "Needs a password";
  return s;
}

function netRender() {
  const st = net;
  const out = [];
  const wifi = st?.devices.find((d) => d.kind === "wifi");
  if (!st) {
    out.push(stEl("st-note", "NetworkManager isn't answering."));
  }
  if (st && wifi) {
    const head = stEl("st-row st-head", "Wi-Fi");
    head.append(
      stSwitch(st.wifi_enabled, async (on) => {
        await invoke("network_wifi", { on });
        net = (await invoke("network_state")) || net;
        netRender();
      }),
    );
    out.push(head);
    if (!st.wifi_hardware) out.push(stEl("st-note", "The Wi-Fi switch on the device is off."));
    if (st.wifi_enabled) {
      const known = st.networks.filter((n) => n.active || n.known);
      const other = st.networks.filter((n) => !n.active && !n.known);
      if (known.length) out.push(stEl("st-label", known.length === 1 ? "Known Network" : "Known Networks"), ...known.map(netRow));
      if (other.length) out.push(stEl("st-label", "Other Networks"), ...other.map(netRow));
      if (!st.networks.length) out.push(stEl("st-note", "Looking for networks…"));
    }
  }
  for (const d of st?.devices.filter((d) => d.kind === "ethernet") || []) {
    out.push(stEl("st-sep"));
    const row = stEl("st-row");
    const text = d.state === "connected" ? `Connected${d.ip4 ? ` · ${d.ip4}` : ""}` : d.state === "unavailable" ? "Cable unplugged" : d.state[0].toUpperCase() + d.state.slice(1);
    row.append(stEl("st-name", "Ethernet"), stEl("st-dim", text));
    out.push(row);
  }
  if (st && !wifi && !st.devices.length) out.push(stEl("st-note", "No network devices."));
  out.push(stEl("st-sep"));
  const settings = stEl("st-row st-item", "Network Settings…");
  settings.addEventListener("click", () => {
    close();
    invoke("open_settings", { pane: "network" });
  });
  out.push(settings);
  stMenu.replaceChildren(...out);
  stPlace();
  /** @type {HTMLInputElement | null} */ (stMenu.querySelector("input"))?.focus();
}

/** @param {Net} n */
function netRow(n) {
  const wrap = document.createElement("div");
  const row = stEl(`st-row st-item${n.active ? " st-active" : ""}`);
  const check = stEl("st-check", n.active ? "✓" : "");
  const name = stEl("st-name", n.ssid);
  const icons = stEl("st-icons");
  if (n.security !== "open") icons.append(netLock());
  icons.append(netBars(n.bars));
  row.append(check, name, icons);
  row.addEventListener("click", () => {
    if (n.active || netJoining) return;
    netResult = null;
    if (n.known || n.security === "open") return netJoin(n.ssid, null);
    netAsking = netAsking === n.ssid ? "" : n.ssid;
    netRender();
  });
  wrap.append(row);
  if (netJoining === n.ssid) wrap.append(stEl("st-note", `Joining ${n.ssid}…`));
  if (netResult?.ssid === n.ssid && !netResult.ok) wrap.append(stEl("st-note st-error", netResult.message));
  if (netAsking === n.ssid) {
    const form = document.createElement("form");
    form.className = "st-pass";
    const input = document.createElement("input");
    input.type = "password";
    input.placeholder = "Password";
    input.autocomplete = "off";
    input.setAttribute("aria-label", `Password for ${n.ssid}`);
    const join = document.createElement("button");
    join.type = "submit";
    join.className = "btn default";
    join.textContent = "Join";
    form.append(input, join);
    form.addEventListener("submit", (e) => {
      e.preventDefault();
      if (input.value) netJoin(n.ssid, input.value);
    });
    form.addEventListener("mousedown", (e) => e.stopPropagation());
    wrap.append(form);
  }
  return wrap;
}

/** @param {string} ssid @param {string | null} password */
function netJoin(ssid, password) {
  netAsking = "";
  netJoining = ssid;
  netRender();
  // @ts-ignore: injected by Tauri.
  window.__TAURI__.core.invoke("network_join", { ssid, password }).catch((/** @type {unknown} */ e) => {
    netJoining = "";
    netResult = { ssid, ok: false, message: String(e) };
    netRender();
  });
}

// @ts-ignore: injected by Tauri.
if (window.__TAURI__) {
  // @ts-ignore
  const t = window.__TAURI__;
  t.event.listen("kompact-network-changed", async () => {
    // Not while a password is being typed: the list would move under it.
    if (mode !== "status" || stWhich !== "network" || netAsking) return;
    net = (await invoke("network_state")) || net;
    netRender();
  });
  t.event.listen("kompact-network-joined", async (/** @type {{payload: {ssid: string, ok: boolean, message: string}}} */ e) => {
    netJoining = "";
    netResult = e.payload;
    if (mode !== "status" || stWhich !== "network") return;
    net = (await invoke("network_state")) || net;
    netRender();
  });
}

// ---- 12b: Bluetooth (BlueZ, src-tauri/src/bluetooth.rs) ----

/** @typedef {{path: string, address: string, name: string, kind: string, paired: boolean, connected: boolean, battery: number | null}} BtDevice */
/** @typedef {{adapter: string | null, name?: string, powered?: boolean, blocked?: boolean, devices?: BtDevice[]}} BtState */
/** @type {BtState | null} */
let bt = null;
/** Devices being connected or disconnected, and the last error. */
const btBusy = new Set();
let btError = "";
const BT_GLYPH = /** @type {Record<string, string>} */ ({ headphones: "🎧", speaker: "🔊", keyboard: "⌨", mouse: "🖱", gamepad: "🎮", phone: "📱", computer: "💻" });

async function btOpen() {
  btError = "";
  bt = (await invoke("bluetooth_state")) || null;
  btRender();
}

function btRender() {
  const st = bt;
  const out = [];
  if (!st || !st.adapter) {
    out.push(stEl("st-note", st ? "No Bluetooth adapter." : "Bluetooth (BlueZ) isn't answering."));
  } else {
    const head = stEl("st-row st-head", "Bluetooth");
    head.append(
      stSwitch(!!st.powered, async (on) => {
        // @ts-ignore: injected by Tauri.
        await window.__TAURI__.core.invoke("bluetooth_power", { on }).catch((/** @type {unknown} */ e) => (btError = String(e)));
        bt = (await invoke("bluetooth_state")) || bt;
        btRender();
      }),
    );
    out.push(head);
    if (st.blocked) out.push(stEl("st-note", "Bluetooth is switched off on this device (rfkill)."));
    const paired = (st.devices || []).filter((d) => d.paired);
    if (st.powered) {
      out.push(stEl("st-label", "Devices"));
      if (!paired.length) out.push(stEl("st-note", "No devices yet: pair one in Bluetooth Settings."));
      for (const d of paired) {
        const row = stEl(`st-row st-item${d.connected ? " st-active" : ""}`);
        const busy = btBusy.has(d.path);
        const right = busy ? (d.connected ? "Disconnecting…" : "Connecting…") : d.connected ? (d.battery !== null ? `${d.battery}%` : "Connected") : "";
        row.append(stEl("st-check", BT_GLYPH[d.kind] || "◉"), stEl("st-name", d.name), stEl("st-dim", right));
        row.addEventListener("click", () => btToggle(d));
        out.push(row);
      }
    }
    if (btError) out.push(stEl("st-note st-error", btError));
  }
  out.push(stEl("st-sep"));
  const settings = stEl("st-row st-item", "Bluetooth Settings…");
  settings.addEventListener("click", () => {
    close();
    invoke("open_settings", { pane: "bluetooth" });
  });
  out.push(settings);
  stMenu.replaceChildren(...out);
  stPlace();
}

/** @param {BtDevice} d */
async function btToggle(d) {
  if (btBusy.has(d.path)) return;
  btBusy.add(d.path);
  btError = "";
  btRender();
  // @ts-ignore: injected by Tauri.
  await window.__TAURI__.core
    .invoke(d.connected ? "bluetooth_disconnect" : "bluetooth_connect", { path: d.path })
    .catch((/** @type {unknown} */ e) => (btError = String(e)));
  btBusy.delete(d.path);
  bt = (await invoke("bluetooth_state")) || bt;
  if (mode === "status" && stWhich === "bluetooth") btRender();
}

// @ts-ignore: injected by Tauri.
if (window.__TAURI__) {
  // @ts-ignore
  window.__TAURI__.event.listen("kompact-bluetooth-changed", async () => {
    if (mode !== "status" || stWhich !== "bluetooth") return;
    bt = (await invoke("bluetooth_state")) || bt;
    btRender();
  });
}

// ---- 12c: Sound (PipeWire, src-tauri/src/sound.rs) ----

/** @typedef {{id: number, name: string, label: string, nick: string | null, output: boolean}} SdNode */
/** @typedef {{devices: SdNode[], current: number | null, volume: number, muted: boolean}} SdSide */
/** @typedef {{output: SdSide, input: SdSide, waves: number}} SdState */
/** @type {SdState | null} */
let sd = null;
/** The slider is being dragged: don't move it under the pointer. */
let sdDragging = false;
let sdTimer = 0;

async function sdOpen() {
  sd = (await invoke("sound_state")) || null;
  sdRender();
}

function sdRender() {
  const st = sd;
  const out = [];
  if (!st) {
    out.push(stEl("st-note", "PipeWire isn't answering."));
  } else {
    out.push(stEl("st-row st-head", "Sound"));
    const row = stEl("st-row st-slider");
    const range = document.createElement("input");
    range.type = "range";
    range.min = "0";
    range.max = "100";
    range.value = String(Math.round((st.output.muted ? 0 : st.output.volume) * 100));
    range.setAttribute("aria-label", "Output volume");
    range.addEventListener("input", () => {
      sdDragging = true;
      clearTimeout(sdTimer);
      // At most a few times a second while dragging (each is a wpctl run).
      sdTimer = setTimeout(() => invoke("sound_volume", { output: true, volume: Number(range.value) / 100 }), 60);
    });
    range.addEventListener("change", () => {
      sdDragging = false;
      clearTimeout(sdTimer);
      invoke("sound_volume", { output: true, volume: Number(range.value) / 100 });
    });
    row.append(stEl("st-check", "🔈"), range, stEl("st-check", "🔊"));
    out.push(row);
    if (st.output.devices.length) {
      out.push(stEl("st-label", "Output"));
      for (const d of st.output.devices) {
        const r = stEl(`st-row st-item${d.id === st.output.current ? " st-active" : ""}`);
        r.append(stEl("st-check", d.id === st.output.current ? "✓" : ""), stEl("st-name", d.nick ? `${d.nick}` : d.label));
        r.title = d.label;
        r.addEventListener("click", async () => {
          if (d.id === st.output.current) return;
          await invoke("sound_default", { id: d.id });
          sd = (await invoke("sound_state")) || sd;
          sdRender();
        });
        out.push(r);
      }
    }
    // 12d: the microphone, under the output (its own mute, level and device).
    if (st.input.devices.length) {
      out.push(stEl("st-sep"));
      const mic = stEl("st-row st-head", "Microphone");
      if (st.input.current !== null) {
        mic.append(
          stSwitch(!st.input.muted, async (on) => {
            await invoke("sound_mute", { output: false, mute: !on });
            sd = (await invoke("sound_state")) || sd;
            sdRender();
          }),
        );
      }
      out.push(mic);
      if (st.input.current === null) {
        out.push(stEl("st-note", "Choose a microphone:"));
      } else {
        const level = stEl("st-row st-slider");
        const range = document.createElement("input");
        range.type = "range";
        range.min = "0";
        range.max = "100";
        range.value = String(Math.round(st.input.volume * 100));
        range.disabled = st.input.muted;
        range.setAttribute("aria-label", "Microphone level");
        range.addEventListener("input", () => {
          sdDragging = true;
          clearTimeout(sdTimer);
          sdTimer = setTimeout(() => invoke("sound_volume", { output: false, volume: Number(range.value) / 100 }), 60);
        });
        range.addEventListener("change", () => {
          sdDragging = false;
          clearTimeout(sdTimer);
          invoke("sound_volume", { output: false, volume: Number(range.value) / 100 });
        });
        level.append(stEl("st-check", "🎙️"), range);
        out.push(level);
      }
      for (const d of st.input.devices) {
        const r = stEl(`st-row st-item${d.id === st.input.current ? " st-active" : ""}`);
        r.append(stEl("st-check", d.id === st.input.current ? "✓" : ""), stEl("st-name", d.nick || d.label));
        r.title = d.label;
        r.addEventListener("click", async () => {
          if (d.id === st.input.current) return;
          await invoke("sound_default", { id: d.id });
          sd = (await invoke("sound_state")) || sd;
          sdRender();
        });
        out.push(r);
      }
    }
  }
  out.push(stEl("st-sep"));
  const settings = stEl("st-row st-item", "Sound Settings…");
  settings.addEventListener("click", () => {
    close();
    invoke("open_settings", { pane: "sound" });
  });
  out.push(settings);
  stMenu.replaceChildren(...out);
  stPlace();
}

// @ts-ignore: injected by Tauri.
if (window.__TAURI__) {
  // @ts-ignore
  window.__TAURI__.event.listen("kompact-sound-changed", async () => {
    if (mode !== "status" || stWhich !== "sound" || sdDragging) return;
    sd = (await invoke("sound_state")) || sd;
    sdRender();
  });
}

// ---- 12d: the Notification Center (the clock's menu): the date, a month, the notifications ----

/** @typedef {{id: number, app: string, summary: string, body: string, time: number, clickable: boolean, icon: boolean}} NcEntry */
/** @type {{dnd: boolean, entries: NcEntry[]} | null} */
let nc = null;
/** The month shown: 0 = this one. */
let ncMonth = 0;

/** 12g: timedated's zone (WebKit keeps the one it started with). @type {string | undefined} */
let ncZone;

async function ncOpen() {
  ncMonth = 0;
  ncZone = ((await invoke("datetime_state")) || {}).zone || ncZone;
  nc = (await invoke("notify_center")) || null;
  ncRender();
}

/** "now", "5 min", "2 h", else the day. @param {number} t seconds since 1970 */
function ncAgo(t) {
  const s = Math.max(0, Date.now() / 1000 - t);
  if (s < 60) return "now";
  if (s < 3600) return `${Math.floor(s / 60)} min`;
  if (s < 86400) return `${Math.floor(s / 3600)} h`;
  return new Date(t * 1000).toLocaleDateString(undefined, { day: "numeric", month: "short" });
}

function ncCalendar() {
  // Today in timedated's zone (the page's own may be stale).
  const parts = new Intl.DateTimeFormat("en-CA", { year: "numeric", month: "2-digit", day: "2-digit", ...(ncZone ? { timeZone: ncZone } : {}) }).format(new Date());
  const [y, m, dd] = parts.split("-").map(Number);
  const today = new Date(y, m - 1, dd);
  const first = new Date(today.getFullYear(), today.getMonth() + ncMonth, 1);
  const box = stEl("nc-cal");
  const head = stEl("nc-cal-head");
  const prev = Object.assign(document.createElement("button"), { className: "nc-nav", textContent: "‹", title: "Previous month" });
  const next = Object.assign(document.createElement("button"), { className: "nc-nav", textContent: "›", title: "Next month" });
  prev.addEventListener("click", () => {
    ncMonth--;
    ncRender();
  });
  next.addEventListener("click", () => {
    ncMonth++;
    ncRender();
  });
  head.append(prev, stEl("nc-month", first.toLocaleDateString(undefined, { month: "long", year: "numeric" })), next);
  box.append(head);
  const grid = stEl("nc-grid");
  // Week from Monday.
  const names = [...Array(7)].map((_, i) => new Date(2024, 0, 1 + i).toLocaleDateString(undefined, { weekday: "narrow" }));
  for (const n of names) grid.append(stEl("nc-wd", n));
  const lead = (first.getDay() + 6) % 7;
  for (let i = 0; i < lead; i++) grid.append(stEl("nc-day empty"));
  const days = new Date(first.getFullYear(), first.getMonth() + 1, 0).getDate();
  for (let d = 1; d <= days; d++) {
    const isToday = ncMonth === 0 && d === today.getDate();
    grid.append(stEl(`nc-day${isToday ? " today" : ""}`, String(d)));
  }
  box.append(grid);
  return box;
}

function ncRender() {
  const out = [];
  const now = new Date();
  out.push(stEl("nc-date", now.toLocaleDateString(undefined, { weekday: "long", day: "numeric", month: "long", ...(ncZone ? { timeZone: ncZone } : {}) })));
  out.push(ncCalendar());
  out.push(stEl("st-sep"));
  const head = stEl("st-row st-head", "Notifications");
  if (nc?.entries.length) {
    const clear = Object.assign(document.createElement("button"), { className: "nc-clear", textContent: "Clear All" });
    clear.addEventListener("click", async () => {
      await invoke("notify_center_clear", { id: null });
    });
    head.append(clear);
  }
  out.push(head);
  if (nc?.dnd) {
    const note = stEl("st-row nc-dnd", "🌙 Do Not Disturb is on");
    const off = Object.assign(document.createElement("button"), { className: "nc-clear", textContent: "Turn Off" });
    off.addEventListener("click", () => ccSetDnd(false));
    note.append(off);
    out.push(note);
  }
  const list = stEl("nc-list");
  for (const e of nc?.entries || []) {
    const card = stEl(`nc-card${e.clickable ? " clickable" : ""}`);
    const top = stEl("nc-top");
    if (e.icon) {
      const img = document.createElement("img");
      img.className = "nc-icon";
      img.alt = "";
      img.src = `kompact-icon://localhost/center-${e.id}`;
      top.append(img);
    }
    top.append(stEl("nc-app", e.app || "Notification"), stEl("nc-time", ncAgo(e.time)));
    const x = Object.assign(document.createElement("button"), { className: "nc-x", textContent: "×", title: "Clear" });
    x.addEventListener("click", (ev) => {
      ev.stopPropagation();
      invoke("notify_center_clear", { id: e.id });
    });
    top.append(x);
    card.append(top);
    if (e.summary) card.append(stEl("nc-title", e.summary));
    if (e.body) card.append(stEl("nc-body", e.body));
    if (e.clickable) {
      card.addEventListener("click", () => {
        invoke("notify_center_open", { id: e.id });
        close();
      });
    }
    list.append(card);
  }
  if (!nc?.entries.length) list.append(stEl("st-note nc-none", "No notifications."));
  out.push(list);
  stMenu.classList.add("wide");
  stMenu.replaceChildren(...out);
  stPlace();
}

// ---- 12d: Control Center (the menu bar's switches): Wi-Fi, Bluetooth, Do Not Disturb, Dark
// Mode, sound and the microphone, from the same commands as their own menus ----

/** @type {{net: NetState | null, bt: BtState | null, sd: SdState | null, dnd: boolean, k: any}} */
let cc = { net: null, bt: null, sd: null, dnd: false, k: null };

async function ccOpen() {
  const [netS, btS, sdS, ncS, k] = await Promise.all([
    invoke("network_state"),
    invoke("bluetooth_state"),
    invoke("sound_state"),
    invoke("notify_center"),
    invoke("kompact_settings_state"),
  ]);
  cc = { net: netS || null, bt: btS || null, sd: sdS || null, dnd: !!ncS?.dnd, k: k || null };
  ccRender();
}

/** @param {boolean} on */
async function ccSetDnd(on) {
  const p = await invoke("notify_prefs");
  if (!p) return;
  await invoke("set_notify_prefs", { dnd: on, bannerMs: p.banner_ms, muted: p.muted });
  cc.dnd = on;
  if (stWhich === "control") ccRender();
  else if (stWhich === "center") void ncOpen();
}

/** A round toggle tile. @param {string} glyph @param {string} name @param {string} detail @param {boolean} on @param {() => void} click */
function ccTile(glyph, name, detail, on, click) {
  const t = document.createElement("button");
  t.className = `cc-tile${on ? " on" : ""}`;
  t.append(stEl("cc-glyph", glyph));
  const text = stEl("cc-text");
  text.append(stEl("cc-name", name), stEl("cc-detail", detail));
  t.append(text);
  t.addEventListener("click", click);
  return t;
}

function ccRender() {
  const out = [];
  const grid = stEl("cc-grid");
  const wifiDev = cc.net?.devices.find((d) => d.kind === "wifi");
  const eth = cc.net?.devices.find((d) => d.kind === "ethernet" && d.state === "connected");
  if (wifiDev) {
    const joined = cc.net?.networks.find((n) => n.active);
    grid.append(
      ccTile("📶", "Wi-Fi", cc.net?.wifi_enabled ? joined?.ssid || "Not connected" : "Off", !!cc.net?.wifi_enabled, async () => {
        await invoke("network_wifi", { on: !cc.net?.wifi_enabled });
        cc.net = (await invoke("network_state")) || cc.net;
        ccRender();
      }),
    );
  } else if (eth) {
    grid.append(ccTile("🖧", "Ethernet", "Connected", true, () => {}));
  }
  if (cc.bt?.adapter) {
    const on = (cc.bt.devices || []).filter((d) => d.paired && d.connected);
    grid.append(
      ccTile("ᛒ", "Bluetooth", cc.bt.powered ? (on.length ? on.map((d) => d.name).join(", ") : "On") : "Off", !!cc.bt.powered, async () => {
        await invoke("bluetooth_power", { on: !cc.bt?.powered });
        cc.bt = (await invoke("bluetooth_state")) || cc.bt;
        ccRender();
      }),
    );
  }
  grid.append(ccTile("🌙", "Do Not Disturb", cc.dnd ? "On" : "Off", cc.dnd, () => ccSetDnd(!cc.dnd)));
  const dark = cc.k ? cc.k.dark !== false : true;
  grid.append(
    ccTile("◐", "Dark Mode", cc.k?.appearance === "auto" ? `Auto (${dark ? "dark" : "light"} now)` : dark ? "On" : "Off", dark, async () => {
      await invoke("kompact_set", { key: "appearance", value: dark ? "light" : "dark" });
      cc.k = { ...(cc.k || {}), appearance: dark ? "light" : "dark", dark: !dark };
      ccRender();
    }),
  );
  out.push(grid);
  if (cc.sd) {
    const st = cc.sd;
    const card = stEl("cc-card");
    card.append(stEl("cc-name", "Sound"));
    const row = stEl("st-row st-slider");
    const range = Object.assign(document.createElement("input"), { type: "range", min: "0", max: "100" });
    range.value = String(Math.round((st.output.muted ? 0 : st.output.volume) * 100));
    range.setAttribute("aria-label", "Volume");
    range.addEventListener("input", () => {
      clearTimeout(sdTimer);
      sdTimer = setTimeout(() => invoke("sound_volume", { output: true, volume: Number(range.value) / 100 }), 60);
    });
    range.addEventListener("change", () => invoke("sound_volume", { output: true, volume: Number(range.value) / 100 }));
    row.append(stEl("st-check", "🔈"), range, stEl("st-check", "🔊"));
    card.append(row);
    if (st.input.current !== null) {
      const mic = stEl("st-row");
      mic.append(stEl("st-check", "🎙️"), stEl("st-name", "Microphone"));
      mic.append(
        stSwitch(!st.input.muted, async (on) => {
          await invoke("sound_mute", { output: false, mute: !on });
          cc.sd = (await invoke("sound_state")) || cc.sd;
          ccRender();
        }),
      );
      card.append(mic);
    }
    out.push(card);
  }
  stMenu.classList.add("wide");
  stMenu.replaceChildren(...out);
  stPlace();
}

// @ts-ignore: injected by Tauri.
if (window.__TAURI__) {
  // @ts-ignore
  const t = window.__TAURI__;
  t.event.listen("kompact-notify-center", async () => {
    if (mode !== "status" || stWhich !== "center") return;
    nc = (await invoke("notify_center")) || nc;
    ncRender();
  });
}

// ---- 12e: polkit's password dialog (src-tauri/src/polkit.rs) ----

/** @type {{id: number, message: string, action: string, user: string, error: string | null} | null} */
let au = null;
let auBusy = false;
const auPass = /** @type {HTMLInputElement} */ ($("au-pass"));

async function auOpen() {
  au = (await invoke("polkit_request")) || null;
  if (!au) return close();
  $("au-text").textContent = au.message || "An app wants to make a change that needs an administrator.";
  $("au-user").textContent = `Password for ${au.user}:`;
  $("au-action").textContent = au.action;
  const err = $("au-error");
  err.hidden = !au.error;
  err.textContent = au.error || "";
  auBusy = false;
  /** @type {HTMLButtonElement} */ ($("au-ok")).disabled = false;
  auPass.value = "";
  auPass.disabled = false;
  auPass.focus();
  console.log(`overlay: polkit asks (${au.action})`);
}

function auCancel() {
  if (au) invoke("polkit_cancel", { id: au.id });
  au = null;
  close();
}

$("au-cancel").addEventListener("click", auCancel);
$("au-box").addEventListener("submit", async (e) => {
  e.preventDefault();
  if (!au || auBusy || !auPass.value) return;
  auBusy = true;
  auPass.disabled = true;
  /** @type {HTMLButtonElement} */ ($("au-ok")).disabled = true;
  const id = au.id;
  // @ts-ignore: injected by Tauri.
  const done = await window.__TAURI__.core.invoke("polkit_answer", { id, password: auPass.value }).catch((/** @type {unknown} */ err) => {
    $("au-error").hidden = false;
    $("au-error").textContent = String(err);
    return null;
  });
  auPass.value = "";
  if (done === true) {
    au = null;
    close();
  } else {
    // Wrong password (the request now says so), or an error: let them try again.
    await auOpen();
  }
});

// @ts-ignore: injected by Tauri.
if (window.__TAURI__) {
  // @ts-ignore
  window.__TAURI__.event.listen("kompact-auth", async () => {
    if (mode !== "auth" || auBusy) return;
    const next = await invoke("polkit_request");
    // Answered or cancelled elsewhere (polkit gave up): close.
    if (!next) {
      au = null;
      close();
    } else if (!au || next.id !== au.id) void auOpen();
  });
}

// ---- 12f: the input switcher (kompact's keyboard layouts) ----

/** @type {{layouts: {code: string, name: string}[], active: number} | null} */
let kb = null;

async function inOpen() {
  kb = (await invoke("keyboard_state")) || null;
  inRender();
}

function inRender() {
  const out = [stEl("st-row st-head", "Keyboard Layout")];
  (kb?.layouts || []).forEach((l, i) => {
    const r = stEl(`st-row st-item${i === kb?.active ? " st-active" : ""}`);
    r.append(stEl("st-check", i === kb?.active ? "✓" : ""), stEl("st-name", l.name), stEl("st-dim", l.code.replace(/\(.*/, "").toUpperCase()));
    r.addEventListener("click", () => {
      invoke("keyboard_layout", { n: i });
      close();
    });
    out.push(r);
  });
  out.push(stEl("st-sep"));
  const settings = stEl("st-row st-item", "Keyboard Settings…");
  settings.addEventListener("click", () => {
    close();
    invoke("open_settings", { pane: "input" });
  });
  out.push(settings);
  stMenu.replaceChildren(...out);
  stPlace();
}

// A click outside the content (Launchpad: not an icon or the search field; Spotlight: outside
// its box; menu: outside the menu or dialog) closes the overlay.
document.addEventListener("mousedown", (e) => {
  const t = /** @type {HTMLElement} */ (e.target);
  if (mode === "launchpad" && !t.closest("li") && t !== lpSearch) close();
  else if (mode === "spotlight" && !t.closest("#sp-box")) close();
  else if (mode === "menu" && !t.closest(".menu") && !t.closest(".dialog")) close();
  else if (mode === "dockmenu" && !t.closest(".menu")) close();
  else if (mode === "spaces" && !t.closest("#ws-box")) close();
  else if (mode === "status" && !t.closest(".menu")) close();
  // 16b: kompact ends the switcher (and the shell hides us).
  else if (mode === "switcher" && !t.closest(".sw-card")) invoke("window_action", { action: "switcher-cancel" });
});

// @ts-ignore: our own global, called by kompact-shell.
window.kompactOverlay = {
  /** 13b: the overlay is closing: hide every view, and once that empty frame is drawn (two
   * animation frames) let the shell hide the window, so its next first frame is empty too.
   * @param {number} gen */
  clear(gen) {
    for (const v of Object.values(views)) v.hidden = true;
    mode = null;
    swDrawn = "";
    requestAnimationFrame(() => requestAnimationFrame(() => invoke("overlay_cleared", { gen })));
  },
  /** @param {Mode} m */
  async open(m) {
    mode = m;
    views.launchpad.hidden = m !== "launchpad";
    views.spotlight.hidden = m !== "spotlight";
    views.menu.hidden = m !== "menu";
    views.dockmenu.hidden = m !== "dockmenu";
    views.spaces.hidden = m !== "spaces";
    views.select.hidden = m !== "select";
    views.status.hidden = m !== "status";
    views.auth.hidden = m !== "auth";
    views.switcher.hidden = m !== "switcher";
    if (m === "switcher") {
      void swOpen();
      return;
    }
    if (m === "auth") {
      void auOpen();
      return;
    }
    if (m === "status") {
      void stOpen();
      return;
    }
    if (m === "select") {
      selOpen();
      return;
    }
    if (m === "spaces") {
      void wsOpen();
      return;
    }
    if (m === "dockmenu") {
      void dmOpen();
      return;
    }
    if (m === "menu") {
      mnShow("list");
      mnSelect(-1);
      console.log("overlay: menu open");
      return; // the menu doesn't need the app list
    }
    if (m === "launchpad") {
      lpSearch.value = "";
      lpSelected = 0;
      lpRender();
      lpSearch.focus();
    } else {
      spSearch.value = "";
      spHits = [];
      spRender();
      spSearch.focus();
    }
    apps = (await invoke("list_apps")) || [];
    if (mode === "launchpad") lpRender();
    else void spUpdate();
    console.log(`overlay: ${m} open, ${apps.length} apps`);
  },
};

// @ts-ignore: from vibrancy.js. Launchpad covers the whole monitor.
kompactVibrancy(views.launchpad, 0);
document.addEventListener("contextmenu", (e) => e.preventDefault());
console.log("overlay: page ready");
// If a panel button was pressed while this page was still loading, open now.
invoke("overlay_ready").then((/** @type {Mode | null} */ m) => {
  // @ts-ignore
  if (m) window.kompactOverlay.open(m);
});
