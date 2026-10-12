// @ts-check
// Panel (menu bar): the clock, the logo menu (6m-b), the Launchpad and Spotlight buttons
// (6t-c, 6m-a), the focused app's name in bold, like macOS (6m-c, from kompact's window
// list), and the current workspace (6e: click for the overview, scroll to step). The clock wakes
// once a minute, so an idle desktop still costs kompact about one frame a minute (the Phase 4
// budget is 0 flips while idle); 12g: once a second if Settings shows seconds.
"use strict";

// 12g: the look from Settings → Date & Time, and the zone from timedated (formatted in that zone
// explicitly: WebKit keeps the zone it started with when the system's changes).
/** @type {{hour24: boolean, weekday: boolean, date: boolean, seconds: boolean}} */
let clockPrefs = { hour24: true, weekday: true, date: true, seconds: false };
/** @type {string | undefined} */
let clockZone;

/** @param {Date} now */
function clockText(now) {
  // macOS style: "Thu 8 Oct  22:30".
  const zone = clockZone ? { timeZone: clockZone } : {};
  /** @type {Intl.DateTimeFormatOptions} */
  const d = { ...zone };
  if (clockPrefs.weekday) d.weekday = "short";
  if (clockPrefs.date) {
    d.day = "numeric";
    d.month = "short";
  }
  const date = clockPrefs.weekday || clockPrefs.date ? now.toLocaleDateString(undefined, d).replace(/,/g, "") : "";
  /** @type {Intl.DateTimeFormatOptions} */
  const t = { ...zone, hour: clockPrefs.hour24 ? "2-digit" : "numeric", minute: "2-digit", hour12: !clockPrefs.hour24 };
  if (clockPrefs.seconds) t.second = "2-digit";
  const time = now.toLocaleTimeString(undefined, t);
  return date ? `${date} ${time}` : time;
}

/** Milliseconds until the next minute (or, showing seconds, second) starts (at least 1 s for
 * minutes, so a late timer can't spin).
 * @param {Date} now */
function msToNextMinute(now) {
  if (clockPrefs.seconds) return 1000 - now.getMilliseconds();
  return Math.max(1000, 60000 - (now.getSeconds() * 1000 + now.getMilliseconds()));
}

let clockTimer = 0;
function tick() {
  const el = /** @type {HTMLTimeElement} */ (document.getElementById("clock"));
  const now = new Date();
  el.textContent = clockText(now);
  el.dateTime = now.toISOString();
  clearTimeout(clockTimer);
  clockTimer = setTimeout(tick, msToNextMinute(now));
}

/** @param {{zone?: string, clock?: typeof clockPrefs} | undefined} s */
function showClock(s) {
  if (!s) return;
  if (s.clock) clockPrefs = s.clock;
  if (s.zone) clockZone = s.zone;
  console.log(`panel: clock ${clockZone || "local"}, ${JSON.stringify(clockPrefs)}`);
  tick();
}

/** @param {string} cmd @param {Record<string, unknown>} [args] */
function invoke(cmd, args) {
  // @ts-ignore: injected by Tauri (app.withGlobalTauri).
  const tauri = window.__TAURI__;
  if (!tauri) return console.log(`panel: no Tauri IPC (${cmd})`);
  tauri.core.invoke(cmd, args).catch((/** @type {unknown} */ e) => console.log(`panel: ${cmd} failed: ${e}`));
}

// ---- 13d: which status items show (menubar.js's rules, Settings → Control Center's modes) ----

/** @type {MenuBarState} */
const mbState = {};
/** @type {Record<string, MenuBarMode>} */
let mbModes = { ...MENU_BAR_DEFAULTS };
/** Item → its element in the bar. */
const MB_ELEMENTS = { network: "net", bluetooth: "bt", sound: "sound", input: "input", spaces: "spaces", spotlight: "spotlight" };
let mbShown = "";

function applyMenuBar() {
  const bar = /** @type {HTMLElement} */ (document.getElementById("bar"));
  /** @type {Record<string, HTMLElement>} */
  const els = {};
  for (const [item, id] of Object.entries(MB_ELEMENTS)) {
    const el = document.getElementById(id);
    if (!el) continue;
    els[item] = el;
    el.hidden = !menuBarShows(item, mbModes[item], mbState);
  }
  // Too wide (a narrow display, a long app name): fold the least needed items.
  const over = bar.scrollWidth - bar.clientWidth;
  /** @type {string[]} */
  let folded = [];
  if (over > 0) {
    /** @type {Record<string, number>} */
    const widths = {};
    for (const [item, el] of Object.entries(els)) if (!el.hidden) widths[item] = el.getBoundingClientRect().width + 2;
    folded = menuBarFolds(widths, over);
    for (const item of folded) els[item].hidden = true;
  }
  const sig = Object.entries(els).filter(([, el]) => !el.hidden).map(([item]) => item).join(" ") + (folded.length ? `; folded ${folded.join(" ")}` : "");
  if (sig !== mbShown) {
    mbShown = sig;
    console.log(`panel: menu bar shows ${sig || "no status items"}`);
  }
}
window.addEventListener("resize", applyMenuBar);

document.getElementById("logo")?.addEventListener("click", () => invoke("toggle_overlay", { mode: "menu" }));
document.getElementById("apps")?.addEventListener("click", () => invoke("toggle_overlay", { mode: "launchpad" }));
document.getElementById("spotlight")?.addEventListener("click", () => invoke("toggle_overlay", { mode: "spotlight" }));
/** @typedef {{id: number, name: string, focused: boolean, workspace: number}} Win */
/** @typedef {{workspace: number, windows: Win[]}} WindowsMsg */

const WORKSPACES = 9; // kompact's workspace::COUNT

let activeSpace = 1;
/** @param {WindowsMsg | null | undefined} msg */
function showSpaces(msg) {
  if (!msg || typeof msg.workspace !== "number") return;
  // 13d: Desktops shows When Active: away from desktop 1, or windows on another one.
  const others = (msg.windows || []).some((w) => typeof w.workspace === "number" && w.workspace !== msg.workspace);
  const was = JSON.stringify(mbState.spaces);
  mbState.spaces = { current: msg.workspace, others_in_use: others };
  if (JSON.stringify(mbState.spaces) !== was) applyMenuBar();
  if (msg.workspace === activeSpace) return;
  activeSpace = msg.workspace;
  const num = /** @type {HTMLElement} */ (document.getElementById("space-num"));
  num.textContent = String(activeSpace);
  document.getElementById("spaces")?.setAttribute("aria-label", `Desktop ${activeSpace}, show all desktops`);
  console.log(`panel: desktop ${activeSpace}`);
}

/** @param {number} n */
function goTo(n) {
  if (n >= 1 && n <= WORKSPACES && n !== activeSpace) invoke("window_action", { action: "workspace", id: n });
}

const spacesButton = document.getElementById("spaces");
spacesButton?.addEventListener("click", () => invoke("toggle_overlay", { mode: "spaces" }));
// Scrolling over it steps to the previous/next desktop.
let lastWheel = 0;
spacesButton?.addEventListener(
  "wheel",
  (e) => {
    e.preventDefault();
    const now = Date.now();
    if (now - lastWheel < 250) return;
    lastWheel = now;
    goTo(activeSpace + (e.deltaY > 0 ? 1 : -1));
  },
  { passive: false },
);

/** Show the focused window's app name; "kompact" (macOS: "Finder") when none is focused.
 * @param {WindowsMsg | null | undefined} msg */
function showApp(msg) {
  showSpaces(msg);
  const focused = msg?.windows?.find((w) => w.focused);
  const el = /** @type {HTMLElement} */ (document.getElementById("app"));
  const name = focused?.name || "kompact";
  if (el.textContent !== name) el.textContent = name;
}

/** 9e-c: the screen-sharing pill, while kompact says a monitor is being shared.
 * @param {{on?: boolean, outputs?: string[]} | null | undefined} msg */
function showSharing(msg) {
  const el = /** @type {HTMLElement} */ (document.getElementById("sharing"));
  const outputs = msg?.on && Array.isArray(msg.outputs) ? msg.outputs : [];
  const on = outputs.length > 0;
  if (el.hidden === !on && el.title === (on ? `Sharing ${outputs.join(", ")}` : "")) return;
  el.hidden = !on;
  el.title = on ? `Sharing ${outputs.join(", ")}` : "";
  el.setAttribute("aria-label", on ? `Screen sharing: ${outputs.join(", ")}` : "Screen sharing");
  console.log(on ? `panel: sharing ${outputs.join(", ")}` : "panel: sharing off");
}

// ---- 12a: the network item (src-tauri/src/network.rs) ----
const netButton = document.getElementById("net");
let netShown = "";
/** @param {{icon: string, bars: number, name: string | null, has_wifi: boolean} | undefined} s */
function showNetwork(s) {
  if (!netButton || !s) return;
  mbState.network = { icon: s.icon, has_wifi: !!s.has_wifi };
  applyMenuBar();
  netButton.dataset.icon = s.icon;
  netButton.dataset.bars = String(s.bars);
  const what =
    s.icon === "ethernet" ? "Ethernet" : s.icon === "wifi" ? `Wi-Fi: ${s.name || "connected"}` : s.icon === "off" ? "Wi-Fi: off" : s.icon === "connecting" ? "Connecting…" : "Not connected";
  netButton.title = what;
  netButton.setAttribute("aria-label", `Network: ${what}`);
  const sig = `${s.icon} ${s.bars} ${s.name}`;
  if (sig !== netShown) {
    netShown = sig;
    console.log(`panel: network ${sig}`);
  }
}
function loadNetwork() {
  // @ts-ignore: injected by Tauri.
  window.__TAURI__?.core.invoke("network_summary").then(showNetwork, (/** @type {unknown} */ e) => console.log(`panel: network: ${e}`));
}
netButton?.addEventListener("click", () => {
  // Its right edge on the monitor: the menu bar spans the monitor from its left edge.
  const r = netButton.getBoundingClientRect();
  invoke("open_status", { which: "network", x: r.right });
});

// ---- 12f: the input switcher (kompact's `keyboard` message) ----
const inputButton = document.getElementById("input");
/** @param {{layouts: {code: string, name: string}[], active: number} | undefined} k */
function showKeyboard(k) {
  if (!inputButton || !k) return;
  mbState.input = { layouts: k.layouts.length };
  applyMenuBar();
  const l = k.layouts[k.active];
  const code = (l?.code || "").replace(/\(.*/, "").toUpperCase();
  const label = document.getElementById("input-code");
  if (label) label.textContent = code;
  inputButton.title = l ? `Keyboard: ${l.name}` : "Keyboard layout";
  console.log(`panel: keyboard ${code} (${k.active + 1} of ${k.layouts.length})`);
}
inputButton?.addEventListener("click", () => invoke("open_status", { which: "input", x: inputButton.getBoundingClientRect().right }));

// ---- 12d: the microphone dot, Do Not Disturb, Control Center, the clock ----
const micDot = document.getElementById("mic-use");
let micShown = "";
/** @param {string[]} apps recording now */
function showRecording(apps) {
  if (!micDot) return;
  micDot.hidden = !apps.length;
  micDot.title = apps.length ? `Microphone in use: ${apps.join(", ")}` : "";
  const sig = apps.join(", ");
  if (sig !== micShown) {
    micShown = sig;
    console.log(apps.length ? `panel: microphone in use by ${sig}` : "panel: microphone free");
  }
}
const dndMark = document.getElementById("dnd");
/** @param {{dnd: boolean} | undefined} p */
function showDnd(p) {
  if (dndMark && p) dndMark.hidden = !p.dnd;
}
function loadDnd() {
  // @ts-ignore: injected by Tauri.
  window.__TAURI__?.core.invoke("notify_prefs").then(showDnd, () => {});
}
for (const [id, which] of [["control", "control"], ["clock-btn", "center"]]) {
  const b = document.getElementById(id);
  b?.addEventListener("click", () => invoke("open_status", { which, x: b.getBoundingClientRect().right }));
}

// ---- 12c: the sound item (src-tauri/src/sound.rs) ----
const soundButton = document.getElementById("sound");
let soundShown = "";
/** @param {{output: boolean, volume?: number, muted?: boolean, waves?: number} | undefined} s */
function showSound(s) {
  if (!soundButton || !s) return;
  showRecording(/** @type {any} */ (s).recording || []);
  mbState.sound = { output: !!s.output, muted: !!s.muted, playing: /** @type {any} */ (s).playing || [] };
  applyMenuBar();
  soundButton.dataset.waves = String(s.waves ?? 0);
  soundButton.dataset.muted = String(!!s.muted);
  const what = s.muted ? "Sound: muted" : `Sound: ${Math.round((s.volume || 0) * 100)}%`;
  soundButton.title = what;
  soundButton.setAttribute("aria-label", what);
  if (what !== soundShown) {
    soundShown = what;
    console.log(`panel: ${what.toLowerCase()}`);
  }
}
function loadSound() {
  // @ts-ignore: injected by Tauri.
  window.__TAURI__?.core.invoke("sound_level").then(showSound, (/** @type {unknown} */ e) => console.log(`panel: sound: ${e}`));
}
soundButton?.addEventListener("click", () => {
  const r = soundButton.getBoundingClientRect();
  invoke("open_status", { which: "sound", x: r.right });
});

// ---- 12b: the Bluetooth item (src-tauri/src/bluetooth.rs) ----
const btButton = document.getElementById("bt");
let btShown = "";
/** @param {{adapter: string | null, powered?: boolean, devices?: {connected: boolean, paired: boolean, name: string}[]} | undefined} s */
function showBluetooth(s) {
  if (!btButton || !s) return;
  const on = (s.devices || []).filter((d) => d.paired && d.connected);
  mbState.bluetooth = { adapter: s.adapter, powered: !!s.powered, connected: on.length };
  applyMenuBar();
  const state = !s.powered ? "off" : on.length ? "connected" : "on";
  btButton.dataset.state = state;
  const what = state === "off" ? "Bluetooth: off" : on.length ? `Bluetooth: ${on.map((d) => d.name).join(", ")}` : "Bluetooth: on";
  btButton.title = what;
  btButton.setAttribute("aria-label", what);
  const sig = `${!!s.adapter} ${state} ${on.length}`;
  if (sig !== btShown) {
    btShown = sig;
    console.log(`panel: bluetooth ${sig}`);
  }
}
function loadBluetooth() {
  // @ts-ignore: injected by Tauri.
  window.__TAURI__?.core.invoke("bluetooth_state").then(showBluetooth, (/** @type {unknown} */ e) => console.log(`panel: bluetooth: ${e}`));
}
btButton?.addEventListener("click", () => {
  const r = btButton.getBoundingClientRect();
  invoke("open_status", { which: "bluetooth", x: r.right });
});

// @ts-ignore: injected by Tauri (app.withGlobalTauri).
const tauriEvents = window.__TAURI__?.event;
if (tauriEvents) {
  tauriEvents.listen("kompact-windows", (/** @type {{payload: any}} */ e) => showApp(e.payload));
  tauriEvents.listen("kompact-sharing", (/** @type {{payload: any}} */ e) => showSharing(e.payload));
  tauriEvents.listen("kompact-network-changed", loadNetwork);
  loadNetwork();
  tauriEvents.listen("kompact-bluetooth-changed", loadBluetooth);
  loadBluetooth();
  tauriEvents.listen("kompact-sound-changed", loadSound);
  loadSound();
  tauriEvents.listen("kompact-notify-prefs", (/** @type {{payload: any}} */ e) => showDnd(e.payload));
  tauriEvents.listen("kompact-keyboard", (/** @type {{payload: any}} */ e) => showKeyboard(e.payload));
  tauriEvents.listen("kompact-clock", (/** @type {{payload: any}} */ e) => showClock(e.payload));
  // 13d
  /** @param {Record<string, MenuBarMode> | undefined} m */
  const setModes = (m) => {
    if (!m) return;
    mbModes = { ...MENU_BAR_DEFAULTS, ...m };
    applyMenuBar();
  };
  tauriEvents.listen("kompact-menubar", (/** @type {{payload: any}} */ e) => setModes(e.payload));
  // @ts-ignore
  window.__TAURI__.core.invoke("menubar_prefs").then(setModes, () => {});
  // @ts-ignore
  window.__TAURI__.core.invoke("datetime_state").then(showClock, () => {});
  // @ts-ignore
  window.__TAURI__.core.invoke("keyboard_state").then(showKeyboard, () => {});
  loadDnd();
  // @ts-ignore
  window.__TAURI__.core.invoke("compositor_state").then(showApp, () => {});
  // @ts-ignore
  window.__TAURI__.core.invoke("sharing_state").then(showSharing, () => {});
}

// @ts-ignore: from vibrancy.js. The menu bar is at the top of the monitor.
kompactVibrancy(document.getElementById("bar"), 0);

applyMenuBar();
tick();
console.log("panel: clock started");
