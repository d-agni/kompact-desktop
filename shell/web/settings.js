// @ts-check
// 10c: System Settings → Keyboard Shortcuts. Everything comes from kompact's `keymap` message
// (via the shell, event `kompact-keymap`); every change is a `keys …` request (`keys_command`)
// that kompact checks, applies and saves, answering with `kompact-keys-result` and a new keymap.
// New keys are recorded by kompact (`keys record` → `kompact-recorded`), since Super shortcuts
// never reach a window.
"use strict";

/** @typedef {{id: string, label: string, group: string, keys: string[], preset_keys: string[], changed: boolean, family: boolean, required: boolean}} ActionInfo */
/** @typedef {{preset: string, presets: {id: string, label: string}[], groups: string[], actions: ActionInfo[]}} Keymap */

// @ts-ignore: injected by Tauri (app.withGlobalTauri).
const tauri = window.__TAURI__;
const $ = (/** @type {string} */ id) => /** @type {HTMLElement} */ (document.getElementById(id));

/** @type {Keymap | null} */
let keymap = null;
/** What's being recorded: an action and which of its combinations (`keys.length` adds one). */
/** @type {{id: string, index: number} | null} */
let recording = null;
/** The row the last request was about, for its message. */
let lastAction = "";
/** @type {Record<string, string>} */
const notes = {};

/** @param {string} args */
function keys(args) {
  if (!tauri) return console.log(`settings: no Tauri IPC (keys ${args})`);
  tauri.core.invoke("keys_command", { args }).catch((/** @type {unknown} */ e) => {
    status(`kompact isn't reachable: ${e}`, true);
  });
}

/** @param {string} text @param {boolean} [error] */
function status(text, error = false) {
  const el = $("status");
  el.textContent = text;
  el.classList.toggle("error", error);
}

// ---- How keys look: macOS symbols for the macOS set, words otherwise.

const MAC_MODS = /** @type {Record<string, string>} */ ({ Ctrl: "⌃", Alt: "⌥", Shift: "⇧", Super: "⌘" });
const MAC_KEYS = /** @type {Record<string, string>} */ ({
  Left: "←", Right: "→", Up: "↑", Down: "↓", Return: "↩", Escape: "⎋", Tab: "⇥",
  BackSpace: "⌫", Delete: "⌦", space: "Space", Print: "Print Screen", "1-9": "1–9",
  XF86AudioRaiseVolume: "Volume Up", XF86AudioLowerVolume: "Volume Down", XF86AudioMute: "Mute", XF86AudioMicMute: "Mic Mute",
});
const PC_KEYS = /** @type {Record<string, string>} */ ({
  Left: "←", Right: "→", Up: "↑", Down: "↓", Return: "Enter", Escape: "Esc", Tab: "Tab",
  BackSpace: "Backspace", space: "Space", Print: "PrtSc", "1-9": "1–9",
  XF86AudioRaiseVolume: "Volume Up", XF86AudioLowerVolume: "Volume Down", XF86AudioMute: "Mute", XF86AudioMicMute: "Mic Mute",
});

/** The keycaps for "Ctrl+Super+Left" in the current set.
 * @param {string} combo @returns {string[]} */
function caps(combo) {
  const parts = combo.split("+");
  // "1-9" has no "+", but a lone tap ("Super") is all modifier.
  const mods = new Set(["Ctrl", "Alt", "Shift", "Super"]);
  const key = mods.has(parts[parts.length - 1]) ? "" : parts.pop() || "";
  const mac = keymap?.preset === "macos";
  const out = parts.map((m) => (mac ? MAC_MODS[m] || m : m === "Super" ? (keymap?.preset === "windows" ? "Win" : "Super") : m));
  if (key) out.push((mac ? MAC_KEYS : PC_KEYS)[key] || key);
  else out.push("tap");
  return out;
}

// ---- Drawing.

function render() {
  $("offline").hidden = keymap !== null;
  if (!keymap) return;
  const km = keymap;
  const presets = $("presets");
  presets.replaceChildren(
    ...km.presets.map((p) => {
      const b = document.createElement("button");
      b.textContent = p.label;
      b.setAttribute("role", "radio");
      b.setAttribute("aria-checked", String(p.id === km.preset));
      b.addEventListener("click", () => choosePreset(p.id, p.label));
      return b;
    }),
  );

  const filter = /** @type {HTMLInputElement} */ ($("search")).value.trim().toLowerCase();
  const groups = $("groups");
  groups.replaceChildren();
  for (const g of km.groups) {
    const rows = km.actions.filter((a) => a.group === g && matches(a, filter));
    if (!rows.length) continue;
    const title = document.createElement("h2");
    title.className = "group-title";
    title.textContent = g;
    const card = document.createElement("section");
    card.className = "card";
    card.setAttribute("aria-label", g);
    for (const a of rows) card.append(row(a));
    groups.append(title, card);
  }
}

/** @param {ActionInfo} a @param {string} filter */
function matches(a, filter) {
  if (!filter) return true;
  return [a.label, a.id, ...a.keys, ...a.keys.flatMap(caps)].some((t) => t.toLowerCase().includes(filter));
}

/** @param {ActionInfo} a */
function row(a) {
  const r = document.createElement("div");
  r.className = "row";
  const label = document.createElement("span");
  label.className = "row-label";
  if (a.changed) {
    const dot = document.createElement("span");
    dot.className = "changed-dot";
    dot.title = "Changed from the set";
    label.append(dot);
  }
  label.append(a.label);
  if (notes[a.id]) {
    const n = document.createElement("span");
    n.className = "note";
    n.textContent = notes[a.id];
    label.append(n);
  }
  const box = document.createElement("div");
  box.className = "keys";
  a.keys.forEach((k, i) => {
    if (i > 0) box.append(Object.assign(document.createElement("span"), { className: "or", textContent: "or" }));
    box.append(recording?.id === a.id && recording.index === i ? recordingChip(a) : combo(a, k, i));
  });
  if (recording?.id === a.id && recording.index >= a.keys.length) box.append(recordingChip(a));
  else if (!a.keys.length) box.append(Object.assign(document.createElement("span"), { className: "none", textContent: "none" }));
  if (a.keys.length < 2 && !(recording?.id === a.id)) {
    const add = document.createElement("button");
    add.className = "icon-btn";
    add.textContent = "+";
    add.title = a.keys.length ? "Add another shortcut" : "Add a shortcut";
    add.setAttribute("aria-label", `${add.title} for ${a.label}`);
    add.addEventListener("click", () => startRecording(a.id, a.keys.length));
    box.append(add);
  }
  if (a.changed) {
    const reset = document.createElement("button");
    reset.className = "link";
    reset.textContent = "Restore";
    reset.title = `Back to ${a.preset_keys.length ? a.preset_keys.map((k) => caps(k).join(" ")).join(" or ") : "none"}`;
    reset.addEventListener("click", () => {
      lastAction = a.id;
      keys(`reset ${a.id}`);
    });
    box.append(reset);
  }
  r.append(label, box);
  return r;
}

/** @param {ActionInfo} a @param {string} k @param {number} i */
function combo(a, k, i) {
  const c = document.createElement("button");
  c.className = "combo";
  c.title = "Click and press new keys";
  c.setAttribute("aria-label", `${a.label}: ${caps(k).join(" ")}. Change`);
  for (const t of caps(k)) c.append(Object.assign(document.createElement("span"), { className: "cap", textContent: t }));
  c.addEventListener("click", () => startRecording(a.id, i));
  const lastOne = a.required && a.keys.length === 1;
  if (!lastOne) {
    const x = document.createElement("span");
    x.className = "remove";
    x.textContent = "×";
    x.title = "Remove this shortcut";
    x.addEventListener("click", (e) => {
      e.stopPropagation();
      bind(a, a.keys.filter((_, j) => j !== i));
    });
    c.append(x);
  }
  return c;
}

/** @param {ActionInfo} a */
function recordingChip(a) {
  const c = document.createElement("span");
  c.className = "recording";
  c.textContent = a.family ? "Press the modifiers with any digit… (Esc cancels)" : "Press the new shortcut… (Esc cancels)";
  return c;
}

// ---- Changing things.

/** @param {ActionInfo} a @param {string[]} list */
function bind(a, list) {
  lastAction = a.id;
  delete notes[a.id];
  keys(`bind ${a.id} ${list.length ? list.join(", ") : "none"}`);
}

/** @param {string} id @param {number} index */
function startRecording(id, index) {
  if (recording) keys("record-cancel");
  recording = { id, index };
  delete notes[id];
  status("");
  render();
  keys("record");
}

/** @param {{keys?: string, cancelled?: string}} msg */
function onRecorded(msg) {
  const rec = recording;
  recording = null;
  if (!rec || !keymap) return render();
  const a = keymap.actions.find((x) => x.id === rec.id);
  if (!a) return render();
  if (msg.keys) {
    const list = [...a.keys];
    list[rec.index] = msg.keys;
    bind(a, list.slice(0, 2));
  } else if (msg.cancelled && msg.cancelled !== "Esc" && msg.cancelled !== "cancelled") {
    status(msg.cancelled === "timeout" ? "No keys pressed: nothing changed." : `Recording stopped (${msg.cancelled}).`);
  }
  render();
}

/** @param {{ok: boolean, message: string, request: string}} msg */
function onResult(msg) {
  if (msg.ok) {
    if (msg.message) status(msg.message.replace(/ \(saved to .*\)$/, " — saved"));
  } else if (msg.request !== "record" || recording === null) {
    if (lastAction && msg.request.includes(lastAction)) notes[lastAction] = msg.message;
    status(msg.message, true);
    render();
  }
}

/** @param {string} id @param {string} label */
function choosePreset(id, label) {
  if (!keymap || id === keymap.preset) return;
  const changed = keymap.actions.filter((a) => a.changed).length;
  if (!changed) return keys(`preset ${id} drop`);
  ask(
    `Switch to the ${label} shortcuts?`,
    `You changed ${changed} shortcut${changed === 1 ? "" : "s"}. Keep them on top of the ${label} set? (Any that clash with it are dropped.)`,
    [
      ["Cancel", () => {}],
      ["Use the Set's Keys", () => keys(`preset ${id} drop`)],
      ["Keep My Changes", () => keys(`preset ${id} keep`), true],
    ],
  );
}

/** @param {string} title @param {string} text @param {[string, () => void, boolean?][]} buttons */
function ask(title, text, buttons) {
  $("dlg-title").textContent = title;
  $("dlg-text").textContent = text;
  $("dlg-input").hidden = $("dlg-code").hidden = true;
  dialogFor = 0;
  const box = $("dlg-buttons");
  box.replaceChildren(
    ...buttons.map(([name, run, primary]) => {
      const b = document.createElement("button");
      b.className = primary ? "btn primary" : "btn";
      b.textContent = name;
      b.addEventListener("click", () => {
        $("dialog").hidden = true;
        run();
      });
      return b;
    }),
  );
  $("dialog").hidden = false;
  /** @type {HTMLElement | null} */ (box.lastElementChild)?.focus();
}

$("restore-all").addEventListener("click", () => {
  const label = keymap?.presets.find((p) => p.id === keymap?.preset)?.label || "the set";
  ask("Restore every shortcut?", `All shortcuts go back to the ${label} keys.`, [
    ["Cancel", () => {}],
    ["Restore", () => keys("reset-all"), true],
  ]);
});

// ---- 10c: the window's own title bar and edges.
const win = tauri?.window?.getCurrentWindow?.();
/** @param {string} what @param {Promise<unknown> | undefined} p */
function winCall(what, p) {
  p?.catch((/** @type {unknown} */ e) => console.log(`settings: ${what} failed: ${e}`));
}
document.querySelectorAll("[data-win]").forEach((b) =>
  b.addEventListener("click", () => {
    const act = /** @type {HTMLElement} */ (b).dataset.win;
    if (act === "close") winCall("close", win?.close());
    else if (act === "minimize") winCall("minimize", win?.minimize());
    else winCall("maximize", win?.toggleMaximize());
  }),
);
// A press only becomes a move once the pointer has moved a little: kompact takes a maximized
// window out of maximized as soon as a move starts, so a plain click must not start one.
/** @type {{x: number, y: number} | null} */
let pressed = null;
document.querySelectorAll("[data-drag]").forEach((el) => {
  el.addEventListener("mousedown", (e) => {
    const me = /** @type {MouseEvent} */ (e);
    if (me.button !== 0 || /** @type {HTMLElement} */ (me.target).closest("button, input, select")) return;
    if (me.detail === 2) {
      pressed = null;
      winCall("maximize", win?.toggleMaximize());
    } else {
      pressed = { x: me.screenX, y: me.screenY };
    }
  });
});
document.addEventListener("mousemove", (e) => {
  if (!pressed) return;
  if (!(e.buttons & 1)) {
    pressed = null;
    return;
  }
  if (Math.hypot(e.screenX - pressed.x, e.screenY - pressed.y) >= 4) {
    pressed = null;
    winCall("move", win?.startDragging());
  }
});
document.addEventListener("mouseup", () => (pressed = null));
document.querySelectorAll("[data-edge]").forEach((el) =>
  el.addEventListener("mousedown", (e) => {
    if (/** @type {MouseEvent} */ (e).button === 0) winCall("resize", win?.startResizeDragging(/** @type {HTMLElement} */ (el).dataset.edge));
  }),
);
window.addEventListener("focus", () => document.body.classList.remove("inactive"));
window.addEventListener("blur", () => document.body.classList.add("inactive"));

// ---- 10d-1: panes. The last one shown opens again next time.

let paneShown = "";
/** 12g: ticks the Date & Time pane's clock while it shows (declared here: `showPane` runs early). */
let dtTimer = 0;
/** @param {string} name */
function showPane(name) {
  // 12b: Bluetooth looks for devices only while its pane shows.
  if (paneShown === "bluetooth" && name !== "bluetooth") void tauri?.core.invoke("bluetooth_discover", { on: false }).catch(() => {});
  paneShown = name;
  document.querySelectorAll("[data-pane]").forEach((b) => {
    const on = /** @type {HTMLElement} */ (b).dataset.pane === name;
    b.classList.toggle("selected", on);
    if (on) b.setAttribute("aria-current", "page");
    else b.removeAttribute("aria-current");
  });
  document.querySelectorAll("main.content").forEach((m) => {
    /** @type {HTMLElement} */ (m).hidden = m.id !== `pane-${name}`;
  });
  try {
    localStorage.setItem("settings-pane", name);
  } catch {}
  // After this script has run (the first pane is shown before `pictures` exists).
  if (name === "wallpaper") setTimeout(loadPictures, 0);
  if (name === "sharing") setTimeout(loadSharing, 0);
  if (name === "network") setTimeout(() => loadNetwork(true), 0);
  if (name === "sound") setTimeout(loadSound, 0);
  if (name === "notifications") setTimeout(loadNotify, 0);
  if (name === "assistant") setTimeout(() => (void loadAssistant(), void loadMenuBar()), 0);
  if (name === "input") setTimeout(loadCatalogue, 0);
  if (name === "displays") setTimeout(loadDisplays, 0);
  // 12j
  if (name === "about") setTimeout(loadAbout, 0);
  if (name === "control") setTimeout(loadMenuBar, 0);
  if (name === "users") setTimeout(loadUsers, 0);
  if (name === "defaults") setTimeout(loadDefaults, 0);
  // 12g: the time ticks while the pane shows.
  clearInterval(dtTimer);
  if (name === "datetime") {
    setTimeout(loadDatetime, 0);
    dtTimer = setInterval(renderDatetime, 1000);
  }
  if (name === "bluetooth")
    setTimeout(() => {
      void loadBluetooth();
      void btCall("bluetooth_discover", { on: true });
    }, 0);
}
document.querySelectorAll("[data-pane]").forEach((b) =>
  b.addEventListener("click", () => showPane(/** @type {HTMLElement} */ (b).dataset.pane || "keys")),
);
let firstPane = "keys";
try {
  const saved = localStorage.getItem("settings-pane");
  if (saved && document.getElementById(`pane-${saved}`)) firstPane = saved;
} catch {}
showPane(firstPane);

// ---- 10d-1: Dock. The shell keeps these (`~/.config/kompact/dock-prefs`); the dock reloads
// them on `kompact-dock-changed`, and following the pointer is passed on to kompact.

/** @typedef {{hide: "smart" | "never" | "always", magnify: boolean, follow: boolean, size: number,
 *   zoom: number, dots: boolean, bounce: boolean, position: "left" | "bottom" | "right"}} DockPrefs */
/** @type {DockPrefs | null} */
let dock = null;

function renderDock() {
  const p = dock;
  document.querySelectorAll("[data-hide]").forEach((b) => b.setAttribute("aria-checked", String(p?.hide === /** @type {HTMLElement} */ (b).dataset.hide)));
  document.querySelectorAll("[data-pos]").forEach((b) => b.setAttribute("aria-checked", String((p?.position || "bottom") === /** @type {HTMLElement} */ (b).dataset.pos)));
  /** @type {[string, keyof DockPrefs][]} */
  const switches = [["dock-magnify", "magnify"], ["dock-follow", "follow"], ["dock-dots", "dots"], ["dock-bounce", "bounce"]];
  for (const [id, key] of switches) {
    const el = /** @type {HTMLInputElement} */ ($(id));
    el.checked = !!p?.[key];
    el.disabled = !p;
  }
  // 10d-1b: a slider being dragged keeps its own value until it's let go.
  const size = /** @type {HTMLInputElement} */ ($("dock-size"));
  const zoom = /** @type {HTMLInputElement} */ ($("dock-zoom"));
  if (p && sliding !== "size") size.value = String(p.size);
  if (p && sliding !== "zoom") zoom.value = String(p.zoom);
  size.disabled = !p;
  zoom.disabled = !p || !p.magnify;
  $("zoom-row").classList.toggle("off", !p?.magnify);
  $("dock-size-val").textContent = `${size.value} px`;
  $("dock-zoom-val").textContent = `${zoom.value}%`;
}

/** Which slider the pointer is dragging, if any. @type {string | null} */
let sliding = null;
let slideTimer = 0;
/** @param {"size" | "zoom"} key */
function hookSlider(key) {
  const el = /** @type {HTMLInputElement} */ ($(`dock-${key}`));
  // The dock follows while dragging (a few times a second, not every pixel), and once more on release.
  el.addEventListener("input", () => {
    sliding = key;
    $(`dock-${key}-val`).textContent = key === "size" ? `${el.value} px` : `${el.value}%`;
    clearTimeout(slideTimer);
    slideTimer = setTimeout(() => void setDock({ [key]: Number(el.value) }), 120);
  });
  el.addEventListener("change", () => {
    sliding = null;
    clearTimeout(slideTimer);
    void setDock({ [key]: Number(el.value) });
  });
}
hookSlider("size");
hookSlider("zoom");

async function loadDock() {
  if (!tauri) return;
  try {
    dock = await tauri.core.invoke("dock_prefs");
  } catch (e) {
    $("dock-status").textContent = `Couldn't read the Dock settings: ${e}`;
  }
  renderDock();
}

/** @param {Partial<DockPrefs>} change */
async function setDock(change) {
  if (!dock || !tauri) return;
  const next = { ...dock, ...change };
  const el = $("dock-status");
  try {
    await tauri.core.invoke("set_dock_prefs", { prefs: next });
    dock = next;
    el.textContent = "";
    el.classList.remove("error");
    console.log(
      `settings: dock hide=${next.hide} magnify=${next.magnify} follow=${next.follow} size=${next.size} zoom=${next.zoom} dots=${next.dots} bounce=${next.bounce} position=${next.position}`,
    );
  } catch (e) {
    el.textContent = `Couldn't save: ${e}`;
    el.classList.add("error");
  }
  renderDock();
}

document.querySelectorAll("[data-pos]").forEach((b) =>
  b.addEventListener("click", () => {
    const position = /** @type {DockPrefs["position"]} */ (/** @type {HTMLElement} */ (b).dataset.pos);
    if (position && dock?.position !== position) void setDock({ position });
  }),
);
document.querySelectorAll("[data-hide]").forEach((b) =>
  b.addEventListener("click", () => {
    const hide = /** @type {DockPrefs["hide"]} */ (/** @type {HTMLElement} */ (b).dataset.hide);
    if (hide && dock?.hide !== hide) void setDock({ hide });
  }),
);
$("dock-magnify").addEventListener("change", (e) => void setDock({ magnify: /** @type {HTMLInputElement} */ (e.target).checked }));
$("dock-follow").addEventListener("change", (e) => void setDock({ follow: /** @type {HTMLInputElement} */ (e.target).checked }));
$("dock-dots").addEventListener("change", (e) => void setDock({ dots: /** @type {HTMLInputElement} */ (e.target).checked }));
$("dock-bounce").addEventListener("change", (e) => void setDock({ bounce: /** @type {HTMLInputElement} */ (e.target).checked }));
renderDock();

// ---- 11c: kompact's own settings (Desktop & Dock → Windows). kompact keeps them and answers
// every change with a `settings` message (`kompact-settings`).

/** @typedef {{version?: string, animations: boolean, title_double_click: string, focus_follows_mouse: boolean,
 *   appearance: string, dark: boolean, accent: string, buttons: string,
 *   shots_dir: string, shots_dir_default: boolean, shot_banner: boolean, shot_pointer: boolean,
 *   pointer_speed: number, pointer_accel: boolean, natural_scroll: boolean, mouse_natural_scroll: boolean,
 *   scroll_speed: number, left_handed: boolean,
 *   tap_to_click: boolean, repeat_delay: number, repeat_rate: number, layouts: string, caps: string,
 *   lock_after: number, screen_off_after: number, terminal?: string, terminal_auto?: string, locked: string[], message?: string}} KompactSettings */
/** @type {KompactSettings | null} */
let kset = null;

// 10d-3: Appearance (kompact's settings too). appearance.js shows the colours, from the same message.
const ACCENT_SWATCH = /** @type {Record<string, string>} */ ({
  blue: "#0a84ff", purple: "#a550a7", pink: "#f74f9e", red: "#ff453a",
  orange: "#ff9f0a", yellow: "#ffd60a", green: "#32d74b", graphite: "#98989d",
});
function renderAppearance() {
  const k = kset;
  document.querySelectorAll("[data-look]").forEach((b) => {
    b.setAttribute("aria-checked", String(k?.appearance === /** @type {HTMLElement} */ (b).dataset.look));
    /** @type {HTMLButtonElement} */ (b).disabled = !k;
  });
  document.querySelectorAll("[data-side]").forEach((b) => {
    b.setAttribute("aria-checked", String(k?.buttons === /** @type {HTMLElement} */ (b).dataset.side));
    /** @type {HTMLButtonElement} */ (b).disabled = !k;
  });
  $("look-hint").textContent =
    k?.appearance === "auto" ? `Light from 7:00 to 19:00; ${k.dark ? "dark" : "light"} now.` : "Auto is light from 7:00 to 19:00.";
  $("accents").replaceChildren(
    ...Object.entries(ACCENT_SWATCH).map(([name, colour]) => {
      const b = document.createElement("button");
      b.className = "swatch";
      b.style.background = colour;
      b.title = name[0].toUpperCase() + name.slice(1);
      b.setAttribute("role", "radio");
      b.setAttribute("aria-checked", String(k?.accent === name));
      b.disabled = !k;
      b.addEventListener("click", () => k?.accent !== name && setKompact("accent", name));
      return b;
    }),
  );
}
document.querySelectorAll("[data-look]").forEach((b) =>
  b.addEventListener("click", () => {
    const v = /** @type {HTMLElement} */ (b).dataset.look;
    if (v && kset?.appearance !== v) setKompact("appearance", v);
  }),
);
document.querySelectorAll("[data-side]").forEach((b) =>
  b.addEventListener("click", () => {
    const v = /** @type {HTMLElement} */ (b).dataset.side;
    if (v && kset?.buttons !== v) setKompact("buttons", v);
  }),
);

// 10d-4: Screenshots (kompact's settings too).
function renderShots() {
  const k = kset;
  const locked = !!k?.locked.includes("shots-dir");
  $("shots-dir").textContent = k
    ? `${k.shots_dir || "?"}${locked ? " (KOMPACT_SCREENSHOT_DIR, for this session)" : k.shots_dir_default ? " (the default)" : ""}`
    : "…";
  /** @type {HTMLButtonElement} */ ($("shots-choose")).disabled = !k || locked;
  /** @type {HTMLButtonElement} */ ($("shots-open")).disabled = !k?.shots_dir;
  $("shots-default").hidden = !k || locked || k.shots_dir_default;
  for (const [id, key] of /** @type {[string, "shot_banner" | "shot_pointer"][]} */ ([["shot-banner", "shot_banner"], ["shot-pointer", "shot_pointer"]])) {
    const el = /** @type {HTMLInputElement} */ ($(id));
    el.checked = !!k?.[key];
    el.disabled = !k;
  }
  // The keys, from the keymap (Keyboard Shortcuts → Screenshots).
  const shots = keymap?.actions.filter((a) => a.group === "Screenshots") || [];
  $("shot-keys").textContent = shots.length
    ? shots.map((a) => `${a.label}: ${a.keys.length ? a.keys.map((k) => caps(k).join(" ")).join(" or ") : "none"}`).join(" · ")
    : "…";
}

/** @param {string} path */
function setShotsDir(path) {
  if (!tauri) return;
  kompactStatus = "shots-status";
  console.log(`settings: screenshots folder ${path || "(default)"}`);
  tauri.core.invoke("kompact_set_path", { key: "shots-dir", path }).catch((/** @type {unknown} */ e) => {
    const el = $("shots-status");
    el.textContent = `kompact isn't reachable: ${e}`;
    el.classList.add("error");
  });
}
$("shots-choose").addEventListener("click", () => tauri?.core.invoke("choose_shots_folder", { current: kset?.shots_dir || null }));
$("shots-default").addEventListener("click", () => setShotsDir(""));
$("shots-open").addEventListener("click", () => {
  if (!kset?.shots_dir) return;
  tauri?.core.invoke("open_folder", { path: kset.shots_dir }).catch((/** @type {unknown} */ e) => {
    const el = $("shots-status");
    el.textContent = `Couldn't open it: ${e}`;
    el.classList.add("error");
  });
});
$("shot-banner").addEventListener("change", (e) => setKompact("shot-banner", /** @type {HTMLInputElement} */ (e.target).checked ? "on" : "off"));
$("shot-pointer").addEventListener("change", (e) => setKompact("shot-pointer", /** @type {HTMLInputElement} */ (e.target).checked ? "on" : "off"));
$("shot-keys-edit").addEventListener("click", () => {
  showPane("keys");
  const search = /** @type {HTMLInputElement} */ ($("search"));
  search.value = "screenshot";
  render();
});

// ---- 12d: Notifications (the shell's notification server) ----

/** @typedef {{dnd: boolean, banner_ms: number, muted: string[], seen: string[]}} NotifyPrefs */
/** @type {NotifyPrefs | null} */
let nt = null;

async function loadNotify() {
  nt = tauri ? await tauri.core.invoke("notify_prefs").catch(() => null) : null;
  renderNotify();
}

/** @param {Partial<NotifyPrefs>} change */
async function setNotify(change) {
  if (!nt || !tauri) return;
  const next = { ...nt, ...change };
  try {
    nt = await tauri.core.invoke("set_notify_prefs", { dnd: next.dnd, bannerMs: next.banner_ms, muted: next.muted });
    $("nt-status").textContent = "";
  } catch (e) {
    $("nt-status").textContent = String(e);
    $("nt-status").classList.add("error");
  }
  renderNotify();
}

function renderNotify() {
  const p = nt;
  const dnd = /** @type {HTMLInputElement} */ ($("nt-dnd"));
  dnd.checked = !!p?.dnd;
  dnd.disabled = !p;
  document.querySelectorAll("[data-ms]").forEach((b) => b.setAttribute("aria-checked", String(p?.banner_ms === Number(/** @type {HTMLElement} */ (b).dataset.ms))));
  const list = $("nt-apps");
  list.replaceChildren(
    ...(p?.seen || []).map((app) => {
      const row = document.createElement("label");
      row.className = "row";
      row.append(netLabel(app, p?.muted.includes(app) ? "Off: no banners, not in the Notification Center" : undefined));
      const sw = Object.assign(document.createElement("input"), { type: "checkbox", className: "switch", checked: !p?.muted.includes(app) });
      sw.setAttribute("role", "switch");
      sw.setAttribute("aria-label", `Notifications from ${app}`);
      sw.addEventListener("change", () => {
        const muted = (nt?.muted || []).filter((m) => m !== app);
        if (!sw.checked) muted.push(app);
        void setNotify({ muted });
      });
      row.append(sw);
      return row;
    }),
  );
  if (!p?.seen.length) list.append(Object.assign(document.createElement("div"), { className: "row muted", textContent: "Apps show up here when they first notify." }));
}

$("nt-dnd").addEventListener("change", (e) => setNotify({ dnd: /** @type {HTMLInputElement} */ (e.target).checked }));
document.querySelectorAll("[data-ms]").forEach((b) =>
  b.addEventListener("click", () => {
    const ms = Number(/** @type {HTMLElement} */ (b).dataset.ms);
    if (nt?.banner_ms !== ms) void setNotify({ banner_ms: ms });
  }),
);

// ---- 19c-2, 19h: Assistant (voice, what it may do, what it did; the menu-bar choice is 13d's) ----

/** @typedef {{at: number, kind: string, tool: string, text: string}} ActivityEntry */
/** @type {ActivityEntry[]} */
let activity = [];
/** 19d @type {{ready: boolean, model: string | null, missing: string | null} | null} */
let voice = null;
/** 19h-2 @type {{ready: boolean, voice: string, speak: "voice" | "always" | "never"} | null} */
let speech = null;
/** 19f @type {{ready: boolean, model: string, missing: string | null} | null} */
let model = null;
/** 20d @type {{mode: "always" | "ask" | "off", place: string, zone_place: string, curl: boolean} | null} */
let internet = null;
/** 20d-2: Settings → Assistant → Cloud AI (`cloud_state`). @type {any} */
let cloud = null;

/** @param {string} cmd @param {Record<string, unknown>} [args] */
function asCall(cmd, args) {
  if (!tauri) return Promise.resolve(null);
  return tauri.core.invoke(cmd, args).catch((/** @type {unknown} */ e) => {
    const el = $("as-status");
    el.textContent = String(e);
    el.classList.add("error");
    return null;
  });
}

async function loadAssistant() {
  void loadRoutines();
  void loadLearnt();
  const [list, v, sp, md, net, cl] = await Promise.all([asCall("assistant_activity"), asCall("voice_state"), asCall("speech_state"), asCall("model_state"), asCall("internet_state"), asCall("cloud_state")]);
  model = md;
  internet = net;
  cloud = cl;
  activity = list || [];
  voice = v;
  speech = sp;
  renderAssistant();
}

async function loadActivity() {
  activity = (await asCall("assistant_activity")) || [];
  renderActivity();
}

const AS_KINDS = /** @type {Record<string, string>} */ ({
  asked: "You asked",
  heard: "You said",
  understood: "Understood as",
  ran: "Done",
  failed: "Didn't work",
  refused: "Wouldn't",
  "asks-first": "Asked you first",
  confirmed: "You confirmed",
  cancelled: "Cancelled",
  undone: "Undone",
});

/** "14:02", "Yesterday 14:02" or "3 Oct 14:02". @param {number} secs */
function asWhen(secs) {
  const d = new Date(secs * 1000);
  const time = d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  const day = new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const now = new Date();
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  if (day === today) return time;
  if (day === today - 86400000) return `Yesterday ${time}`;
  return `${d.toLocaleDateString([], { day: "numeric", month: "short" })} ${time}`;
}

function renderAssistant() {
  $("as-voice").textContent = !voice ? "" : voice.ready ? `Ready, with the ${voice.model} speech model. What you say is turned into text on this PC and never kept.` : `Not set up yet. ${voice.missing}`;
  const speak = /** @type {HTMLSelectElement} */ ($("as-speak"));
  speak.value = speech?.speak || "voice";
  speak.disabled = !speech?.ready;
  $("as-speech").textContent = !speech
    ? ""
    : speech.ready
      ? `With the British English “Alan” voice (${speech.voice}), made on this PC.`
      : "Not set up yet. Run “kompact-assistant-setup voice” in a terminal (about 140 MB, once).";
  $("as-model").textContent = !model
    ? ""
    : model.ready
      ? `Ready (${model.model}): “I can't hear anything”, “get the browser out of the way”. A second or two; a guess of several steps asks first.`
      : `Not set up yet. ${model.missing}`;
  renderInternet();
  renderCloud();
  renderActivity();
}

/** 20d */
function renderInternet() {
  if (!internet) return;
  /** @type {HTMLSelectElement} */ ($("as-net")).value = internet.mode;
  const town = /** @type {HTMLInputElement} */ ($("as-town"));
  if (document.activeElement !== town) town.value = internet.place;
  town.placeholder = internet.zone_place || "e.g. Chennai";
  town.disabled = internet.mode === "off";
  $("as-town-hint").textContent = internet.place
    ? "For “what's the weather” and local news. You can also say “I live in Chennai”."
    : `Not set: ${internet.zone_place ? `${internet.zone_place}, from your time zone, is used` : "say the town when you ask"}. Type yours, or say “I live in Chennai”.`;
  if (!internet.curl) $("as-net-hint").textContent = "Needs curl, which isn't installed: run “sudo apt install curl” in a terminal.";
}

/** @param {string} mode @param {string} place */
async function setInternet(mode, place) {
  const st = await asCall("set_internet", { mode, place });
  if (st) internet = st;
  renderInternet();
  // 20d-2: the cloud AI is used only while Internet is Always.
  cloud = (await asCall("cloud_state")) || cloud;
  renderCloud();
}

/** 20d-2 */
function renderCloud() {
  if (!cloud) return;
  const on = /** @type {HTMLInputElement} */ ($("as-cloud-on"));
  on.checked = cloud.on;
  const pick = /** @type {HTMLSelectElement} */ ($("as-cloud-provider"));
  if (pick.options.length !== cloud.providers.length) {
    pick.replaceChildren(...cloud.providers.map((/** @type {{id: string, name: string}} */ p) => new Option(p.name, p.id)));
  }
  pick.value = cloud.provider;
  const chosen = cloud.providers.find((/** @type {{id: string}} */ p) => p.id === cloud.provider);
  const model = /** @type {HTMLInputElement} */ ($("as-cloud-model"));
  if (document.activeElement !== model) model.value = cloud.model;
  model.placeholder = chosen?.model || "";
  const key = /** @type {HTMLInputElement} */ ($("as-cloud-key"));
  key.value = "";
  key.placeholder = cloud.has_key ? `Saved${cloud.key_end ? `, ends in …${cloud.key_end}` : ""}` : "Paste your key";
  /** @type {HTMLButtonElement} */ ($("as-cloud-forget")).disabled = !cloud.has_key;
  /** @type {HTMLButtonElement} */ ($("as-cloud-test")).disabled = !cloud.has_key;
  $("as-cloud-key-hint").textContent = cloud.has_key ? "Saved in your home folder, readable by you alone. Paste a new one to replace it." : "From the service's website. Kept in your home folder, readable by you alone.";
  $("as-cloud-hint").textContent = !cloud.on
    ? "Optional. With your own key, a cloud AI understands and talks in place of the small model on this PC. It's given only your words, never your windows, files or screen, and Siva still asks before anything drastic. Only while Internet is Always."
    : cloud.not_used
      ? `Not in use: ${cloud.not_used}. The model on this PC answers meanwhile.`
      : `In use (${chosen?.name || cloud.provider}). If it fails, the model on this PC answers instead.`;
}

/** @param {string | null} key a new key, "" to forget it, null to keep it */
async function setCloud(key) {
  const st = await asCall("set_cloud", {
    on: /** @type {HTMLInputElement} */ ($("as-cloud-on")).checked,
    provider: /** @type {HTMLSelectElement} */ ($("as-cloud-provider")).value,
    model: /** @type {HTMLInputElement} */ ($("as-cloud-model")).value,
    key,
  });
  if (st) cloud = st;
  $("as-cloud-result").textContent = "";
  renderCloud();
}

async function testCloud() {
  if (!tauri) return;
  const out = $("as-cloud-result");
  out.textContent = "Asking…";
  out.classList.remove("error");
  try {
    out.textContent = await tauri.core.invoke("cloud_test");
  } catch (e) {
    out.textContent = String(e);
    out.classList.add("error");
  }
}

function renderActivity() {
  const list = $("as-list");
  list.replaceChildren(
    ...activity.map((e) => {
      const row = document.createElement("div");
      row.className = `row as-entry as-${e.kind}`;
      const what = e.kind === "asked" || e.kind === "heard" ? `“${e.text}”` : e.text || (e.tool ? e.tool.replace(/_/g, " ") : "");
      const hint = [AS_KINDS[e.kind] || e.kind, e.kind !== "asked" && e.tool ? e.tool.replace(/_/g, " ") : ""].filter(Boolean).join(" · ");
      row.append(netLabel(what, hint));
      row.append(Object.assign(document.createElement("span"), { className: "as-when", textContent: asWhen(e.at) }));
      return row;
    }),
  );
  if (!activity.length) list.append(Object.assign(document.createElement("div"), { className: "row muted", textContent: "Nothing yet. What you ask Siva shows up here." }));
  /** @type {HTMLButtonElement} */ ($("as-clear")).disabled = !activity.length;
}

// ---- 19j: learn my voice ----

/** @typedef {{phrases: [string, string][], word_fixes: [string, string][], words: string[], training: string[]}} Learnt */
/** @type {Learnt | null} */
let learnt = null;
/** The training session: which phrase, how many learnt, and whether it's listening. */
const lv = { i: 0, learnt: 0, busy: false, on: false };

async function loadLearnt() {
  learnt = await asCall("voice_learnt");
  renderLearnt();
}

function renderLearnt() {
  const list = $("lv-list");
  const l = learnt;
  /** @type {HTMLTextAreaElement} */ ($("lv-words")).value = (l?.words || []).join("\n");
  const rows = (l?.phrases || []).slice().reverse().map(([heard, meant]) => {
    const row = Object.assign(document.createElement("div"), { className: "row" });
    row.append(netLabel(`“${heard}” → ${meant}`, "heard → meant"));
    const b = Object.assign(document.createElement("button"), { className: "btn", type: "button", textContent: "Forget" });
    b.addEventListener("click", async () => {
      const st = await asCall("voice_learnt_change", { forget: heard });
      if (st) learnt = st;
      renderLearnt();
    });
    row.append(b);
    return row;
  });
  list.replaceChildren(...rows);
  if (l?.word_fixes.length) {
    list.append(Object.assign(document.createElement("div"), { className: "row muted", textContent: `Words it puts right everywhere: ${l.word_fixes.map(([a, b]) => `${a} → ${b}`).join(", ")}` }));
  }
  if (!rows.length) list.append(Object.assign(document.createElement("div"), { className: "row muted", textContent: "Nothing learnt yet. Train it, or fix what it mishears with ✎." }));
  /** @type {HTMLButtonElement} */ ($("lv-clear")).disabled = !rows.length;
}

function lvShow() {
  const phrases = learnt?.training || [];
  if (lv.i >= phrases.length) {
    lv.on = false;
    $("lv-train").hidden = true;
    $("lv-start").textContent = "Train Again";
    $("lv-status").textContent = lv.learnt ? `Training done: learnt ${lv.learnt} new thing${lv.learnt === 1 ? "" : "s"} about your voice.` : "Training done: it heard you right every time.";
    void loadLearnt();
    return;
  }
  $("lv-count").textContent = `${lv.i + 1} of ${phrases.length}.`;
  $("lv-phrase").textContent = phrases[lv.i];
  $("lv-heard").textContent = "Press Listen, wait for the chime, then say it as you would to Siva.";
  $("lv-heard").classList.remove("learnt");
}

$("lv-start").addEventListener("click", async () => {
  if (!learnt) await loadLearnt();
  Object.assign(lv, { i: 0, learnt: 0, busy: false, on: true });
  $("lv-train").hidden = false;
  $("lv-status").textContent = "";
  lvShow();
});
$("lv-listen").addEventListener("click", async () => {
  if (lv.busy || !lv.on) return;
  lv.busy = true;
  const meant = learnt?.training[lv.i] || "";
  const heardEl = $("lv-heard");
  heardEl.classList.remove("learnt");
  heardEl.textContent = "Listening… wait for the chime, then speak.";
  /** @type {{text: string | null, note: string | null} | null} */
  const r = await asCall("voice_hear");
  lv.busy = false;
  if (!lv.on) return;
  if (!r?.text) {
    heardEl.textContent = r?.note || "I didn't hear anything. Try again.";
    return;
  }
  const isNew = await asCall("voice_learn", { heard: r.text, meant });
  if (isNew) {
    lv.learnt++;
    heardEl.classList.add("learnt");
    heardEl.textContent = `Heard “${r.text}”: learnt.`;
  } else {
    heardEl.textContent = `Heard “${r.text}”: right.`;
  }
  setTimeout(() => {
    if (!lv.on || lv.busy) return;
    lv.i++;
    lvShow();
  }, 1400);
});
$("lv-skip").addEventListener("click", () => {
  if (lv.busy) return;
  lv.i++;
  lvShow();
});
$("lv-stop").addEventListener("click", () => {
  lv.on = false;
  if (lv.busy) void asCall("voice_cancel");
  $("lv-train").hidden = true;
  void loadLearnt();
});
$("lv-words-save").addEventListener("click", async () => {
  const words = /** @type {HTMLTextAreaElement} */ ($("lv-words")).value.split("\n").map((w) => w.trim()).filter(Boolean);
  const st = await asCall("voice_learnt_change", { words });
  if (st) {
    learnt = st;
    $("lv-status").textContent = `Saved ${st.words.length} word${st.words.length === 1 ? "" : "s"}.`;
  }
  renderLearnt();
});
$("lv-clear").addEventListener("click", () =>
  ask("Forget what Siva learnt about your voice?", "The corrections and your words are deleted from this PC. You can train it again any time.", [
    ["Cancel", () => {}],
    [
      "Forget All",
      async () => {
        const st = await asCall("voice_learnt_change", { clear: true });
        if (st) learnt = st;
        renderLearnt();
      },
      true,
    ],
  ]),
);

// ---- 19e: routines ----

/** @typedef {{name: string, phrases: string[]}} RoutineDef */

async function loadRoutines() {
  /** @type {RoutineDef[]} */
  const list = (await asCall("routines_state")) || [];
  renderRoutines(list, []);
  $("as-rt-status").textContent = "";
}

/** @param {RoutineDef[]} list @param {{index: number, text: string}[]} problems */
function renderRoutines(list, problems) {
  $("as-routines").replaceChildren(...list.map((r, i) => routineCard(r, problems.find((p) => p.index === i)?.text || "")));
}

/** @param {RoutineDef} r @param {string} problem */
function routineCard(r, problem) {
  const card = Object.assign(document.createElement("section"), { className: "card as-rt" });
  const row = Object.assign(document.createElement("div"), { className: "row" });
  const name = Object.assign(document.createElement("input"), { className: "text-field as-rt-name", type: "text", value: r.name, maxLength: 40, placeholder: "Name, e.g. coding", spellcheck: false });
  name.setAttribute("aria-label", "Routine name");
  const del = Object.assign(document.createElement("button"), { className: "btn", type: "button", textContent: "Delete" });
  del.addEventListener("click", () => {
    card.remove();
    $("as-rt-status").textContent = "Not saved yet.";
  });
  row.append(name, del);
  const steps = Object.assign(document.createElement("textarea"), { value: r.phrases.join("\n"), placeholder: "One request a line, e.g.\nopen terminal\nopen firefox\nput terminal and firefox side by side", spellcheck: false });
  steps.setAttribute("aria-label", `Requests of ${r.name || "the routine"}, one a line`);
  const note = Object.assign(document.createElement("div"), { className: "note", textContent: problem });
  card.append(row, steps, note);
  return card;
}

/** What the cards say now. @returns {RoutineDef[]} */
function routinesOnScreen() {
  return [...document.querySelectorAll("#as-routines .as-rt")].map((card) => ({
    name: /** @type {HTMLInputElement} */ (card.querySelector(".as-rt-name")).value,
    phrases: /** @type {HTMLTextAreaElement} */ (card.querySelector("textarea")).value.split("\n").map((l) => l.trim()).filter(Boolean),
  }));
}

$("as-rt-add").addEventListener("click", () => {
  const card = routineCard({ name: "", phrases: [] }, "");
  $("as-routines").append(card);
  /** @type {HTMLInputElement} */ (card.querySelector(".as-rt-name")).focus();
});
$("as-rt-save").addEventListener("click", async () => {
  if (!tauri) return;
  const list = routinesOnScreen();
  const status = $("as-rt-status");
  try {
    /** @type {RoutineDef[]} */
    const saved = await tauri.core.invoke("routines_save", { routines: list });
    renderRoutines(saved, []);
    status.classList.remove("error");
    status.textContent = `Saved ${saved.length} routine${saved.length === 1 ? "" : "s"}.`;
  } catch (e) {
    // Which routine is wrong, and how (nothing was saved).
    const problems = Array.isArray(e) ? e : [{ index: 0, text: String(e) }];
    renderRoutines(list, problems);
    status.classList.add("error");
    status.textContent = "Not saved: see the routines marked in red.";
  }
});

$("as-net").addEventListener("change", (e) => void setInternet(/** @type {HTMLSelectElement} */ (e.target).value, internet?.place || ""));
$("as-town").addEventListener("change", (e) => void setInternet(internet?.mode || "always", /** @type {HTMLInputElement} */ (e.target).value));
$("as-cloud-on").addEventListener("change", () => void setCloud(null));
$("as-cloud-provider").addEventListener("change", () => {
  // Another service: its own model, not the last one's.
  /** @type {HTMLInputElement} */ ($("as-cloud-model")).value = "";
  void setCloud(null);
});
$("as-cloud-model").addEventListener("change", () => void setCloud(null));
$("as-cloud-key").addEventListener("change", (e) => {
  const k = /** @type {HTMLInputElement} */ (e.target).value.trim();
  if (k) void setCloud(k);
});
$("as-cloud-forget").addEventListener("click", () => void setCloud(""));
$("as-cloud-test").addEventListener("click", () => void testCloud());
$("as-speak").addEventListener("change", async (e) => {
  const st = await asCall("set_speak", { speak: /** @type {HTMLSelectElement} */ (e.target).value });
  if (st) speech = st;
  renderAssistant();
});
$("as-clear").addEventListener("click", () =>
  ask("Clear Siva's activity?", "The list of what you asked and what it did is deleted from this PC.", [
    ["Cancel", () => {}],
    ["Clear", () => void asCall("assistant_activity_clear").then(loadActivity), true],
  ]),
);

// ---- 12c: Sound (PipeWire; the menu bar's speaker and the volume keys change it too) ----

/** @typedef {{id: number, name: string, label: string, nick: string | null, output: boolean}} SdNode */
/** @typedef {{devices: SdNode[], current: number | null, volume: number, muted: boolean}} SdSide */
/** @typedef {{output: SdSide, input: SdSide, waves: number}} SdState */
/** @type {SdState | null} */
let sd = null;
let sdDragging = "";
let sdTimer = 0;

/** @param {string} cmd @param {Record<string, unknown>} [args] */
function sdCall(cmd, args) {
  if (!tauri) return Promise.resolve(null);
  return tauri.core.invoke(cmd, args).catch((/** @type {unknown} */ e) => {
    const el = $("sd-status");
    el.textContent = String(e);
    el.classList.add("error");
    return null;
  });
}

async function loadSound() {
  const st = await sdCall("sound_state");
  if (st) sd = st;
  renderSound();
}

function renderSound() {
  const st = sd;
  for (const [side, key] of /** @type {["out" | "in", "output" | "input"][]} */ ([["out", "output"], ["in", "input"]])) {
    const s = st?.[key];
    const vol = /** @type {HTMLInputElement} */ ($(`sd-${side}-vol`));
    const mute = /** @type {HTMLInputElement} */ ($(`sd-${side}-mute`));
    const none = !s || s.current === null;
    if (sdDragging !== side) vol.value = String(Math.round((s?.volume || 0) * 100));
    $(`sd-${side}-val`).textContent = none ? "" : `${vol.value}%`;
    vol.disabled = mute.disabled = none;
    mute.checked = !!s?.muted;
    const list = $(side === "out" ? "sd-outputs" : "sd-inputs");
    list.replaceChildren(
      ...(s?.devices || []).map((d) => {
        const row = document.createElement("div");
        row.className = "row pickable";
        row.append(Object.assign(document.createElement("span"), { className: "sd-pick", textContent: d.id === s?.current ? "✓" : "" }));
        row.append(netLabel(d.nick || d.label, d.nick ? d.label : undefined));
        row.addEventListener("click", async () => {
          if (d.id === s?.current) return;
          console.log(`settings: sound ${key} → ${d.nick || d.label}`);
          await sdCall("sound_default", { id: d.id });
          await loadSound();
        });
        return row;
      }),
    );
    if (!s?.devices.length) list.append(Object.assign(document.createElement("div"), { className: "row muted", textContent: key === "output" ? "No outputs." : "No microphones." }));
  }
  $("sd-in-hint").textContent = st && st.input.current === null && st.input.devices.length ? "Choose a microphone below." : "";
}

for (const side of ["out", "in"]) {
  const output = side === "out";
  const vol = /** @type {HTMLInputElement} */ ($(`sd-${side}-vol`));
  vol.addEventListener("input", () => {
    sdDragging = side;
    $(`sd-${side}-val`).textContent = `${vol.value}%`;
    clearTimeout(sdTimer);
    sdTimer = setTimeout(() => sdCall("sound_volume", { output, volume: Number(vol.value) / 100 }), 60);
  });
  vol.addEventListener("change", () => {
    sdDragging = "";
    clearTimeout(sdTimer);
    void sdCall("sound_volume", { output, volume: Number(vol.value) / 100 });
  });
  $(`sd-${side}-mute`).addEventListener("change", (e) => sdCall("sound_mute", { output, mute: /** @type {HTMLInputElement} */ (e.target).checked }));
}

// ---- 12b: Bluetooth (BlueZ; the menu bar's Bluetooth menu connects paired devices) ----

/** @typedef {{path: string, address: string, name: string, kind: string, paired: boolean, connected: boolean, battery: number | null, rssi: number | null}} BtDevice */
/** @typedef {{adapter: string | null, name?: string, powered?: boolean, blocked?: boolean, discovering?: boolean, discoverable?: boolean, devices?: BtDevice[]}} BtState */
/** @typedef {{id: number, kind: "confirm" | "pin" | "passkey" | "show" | "authorize", device: string, code: string | null}} BtQuestion */
/** @type {BtState | null} */
let bt = null;
const btBusy = new Map(); // path → what's under way
/** fix/bluetooth: devices that forgot this computer ("key missing"): they get Pair Again. */
const btForgotUs = new Set();
/** The pairing question the dialog shows (0: none). */
let dialogFor = 0;
const BT_GLYPH = /** @type {Record<string, string>} */ ({ headphones: "🎧", speaker: "🔊", keyboard: "⌨️", mouse: "🖱️", gamepad: "🎮", phone: "📱", computer: "💻" });

/** @param {string} cmd @param {Record<string, unknown>} [args] */
function btCall(cmd, args) {
  if (!tauri) return Promise.resolve(null);
  return tauri.core.invoke(cmd, args).catch((/** @type {unknown} */ e) => {
    const text = String(e);
    // fix/bluetooth: "key-missing: …": that device gets a Pair Again button.
    const path = /** @type {any} */ (args)?.path;
    if (text.startsWith("key-missing: ") && path) btForgotUs.add(path);
    const el = $("bt-status");
    el.textContent = text.replace(/^key-missing: /, "");
    el.classList.add("error");
    return null;
  });
}

async function loadBluetooth() {
  const st = await btCall("bluetooth_state");
  if (st) bt = st;
  renderBluetooth();
}

function renderBluetooth() {
  const st = bt;
  const power = /** @type {HTMLInputElement} */ ($("bt-power"));
  power.checked = !!st?.powered;
  power.disabled = !st?.adapter;
  $("bt-hint").textContent = !st
    ? "Bluetooth (BlueZ) isn't answering."
    : !st.adapter
      ? "No Bluetooth adapter."
      : st.blocked
        ? "Switched off on this device (rfkill)."
        : st.powered
          ? st.discoverable
            ? `This computer shows up as “${st.name}” while this pane is open.`
            : `On, as “${st.name}”.`
          : "Off.";
  const on = !!st?.powered;
  for (const id of ["bt-mine-title", "bt-mine", "bt-near-title", "bt-near"]) $(id).hidden = !on;
  const devices = st?.devices || [];
  const mine = devices.filter((d) => d.paired);
  const near = devices.filter((d) => !d.paired);
  $("bt-mine").replaceChildren(...(mine.length ? mine.map(btRow) : [btEmpty("None yet: pair one below.")]));
  $("bt-near").replaceChildren(...(near.length ? near.map(btRow) : [btEmpty(st?.discovering ? "Looking for devices… (put the device in pairing mode)" : "Not looking right now.")]));
}

/** @param {string} text */
function btEmpty(text) {
  return Object.assign(document.createElement("div"), { className: "row muted", textContent: text });
}

/** @param {string} text @param {() => void} run @param {boolean} [primary] */
function btButton(text, run, primary = false) {
  const b = Object.assign(document.createElement("button"), { className: primary ? "btn primary" : "btn", textContent: text });
  b.addEventListener("click", run);
  return b;
}

/** @param {BtDevice} d */
function btRow(d) {
  const row = document.createElement("div");
  row.className = "row";
  row.append(Object.assign(document.createElement("span"), { className: "bt-glyph", textContent: BT_GLYPH[d.kind] || "◉" }));
  const busy = btBusy.get(d.path);
  const hint = busy || (d.paired ? (d.connected ? `Connected${d.battery !== null ? ` · battery ${d.battery}%` : ""}` : "Not connected") : d.address);
  row.append(netLabel(d.name, hint));
  if (!busy) {
    if (d.paired && btForgotUs.has(d.path) && !d.connected) {
      // fix/bluetooth: it forgot this computer: forget it here too, find it, pair again.
      row.append(btButton("Pair Again", () => btDo(d, "Put it in pairing mode now…", "bluetooth_repair"), true));
    }
    if (d.paired) {
      row.append(btButton(d.connected ? "Disconnect" : "Connect", () => btDo(d, d.connected ? "Disconnecting…" : "Connecting…", d.connected ? "bluetooth_disconnect" : "bluetooth_connect")));
      row.append(
        btButton("Forget", () =>
          ask(`Forget “${d.name}”?`, "It will need to be paired again to connect.", [
            ["Cancel", () => {}],
            ["Forget", () => btDo(d, "Removing…", "bluetooth_forget"), true],
          ]),
        ),
      );
    } else {
      row.append(btButton("Pair", () => btDo(d, "Pairing…", "bluetooth_pair"), true));
    }
  }
  return row;
}

/** @param {BtDevice} d @param {string} what @param {string} cmd */
async function btDo(d, what, cmd) {
  btBusy.set(d.path, what);
  const el = $("bt-status");
  el.textContent = "";
  el.classList.remove("error");
  renderBluetooth();
  console.log(`settings: ${cmd} ${d.name}`);
  if (cmd !== "bluetooth_connect") btForgotUs.delete(d.path);
  await btCall(cmd, { path: d.path });
  // btCall marks the status line when it failed (and keeps Pair Again for "key missing").
  const failed = $("bt-status").classList.contains("error");
  if (!failed && cmd === "bluetooth_repair") btForgotUs.delete(d.path);
  btBusy.delete(d.path);
  await loadBluetooth();
}

/** A question from BlueZ's pairing agent: in the dialog. @param {BtQuestion} q */
function btAsk(q) {
  if (dialogFor === q.id) return;
  if ($("pane-bluetooth").hidden) showPane("bluetooth");
  const answer = (/** @type {boolean} */ yes, /** @type {string} */ value = "") => {
    dialogFor = 0;
    void btCall("bluetooth_answer", { id: q.id, yes, value });
  };
  const titles = {
    confirm: [`Pair with “${q.device}”?`, "Check that the device shows this code, then pair.", "Pair"],
    pin: [`PIN for “${q.device}”`, "Type the PIN the device shows or came with (often 0000).", "Pair"],
    passkey: [`Passkey for “${q.device}”`, "Type the six-digit passkey the device shows.", "Pair"],
    show: [`Pair with “${q.device}”`, "Type this code on the device, then press Enter on it.", ""],
    authorize: [`“${q.device}” wants to pair`, "Allow it only if you are pairing it now.", "Allow"],
  };
  const [title, text, yes] = titles[q.kind];
  ask(title, text, [
    [q.kind === "authorize" ? "Don't Allow" : "Cancel", () => answer(false)],
    ...(yes ? /** @type {[string, () => void, boolean][]} */ ([[yes, () => answer(true, /** @type {HTMLInputElement} */ ($("dlg-input")).value), true]]) : []),
  ]);
  dialogFor = q.id;
  const code = $("dlg-code");
  code.hidden = !q.code;
  code.textContent = q.code || "";
  const input = /** @type {HTMLInputElement} */ ($("dlg-input"));
  input.hidden = q.kind !== "pin" && q.kind !== "passkey";
  input.value = "";
  input.inputMode = q.kind === "passkey" ? "numeric" : "text";
  input.maxLength = q.kind === "passkey" ? 6 : 16;
  input.placeholder = q.kind === "passkey" ? "123456" : "0000";
  input.onkeydown = (e) => {
    if (e.key === "Enter" && input.value) {
      $("dialog").hidden = true;
      answer(true, input.value);
    }
  };
  if (!input.hidden) input.focus();
  console.log(`settings: bluetooth asks ${q.kind} for ${q.device}`);
}

$("bt-power").addEventListener("change", async (e) => {
  const on = /** @type {HTMLInputElement} */ (e.target).checked;
  await btCall("bluetooth_power", { on });
  if (on) void btCall("bluetooth_discover", { on: true });
  await loadBluetooth();
});

// ---- 12a: Network (NetworkManager; the same as the menu bar's network menu) ----

/** @typedef {{ssid: string, strength: number, bars: number, security: string, active: boolean, known: boolean}} Net */
/** @typedef {{iface: string, kind: string, state: string, ip4: string | null, connection: string | null}} NetDevice */
/** @typedef {{wifi_enabled: boolean, wifi_hardware: boolean, devices: NetDevice[], networks: Net[], known: {uuid: string, name: string, ssid: string | null}[]}} NetState */
/** @type {NetState | null} */
let net = null;
let netAsking = "";
let netJoining = "";
/** @type {{ssid: string, ok: boolean, message: string} | null} */
let netResult = null;

/** @param {string} cmd @param {Record<string, unknown>} [args] */
function netCall(cmd, args) {
  if (!tauri) return Promise.resolve(null);
  return tauri.core.invoke(cmd, args).catch((/** @type {unknown} */ e) => {
    const el = $("net-status");
    el.textContent = String(e);
    el.classList.add("error");
    return null;
  });
}

async function loadNetwork(scan = false) {
  const st = await netCall("network_state");
  if (st) net = st;
  renderNetwork();
  if (scan) void netCall("network_scan");
}

/** @param {number} bars */
function netBarsIcon(bars) {
  const ns = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(ns, "svg");
  svg.setAttribute("viewBox", "0 0 16 16");
  svg.setAttribute("class", "net-bars");
  ["M8 13.2l1.6-1.9a2.4 2.4 0 0 0-3.2 0z", "M4.6 9.2a5.2 5.2 0 0 1 6.8 0l-1 1.2a3.6 3.6 0 0 0-4.8 0z", "M2.4 6.6a8.6 8.6 0 0 1 11.2 0l-1 1.2a7 7 0 0 0-9.2 0z"].forEach((d, i) => {
    const p = document.createElementNS(ns, "path");
    p.setAttribute("d", d);
    if (i > bars) p.setAttribute("class", "dim");
    svg.append(p);
  });
  return svg;
}

/** @param {string} label @param {string} [hint] */
function netLabel(label, hint) {
  const l = document.createElement("span");
  l.className = "row-label";
  l.append(label);
  if (hint) l.append(Object.assign(document.createElement("span"), { className: "hint", textContent: hint }));
  return l;
}

function renderNetwork() {
  const st = net;
  const devices = $("net-devices");
  devices.replaceChildren();
  if (!st) {
    devices.append(Object.assign(document.createElement("p"), { className: "note-text", textContent: "NetworkManager isn't answering." }));
  }
  for (const d of st?.devices || []) {
    const card = document.createElement("section");
    card.className = "card";
    const row = document.createElement("div");
    row.className = "row";
    const state = d.state === "connected" ? "Connected" : d.state === "unavailable" ? (d.kind === "ethernet" ? "Cable unplugged" : "Unavailable") : d.state[0].toUpperCase() + d.state.slice(1);
    const detail = [d.kind === "wifi" && d.connection ? d.connection : "", d.ip4 ? `IP ${d.ip4}` : "", d.iface].filter(Boolean).join(" · ");
    row.append(netLabel(d.kind === "wifi" ? "Wi-Fi" : "Ethernet", `${state}${detail ? ` — ${detail}` : ""}`));
    if (d.kind === "wifi") {
      if (d.state === "connected") {
        const b = Object.assign(document.createElement("button"), { className: "btn", textContent: "Disconnect" });
        b.addEventListener("click", () => netCall("network_disconnect", { iface: d.iface }).then(() => loadNetwork()));
        row.append(b);
      }
      const sw = Object.assign(document.createElement("input"), { type: "checkbox", className: "switch", checked: !!st?.wifi_enabled });
      sw.setAttribute("role", "switch");
      sw.setAttribute("aria-label", "Wi-Fi");
      sw.addEventListener("change", () => netCall("network_wifi", { on: sw.checked }).then(() => loadNetwork(true)));
      row.append(sw);
    }
    card.append(row);
    devices.append(card);
  }
  // Networks in range.
  const wifiOn = !!st?.wifi_enabled && !!st?.devices.some((d) => d.kind === "wifi");
  $("net-other-title").hidden = $("net-networks").hidden = !wifiOn;
  const list = $("net-networks");
  list.replaceChildren(...(st?.networks || []).map(netRowEl));
  if (wifiOn && !st?.networks.length) list.append(Object.assign(document.createElement("div"), { className: "row muted", textContent: "Looking for networks…" }));
  // Known networks.
  const known = $("net-known");
  known.replaceChildren(
    ...(st?.known || []).map((k) => {
      const row = document.createElement("div");
      row.className = "row";
      row.append(netLabel(k.ssid || k.name, k.ssid && k.name !== k.ssid ? k.name : undefined));
      const forget = Object.assign(document.createElement("button"), { className: "btn", textContent: "Forget" });
      forget.addEventListener("click", () =>
        ask(`Forget "${k.ssid || k.name}"?`, "kompact won't join it again on its own, and its password is removed.", [
          ["Cancel", () => {}],
          ["Forget", () => netCall("network_forget", { uuid: k.uuid }).then(() => loadNetwork()), true],
        ]),
      );
      row.append(forget);
      return row;
    }),
  );
  if (!st?.known.length) known.append(Object.assign(document.createElement("div"), { className: "row muted", textContent: "None yet: networks you join are remembered here." }));
  /** @type {HTMLInputElement | null} */ (list.querySelector("input"))?.focus();
}

/** @param {Net} n */
function netRowEl(n) {
  const wrap = document.createElement("div");
  const row = document.createElement("div");
  row.className = "row";
  row.append(Object.assign(document.createElement("span"), { className: "net-check", textContent: n.active ? "✓" : "" }));
  row.append(netLabel(n.ssid, n.active ? "Connected" : n.known ? "Known" : n.security === "unsupported" ? "Needs a sign-in kompact can't do yet" : undefined));
  if (n.security !== "open") row.append(Object.assign(document.createElement("span"), { className: "net-lock", textContent: "🔒" }));
  row.append(netBarsIcon(n.bars));
  if (!n.active && n.security !== "unsupported") {
    const join = Object.assign(document.createElement("button"), { className: "btn", textContent: netJoining === n.ssid ? "Joining…" : "Join" });
    join.disabled = !!netJoining;
    join.addEventListener("click", () => {
      netResult = null;
      if (n.known || n.security === "open") return netJoinNow(n.ssid, null);
      netAsking = netAsking === n.ssid ? "" : n.ssid;
      renderNetwork();
    });
    row.append(join);
  }
  wrap.append(row);
  if (netResult?.ssid === n.ssid && !netResult.ok) wrap.append(Object.assign(document.createElement("div"), { className: "net-note error", textContent: netResult.message }));
  if (netAsking === n.ssid) {
    const form = document.createElement("form");
    form.className = "net-pass";
    const input = Object.assign(document.createElement("input"), { type: "password", placeholder: "Password", autocomplete: "off" });
    input.setAttribute("aria-label", `Password for ${n.ssid}`);
    const go = Object.assign(document.createElement("button"), { type: "submit", className: "btn primary", textContent: "Join" });
    form.append(input, go);
    form.addEventListener("submit", (e) => {
      e.preventDefault();
      if (input.value) netJoinNow(n.ssid, input.value);
    });
    wrap.append(form);
  }
  return wrap;
}

/** @param {string} ssid @param {string | null} password */
function netJoinNow(ssid, password) {
  netAsking = "";
  netJoining = ssid;
  console.log(`settings: joining ${ssid}`);
  renderNetwork();
  netCall("network_join", { ssid, password }).then((r) => {
    if (r === null && netJoining === ssid) {
      netJoining = "";
      renderNetwork();
    }
  });
}

// ---- 10d-5: Screen Sharing (the shell keeps xdg-desktop-portal-wlr's file).

/** @typedef {{installed: boolean, slurp: boolean, configured: boolean, fps: number | null,
 *   choices: number[], sharing: string[], restart_pending: boolean}} SharingState */
/** @type {SharingState | null} */
let share = null;

function renderSharing() {
  const st = share;
  let state = "…";
  let hint = "";
  if (st && !st.installed) {
    state = "Not available";
    hint = "Install the portal first: sudo apt install xdg-desktop-portal-wlr slurp";
  } else if (st && !st.configured) {
    state = "Not set up";
    hint = "Set Up lets apps share a screen in kompact sessions (your GNOME session is untouched).";
  } else if (st) {
    state = st.sharing.length ? `Sharing ${st.sharing.join(", ")} now` : "Ready";
    hint = st.restart_pending
      ? "The new frame rate applies once this share ends."
      : st.slurp
        ? "Apps ask to share; you click the screen to share."
        : "slurp isn't installed (it picks the screen): sudo apt install slurp";
  }
  $("share-state").textContent = state;
  $("share-hint").textContent = hint;
  $("share-setup").hidden = !st || !st.installed || st.configured;
  $("share-options").classList.toggle("off", !st?.configured);
  $("share-fps").replaceChildren(
    ...(st?.choices || [5, 10, 15, 20, 30]).map((n) => {
      const b = document.createElement("button");
      b.textContent = `${n} fps`;
      b.setAttribute("role", "radio");
      b.setAttribute("aria-checked", String(st?.fps === n));
      b.disabled = !st?.configured;
      b.addEventListener("click", () => st?.fps !== n && sharingCall("set_sharing_fps", { fps: n }));
      return b;
    }),
  );
}

/** @param {string} cmd @param {Record<string, unknown>} [args] */
function sharingCall(cmd, args) {
  if (!tauri) return;
  console.log(`settings: ${cmd} ${JSON.stringify(args || {})}`);
  tauri.core
    .invoke(cmd, args)
    .then((/** @type {SharingState} */ st) => {
      share = st;
      $("share-status").textContent = "";
      renderSharing();
    })
    .catch((/** @type {unknown} */ e) => {
      const el = $("share-status");
      el.textContent = String(e);
      el.classList.add("error");
    });
}

function loadSharing() {
  tauri?.core.invoke("sharing_settings").then((/** @type {SharingState} */ st) => {
    share = st;
    renderSharing();
  });
}
$("share-setup").addEventListener("click", () => sharingCall("set_up_sharing"));
renderSharing();

// ---- 12g: Date & Time (timedated; the menu-bar clock's look) ----

/** @typedef {{hour24: boolean, weekday: boolean, date: boolean, seconds: boolean}} ClockPrefs */
/** @type {{zone: string, ntp: boolean, can_ntp: boolean, synced: boolean, clock: ClockPrefs} | null} */
let dt = null;
/** @type {string[] | null} */
let zones = null;

/** @param {string} msg @param {boolean} [error] */
function dtStatus(msg, error = false) {
  $("dt-status").textContent = msg;
  $("dt-status").classList.toggle("error", error);
}

/** @param {string} cmd @param {Record<string, unknown>} [args] */
async function dtCall(cmd, args) {
  if (!tauri) return null;
  try {
    return await tauri.core.invoke(cmd, args);
  } catch (e) {
    dtStatus(String(e), true);
    return null;
  }
}

async function loadDatetime() {
  dt = (await dtCall("datetime_state")) || dt;
  if (!zones) zones = (await dtCall("datetime_zones")) || [];
  renderDatetime();
}

/** The menu bar's clock text for these settings (as panel.js makes it). @param {ClockPrefs} p @param {Date} now */
function clockPreview(p, now) {
  const zone = dt?.zone ? { timeZone: dt.zone } : {};
  /** @type {Intl.DateTimeFormatOptions} */
  const d = { ...zone };
  if (p.weekday) d.weekday = "short";
  if (p.date) {
    d.day = "numeric";
    d.month = "short";
  }
  const date = p.weekday || p.date ? now.toLocaleDateString(undefined, d).replace(/,/g, "") : "";
  /** @type {Intl.DateTimeFormatOptions} */
  const t = { ...zone, hour: p.hour24 ? "2-digit" : "numeric", minute: "2-digit", hour12: !p.hour24 };
  if (p.seconds) t.second = "2-digit";
  const time = now.toLocaleTimeString(undefined, t);
  return date ? `${date} ${time}` : time;
}

function renderDatetime() {
  const s = dt;
  const now = new Date();
  $("dt-now").textContent = s ? now.toLocaleString(undefined, { dateStyle: "full", timeStyle: "medium", timeZone: s.zone || undefined }) : "timedated isn't answering.";
  const ntp = /** @type {HTMLInputElement} */ ($("dt-ntp"));
  ntp.checked = !!s?.ntp;
  ntp.disabled = !s?.can_ntp;
  $("dt-ntp-hint").textContent = !s?.can_ntp ? "Not available here." : s.ntp ? (s.synced ? "From the internet; in sync." : "From the internet; not in sync yet.") : "Off.";
  $("dt-zone").textContent = s?.zone || "?";
  for (const [id, key] of /** @type {[string, keyof ClockPrefs][]} */ ([["dt-24", "hour24"], ["dt-weekday", "weekday"], ["dt-date", "date"], ["dt-seconds", "seconds"]])) {
    const el = /** @type {HTMLInputElement} */ ($(id));
    el.checked = !!s?.clock[key];
    el.disabled = !s;
  }
  if (s) $("dt-preview").textContent = clockPreview(s.clock, now);
}

function renderZoneSearch() {
  const q = /** @type {HTMLInputElement} */ ($("dt-search")).value.trim().toLowerCase().replace(/ /g, "_");
  const box = $("dt-results");
  if (!q || !zones) return box.replaceChildren();
  const hits = zones.filter((z) => z.toLowerCase().includes(q)).slice(0, 30);
  box.replaceChildren(
    ...hits.map((z) => {
      const row = document.createElement("div");
      row.className = "row";
      const time = new Date().toLocaleTimeString(undefined, { timeZone: z, hour: "2-digit", minute: "2-digit" });
      row.append(netLabel(z.replace(/_/g, " "), `${time} there`));
      row.addEventListener("click", async () => {
        /** @type {HTMLInputElement} */ ($("dt-search")).value = "";
        box.replaceChildren();
        dtStatus(`Setting ${z}…`);
        console.log(`settings: time zone ${z}`);
        if ((await dtCall("datetime_set_zone", { zone: z })) !== null) dtStatus(`Time zone now ${z}.`);
        await loadDatetime();
      });
      return row;
    }),
  );
  if (!hits.length) box.append(Object.assign(document.createElement("div"), { className: "row muted", textContent: "No time zone by that name." }));
}

$("dt-search").addEventListener("input", renderZoneSearch);
$("dt-ntp").addEventListener("change", async (e) => {
  await dtCall("datetime_set_ntp", { on: /** @type {HTMLInputElement} */ (e.target).checked });
  await loadDatetime();
});
for (const id of ["dt-24", "dt-weekday", "dt-date", "dt-seconds"]) {
  $(id).addEventListener("change", async () => {
    const v = (/** @type {string} */ i) => /** @type {HTMLInputElement} */ ($(i)).checked;
    await dtCall("set_clock_prefs", { hour24: v("dt-24"), weekday: v("dt-weekday"), date: v("dt-date"), seconds: v("dt-seconds") });
    await loadDatetime();
  });
}

// ---- 14b: Users & Login (users.rs; kompact-lock reads the look at each lock) ----

/** @typedef {{login: string, name: string, picture: boolean, background: string, image: string | null, message: string}} UsersState */
/** @type {UsersState | null} */
let us = null;
let usStamp = 0;

/** @param {string} text @param {boolean} [error] */
function usStatus(text, error = false) {
  $("us-status").textContent = text;
  $("us-status").classList.toggle("error", error);
}

/** @param {string} cmd @param {Record<string, unknown>} [args] */
async function usCall(cmd, args) {
  if (!tauri) return null;
  try {
    return (await tauri.core.invoke(cmd, args)) ?? true;
  } catch (e) {
    usStatus(String(e), true);
    return null;
  }
}

async function loadUsers() {
  const s = await usCall("users_state");
  if (!s || s === true) return;
  us = /** @type {UsersState} */ (s);
  usStamp = Date.now();
  renderUsers();
}

/** "SL" from "Sam Lee". @param {string} name */
function initialsOf(name) {
  const w = name.trim().split(/\s+/).filter(Boolean);
  if (!w.length) return "?";
  const first = w[0][0] || "";
  return (w.length > 1 ? first + (w[w.length - 1][0] || "") : first).toUpperCase();
}

function renderUsers() {
  if (!us) return;
  $("us-name").textContent = us.name;
  $("us-login").textContent = us.login;
  const pic = $("us-picture");
  if (us.picture) {
    pic.style.backgroundImage = `url("kompact-icon://localhost/user-picture?${usStamp}")`;
    pic.textContent = "";
  } else {
    pic.style.backgroundImage = "";
    pic.textContent = initialsOf(us.name);
  }
  const full = /** @type {HTMLInputElement} */ ($("us-fullname"));
  if (document.activeElement !== full) full.value = us.name;
  /** @type {HTMLSelectElement} */ ($("us-bg")).value = us.background;
  $("us-image-row").classList.toggle("off", us.background !== "image");
  $("us-image-path").textContent = us.image || "None chosen";
  const thumb = /** @type {HTMLImageElement} */ ($("us-image"));
  const src = us.image ? `kompact-icon://localhost/user-login-image?${usStamp}` : "";
  if (thumb.getAttribute("src") !== src) thumb.setAttribute("src", src);
  const msg = /** @type {HTMLInputElement} */ ($("us-message"));
  if (document.activeElement !== msg) msg.value = us.message;
}

async function saveLoginPrefs() {
  const background = /** @type {HTMLSelectElement} */ ($("us-bg")).value;
  const message = /** @type {HTMLInputElement} */ ($("us-message")).value;
  usStatus("");
  if ((await usCall("set_login_prefs", { background, message })) !== null) usStatus("Saved; the lock screen shows it next time.");
  await loadUsers();
}

$("us-fullname").addEventListener("change", async (e) => {
  const name = /** @type {HTMLInputElement} */ (e.target).value;
  usStatus("");
  if ((await usCall("set_real_name", { name })) !== null) usStatus("Name changed.");
  await loadUsers();
});
$("us-pick").addEventListener("click", () => void usCall("choose_user_picture"));
$("us-image-pick").addEventListener("click", () => void usCall("choose_login_image"));
$("us-bg").addEventListener("change", () => {
  if (/** @type {HTMLSelectElement} */ ($("us-bg")).value === "image" && !us?.image) {
    void usCall("choose_login_image");
    return;
  }
  void saveLoginPrefs();
});
$("us-message").addEventListener("change", () => void saveLoginPrefs());
$("us-preview").addEventListener("click", () => void usCall("preview_lock"));

// ---- 13d: Control Center (menubar.rs; the menu bar applies the modes, web/menubar.js) ----

/** @param {Record<string, string> | null | undefined} m */
function renderMenuBar(m) {
  if (!m) return;
  document.querySelectorAll(".cc-select").forEach((el) => {
    const sel = /** @type {HTMLSelectElement} */ (el);
    const v = m[sel.dataset.item || ""];
    if (v) sel.value = v;
  });
}

async function loadMenuBar() {
  if (!tauri) return;
  try {
    renderMenuBar(await tauri.core.invoke("menubar_prefs"));
  } catch (e) {
    $("cc-status").textContent = String(e);
    $("cc-status").classList.add("error");
  }
}

document.querySelectorAll(".cc-select").forEach((el) =>
  el.addEventListener("change", async () => {
    const sel = /** @type {HTMLSelectElement} */ (el);
    const item = sel.dataset.item || "";
    console.log(`settings: menu bar ${item} = ${sel.value}`);
    $("cc-status").textContent = "";
    $("cc-status").classList.remove("error");
    try {
      await tauri?.core.invoke("set_menubar_pref", { item, mode: sel.value });
    } catch (e) {
      $("cc-status").textContent = String(e);
      $("cc-status").classList.add("error");
      void loadMenuBar();
    }
  }),
);

// ---- 12j: About (system.rs `about_state`) ----

/** @typedef {{version: string, os: string, kernel: string, cpu: string, gpu: string, memory: string,
 *   name: string, model: string, gpu_model: string, disk: string, uptime: string}} AboutInfo */

async function loadAbout() {
  if (!tauri) return;
  /** @type {AboutInfo} */
  let a;
  try {
    a = await tauri.core.invoke("about_state");
  } catch (e) {
    $("ab-status").textContent = String(e);
    $("ab-status").classList.add("error");
    return;
  }
  $("ab-name").textContent = a.name || "This Computer";
  $("ab-model").textContent = a.model;
  const graphics = a.gpu_model ? `${a.gpu_model}${a.gpu ? ` — ${a.gpu}` : ""}` : a.gpu;
  /** @type {[string, string][]} */
  const facts = [
    ["Name", a.name],
    ["Model", a.model],
    ["Processor", a.cpu],
    ["Graphics", graphics],
    ["Memory", a.memory],
    ["Startup disk", a.disk],
    ["Operating system", a.os],
    ["Kernel", a.kernel],
    // 15a: the two halves of the desktop, each with its version.
    ["Desktop", `kompact desktop ${kset?.version || a.version.replace(/^kompact-shell /, "")} (Wayland)`],
    ["Compositor", kset?.version ? `kompact ${kset.version}` : "not connected"],
    ["Shell", a.version],
    ["Up for", a.uptime],
  ];
  const card = $("ab-facts");
  card.replaceChildren();
  for (const [label, value] of facts) {
    if (!value) continue;
    const row = document.createElement("div");
    row.className = "row";
    const l = document.createElement("span");
    l.className = "row-label";
    l.textContent = label;
    const v = document.createElement("span");
    v.className = "ab-value";
    v.textContent = value;
    row.append(l, v);
    card.append(row);
  }
  $("ab-note").textContent = "The computer's name can be changed with hostnamectl; displays are under Displays.";
  console.log(`settings: about ${a.name} / ${a.model}`);
}

// ---- 12j: Default Apps (defaults.rs; the terminal is kompact's `terminal` setting) ----

/** @typedef {{id: string, name: string, command?: string}} DefaultApp */
/** @typedef {{apps: DefaultApp[], current?: string | null}} DefaultRole */
/** @type {Record<string, DefaultRole> | null} */
let defaults = null;

async function loadDefaults() {
  if (!tauri) return;
  try {
    defaults = await tauri.core.invoke("defaults_state");
  } catch (e) {
    dfStatus(String(e), true);
    return;
  }
  renderDefaults();
}

/** @param {string} text @param {boolean} [error] */
function dfStatus(text, error = false) {
  $("df-status").textContent = text;
  $("df-status").classList.toggle("error", error);
}

/** @param {string} id */
function iconUrl(id) {
  return id ? `kompact-icon://localhost/${encodeURIComponent(id)}` : "";
}

function renderDefaults() {
  if (!defaults) return;
  for (const role of ["browser", "mail", "files", "terminal"]) {
    const sel = /** @type {HTMLSelectElement} */ ($(`df-${role}`));
    const r = defaults[role] || { apps: [] };
    /** @type {[string, string][]} */
    const options = r.apps.map((a) => [a.id, a.name]);
    let chosen = "";
    if (role === "terminal") {
      // kompact's setting is a command: find the app with that command.
      const cmd = kset?.terminal || "";
      const auto = kset?.terminal_auto || "";
      chosen = cmd ? r.apps.find((a) => a.command === cmd)?.id || `cmd:${cmd}` : "";
      options.unshift(["", auto ? `kompact's own (${auto})` : "kompact's own"]);
      if (chosen.startsWith("cmd:")) options.push([chosen, cmd]);
      sel.disabled = !kset;
      $("df-terminal-hint").textContent = kset ? "Starts with the Open a terminal shortcut." : "kompact isn't reachable.";
    } else {
      chosen = r.current || "";
      if (!chosen) options.unshift(["", r.apps.length ? "None chosen" : "None installed"]);
      sel.disabled = !r.apps.length;
    }
    sel.replaceChildren(
      ...options.map(([v, label]) => {
        const o = document.createElement("option");
        o.value = v;
        o.textContent = label;
        return o;
      }),
    );
    sel.value = chosen;
    const icon = /** @type {HTMLImageElement} */ ($(`df-${role}-icon`));
    const id = chosen.startsWith("cmd:") ? "" : chosen;
    if (icon.getAttribute("src") !== iconUrl(id)) icon.setAttribute("src", iconUrl(id));
  }
}

document.querySelectorAll(".df-select").forEach((el) =>
  el.addEventListener("change", async () => {
    const sel = /** @type {HTMLSelectElement} */ (el);
    const role = sel.dataset.role || "";
    if (sel.value.startsWith("cmd:")) return;
    const name = sel.selectedOptions[0]?.textContent || sel.value;
    console.log(`settings: default ${role} = ${sel.value || "(kompact's own)"}`);
    try {
      await tauri?.core.invoke("defaults_set", { role, id: sel.value });
      dfStatus(role === "terminal" ? `Open a terminal now starts ${name}.` : `${name} is now the default.`);
    } catch (e) {
      dfStatus(String(e), true);
    }
    // The terminal comes back with kompact's `settings` message.
    if (role !== "terminal") await loadDefaults();
  }),
);

// ---- 12i-1: Lock Screen (kompact's `lock-after`; `idle` messages say what keeps it awake) ----

/** @type {string[]} */
let awakeApps = [];

function renderLock() {
  const k = kset;
  const sel = /** @type {HTMLSelectElement} */ ($("lk-after"));
  sel.disabled = !k;
  if (k) sel.value = String(k.lock_after ?? 0);
  const off = /** @type {HTMLSelectElement} */ ($("lk-off"));
  off.disabled = !k;
  if (k) off.value = String(k.screen_off_after ?? 0);
  $("lk-after-hint").textContent = !k ? "kompact isn't reachable." : k.lock_after ? "After that long without a key, a click or the pointer moving." : "Only when you lock it.";
  $("lk-awake").textContent = awakeApps.length ? awakeApps.join(", ") : "Nothing right now.";
  if (k?.message && kompactStatus === "lk-status") {
    $("lk-status").textContent = k.message;
    $("lk-status").classList.add("error");
  }
}

$("lk-off").addEventListener("change", (e) => setKompact("screen-off-after", /** @type {HTMLSelectElement} */ (e.target).value));
$("lk-after").addEventListener("change", (e) => setKompact("lock-after", /** @type {HTMLSelectElement} */ (e.target).value));
$("lk-now").addEventListener("click", () => {
  console.log("settings: lock the screen now");
  tauri?.core.invoke("window_action", { action: "lock" }).catch((/** @type {unknown} */ e) => {
    $("lk-status").textContent = `kompact isn't reachable: ${e}`;
    $("lk-status").classList.add("error");
  });
});

// ---- 12h-1: Displays (kompact's arrangement; it snaps a dropped display next to the others) ----

/** @typedef {{name: string, x: number, y: number, w: number, h: number, refresh: number, mm_w: number, mm_h: number}} DisplayInfo */
/** @type {DisplayInfo[]} */
let displays = [];
/** 12h-2: the main display, and whether every display has a menu bar. */
let dsMain = "";
let dsEvery = false;
/** Logical px → CSS px on the stage, as last drawn (a drag keeps it). */
let dsView = { scale: 0.1, ox: 0, oy: 0 };
/** @type {{name: string, el: HTMLElement, id: number, sx: number, sy: number, left: number, top: number, moved: boolean} | null} */
let dsDrag = null;

/** @param {string} msg */
function dsStatus(msg, error = false) {
  $("ds-status").textContent = msg;
  $("ds-status").classList.toggle("error", error);
}

async function loadDisplays() {
  if (!tauri) return;
  const msg = await tauri.core.invoke("displays_state").catch(() => null);
  if (msg) dsTake(msg);
  renderDisplays();
}

/** @param {{outputs?: DisplayInfo[], primary?: string, menu_bars?: string}} msg */
function dsTake(msg) {
  displays = msg.outputs || [];
  dsMain = msg.primary || "";
  dsEvery = msg.menu_bars === "every";
}

function renderDisplays() {
  if (dsDrag) return;
  const stage = $("ds-stage");
  const list = $("ds-list");
  if (!displays.length) {
    stage.replaceChildren(Object.assign(document.createElement("div"), { className: "ds-empty", textContent: "kompact hasn't said where the displays are yet." }));
    list.replaceChildren();
    /** @type {HTMLInputElement} */ ($("ds-every")).disabled = true;
    return;
  }
  // The whole arrangement in the middle, with room around it to drag a display to any side.
  const minX = Math.min(...displays.map((d) => d.x));
  const minY = Math.min(...displays.map((d) => d.y));
  const bw = Math.max(...displays.map((d) => d.x + d.w)) - minX;
  const bh = Math.max(...displays.map((d) => d.y + d.h)) - minY;
  const W = stage.clientWidth || 480;
  const H = stage.clientHeight || 240;
  const scale = Math.min((W * 0.55) / bw, (H * 0.55) / bh);
  dsView = { scale, ox: (W - bw * scale) / 2 - minX * scale, oy: (H - bh * scale) / 2 - minY * scale };
  const every = /** @type {HTMLInputElement} */ ($("ds-every"));
  every.checked = dsEvery;
  every.disabled = displays.length < 2;
  stage.replaceChildren(
    ...displays.map((d) => {
      const el = document.createElement("div");
      el.className = "ds-display";
      el.dataset.name = d.name;
      el.style.left = `${d.x * scale + dsView.ox}px`;
      el.style.top = `${d.y * scale + dsView.oy}px`;
      el.style.width = `${d.w * scale}px`;
      el.style.height = `${d.h * scale}px`;
      el.title = `${d.name}: drag to move it`;
      if (d.name === dsMain || dsEvery) el.append(Object.assign(document.createElement("span"), { className: d.name === dsMain ? "ds-bar" : "ds-bar dim" }));
      el.append(
        Object.assign(document.createElement("span"), { className: "ds-name", textContent: d.name }),
        Object.assign(document.createElement("span"), { className: "ds-size", textContent: `${d.w} × ${d.h}` }),
      );
      el.addEventListener("pointerdown", (e) => dsDown(e, d.name, el));
      return el;
    }),
  );
  list.replaceChildren(
    ...displays.map((d) => {
      const row = document.createElement("div");
      row.className = "row";
      const hz = d.refresh ? `, ${Math.round(d.refresh / 1000)} Hz` : "";
      const inches = d.mm_w && d.mm_h ? `, ${Math.round(Math.hypot(d.mm_w, d.mm_h) / 25.4)}-inch` : "";
      row.append(netLabel(d.name, `${d.w} × ${d.h}${hz}${inches}, at ${d.x}, ${d.y}`));
      if (d.name === dsMain) row.append(Object.assign(document.createElement("span"), { className: "ds-main", textContent: "Main display" }));
      else if (displays.length > 1) {
        const b = Object.assign(document.createElement("button"), { className: "btn", textContent: "Use as Main Display" });
        b.addEventListener("click", async () => {
          console.log(`settings: main display ${d.name}`);
          dsStatus(`Making ${d.name} the main display…`);
          try {
            await tauri?.core.invoke("display_main", { name: d.name });
            dsStatus("");
          } catch (err) {
            dsStatus(String(err), true);
          }
        });
        row.append(b);
      }
      return row;
    }),
  );
}

/** @param {PointerEvent} e @param {string} name @param {HTMLElement} el */
function dsDown(e, name, el) {
  if (e.button !== 0 || dsDrag) return;
  e.preventDefault();
  el.setPointerCapture(e.pointerId);
  el.classList.add("dragging");
  dsDrag = { name, el, id: e.pointerId, sx: e.clientX, sy: e.clientY, left: parseFloat(el.style.left), top: parseFloat(el.style.top), moved: false };
}

$("ds-stage").addEventListener("pointermove", (e) => {
  const d = dsDrag;
  if (!d || e.pointerId !== d.id) return;
  const dx = e.clientX - d.sx;
  const dy = e.clientY - d.sy;
  if (Math.abs(dx) + Math.abs(dy) > 3) d.moved = true;
  d.el.style.left = `${d.left + dx}px`;
  d.el.style.top = `${d.top + dy}px`;
});

/** @param {PointerEvent} e */
async function dsUp(e) {
  const d = dsDrag;
  if (!d || e.pointerId !== d.id) return;
  dsDrag = null;
  d.el.classList.remove("dragging");
  if (!d.moved || e.type === "pointercancel") return renderDisplays();
  const x = Math.round((parseFloat(d.el.style.left) - dsView.ox) / dsView.scale);
  const y = Math.round((parseFloat(d.el.style.top) - dsView.oy) / dsView.scale);
  console.log(`settings: display ${d.name} dropped at ${x},${y}`);
  dsStatus(`Moving ${d.name}…`);
  try {
    // kompact answers with `kompact-displays`, which draws the snapped arrangement.
    await tauri?.core.invoke("display_move", { name: d.name, x, y });
    dsStatus("");
  } catch (err) {
    dsStatus(String(err), true);
    renderDisplays();
  }
}
$("ds-stage").addEventListener("pointerup", dsUp);
$("ds-every").addEventListener("change", async (e) => {
  const every = /** @type {HTMLInputElement} */ (e.target).checked;
  console.log(`settings: menu bar on ${every ? "every display" : "the main display"}`);
  try {
    await tauri?.core.invoke("menu_bars", { every });
  } catch (err) {
    dsStatus(String(err), true);
  }
});
$("ds-stage").addEventListener("pointercancel", dsUp);
window.addEventListener("resize", () => {
  if (!$("pane-displays").hidden) renderDisplays();
});

// ---- 12f: Mouse & Keyboard (kompact's settings; the layouts' names from xkb's catalogue) ----

/** @type {{code: string, name: string}[] | null} */
let catalogue = null;
/** A slider being dragged: kompact's answers don't move it. */
let inDragging = "";
let inTimer = 0;

function layoutName(/** @type {string} */ code) {
  return catalogue?.find((l) => l.code === code)?.name || code;
}

function renderInput() {
  const k = kset;
  /** @type {[string, keyof KompactSettings][]} */
  const switches = [["in-accel", "pointer_accel"], ["in-natural", "natural_scroll"], ["in-mnatural", "mouse_natural_scroll"], ["in-left", "left_handed"], ["in-tap", "tap_to_click"]];
  for (const [id, key] of switches) {
    const el = /** @type {HTMLInputElement} */ ($(id));
    el.checked = !!k?.[key];
    el.disabled = !k;
  }
  const speed = /** @type {HTMLInputElement} */ ($("in-speed"));
  if (k && inDragging !== "speed") speed.value = String(Math.round(k.pointer_speed * 10));
  // 13h: older kompacts don't send it.
  const scroll = /** @type {HTMLInputElement} */ ($("in-scroll"));
  if (k && inDragging !== "scroll") scroll.value = String(k.scroll_speed ?? 100);
  scroll.disabled = !k || k.scroll_speed === undefined;
  $("in-scroll-val").textContent = `${scroll.value}%`;
  const repeat = /** @type {HTMLInputElement} */ ($("in-repeat"));
  repeat.checked = !!k && k.repeat_rate > 0;
  const delay = /** @type {HTMLInputElement} */ ($("in-delay"));
  const rate = /** @type {HTMLInputElement} */ ($("in-rate"));
  if (k && inDragging !== "delay") delay.value = String(k.repeat_delay);
  if (k && inDragging !== "rate" && k.repeat_rate > 0) rate.value = String(k.repeat_rate);
  $("in-delay-val").textContent = `${delay.value} ms`;
  $("in-rate-val").textContent = `${rate.value}/s`;
  $("in-delay-row").classList.toggle("off", !repeat.checked);
  $("in-rate-row").classList.toggle("off", !repeat.checked);
  delay.disabled = rate.disabled = !repeat.checked;
  document.querySelectorAll("[data-caps]").forEach((b) => b.setAttribute("aria-checked", String(k?.caps === /** @type {HTMLElement} */ (b).dataset.caps)));
  // Layouts: the first is the default.
  const codes = (k?.layouts || "us").split(",").filter(Boolean);
  $("in-layouts").replaceChildren(
    ...codes.map((code, i) => {
      const row = document.createElement("div");
      row.className = "row";
      row.append(netLabel(layoutName(code), i === 0 ? "Default" : undefined));
      if (i > 0) {
        const up = Object.assign(document.createElement("button"), { className: "btn", textContent: "Make Default" });
        up.addEventListener("click", () => setKompact("layouts", [code, ...codes.filter((c) => c !== code)].join(",")));
        row.append(up);
      }
      if (codes.length > 1) {
        const rm = Object.assign(document.createElement("button"), { className: "btn", textContent: "Remove" });
        rm.addEventListener("click", () => setKompact("layouts", codes.filter((c) => c !== code).join(",")));
        row.append(rm);
      }
      return row;
    }),
  );
  const add = /** @type {HTMLInputElement} */ ($("in-add"));
  add.disabled = codes.length >= 4;
  add.placeholder = codes.length >= 4 ? "Four layouts at most" : "Add a layout: search, e.g. German, Hindi, Dvorak";
  // The switch keys, from Keyboard Shortcuts.
  const next = keymap?.actions.find((a) => a.id === "input.next-layout");
  $("in-switch-hint").textContent =
    codes.length > 1
      ? `Switch layouts with ${next?.keys.length ? next.keys.map((c) => caps(c).join(" ")).join(" or ") : "the keyboard icon"} in the menu bar${next?.keys.length ? ", or the keyboard icon in the menu bar" : ""}.`
      : "With two or more layouts, a keyboard icon in the menu bar switches between them.";
}

function renderLayoutSearch() {
  const q = /** @type {HTMLInputElement} */ ($("in-add")).value.trim().toLowerCase();
  const box = $("in-results");
  if (!q || !catalogue) return box.replaceChildren();
  const have = (kset?.layouts || "").split(",");
  const hits = catalogue.filter((l) => !have.includes(l.code) && (l.name.toLowerCase().includes(q) || l.code.includes(q))).slice(0, 40);
  box.replaceChildren(
    ...hits.map((l) => {
      const row = document.createElement("div");
      row.className = "row";
      row.append(netLabel(l.name, l.code));
      row.addEventListener("click", () => {
        /** @type {HTMLInputElement} */ ($("in-add")).value = "";
        box.replaceChildren();
        setKompact("layouts", [...(kset?.layouts || "us").split(",").filter(Boolean), l.code].join(","));
      });
      return row;
    }),
  );
  if (!hits.length) box.append(Object.assign(document.createElement("div"), { className: "row muted", textContent: "No layout by that name." }));
}

async function loadCatalogue() {
  if (catalogue || !tauri) return;
  catalogue = (await tauri.core.invoke("keyboard_layouts").catch(() => [])) || [];
  renderInput();
}

for (const [id, key] of [["in-accel", "pointer-accel"], ["in-natural", "natural-scroll"], ["in-mnatural", "mouse-natural-scroll"], ["in-left", "left-handed"], ["in-tap", "tap-to-click"]]) {
  $(id).addEventListener("change", (e) => setKompact(key, /** @type {HTMLInputElement} */ (e.target).checked ? "on" : "off"));
}
/** @param {string} id @param {string} drag @param {string} key @param {(v: number) => string} value */
function hookInputSlider(id, drag, key, value) {
  const el = /** @type {HTMLInputElement} */ ($(id));
  el.addEventListener("input", () => {
    inDragging = drag;
    if (drag === "delay") $("in-delay-val").textContent = `${el.value} ms`;
    if (drag === "rate") $("in-rate-val").textContent = `${el.value}/s`;
    if (drag === "scroll") $("in-scroll-val").textContent = `${el.value}%`;
    clearTimeout(inTimer);
    inTimer = setTimeout(() => setKompact(key, value(Number(el.value))), 150);
  });
  el.addEventListener("change", () => {
    inDragging = "";
    clearTimeout(inTimer);
    setKompact(key, value(Number(el.value)));
  });
}
hookInputSlider("in-speed", "speed", "pointer-speed", (v) => (v / 10).toFixed(1));
hookInputSlider("in-scroll", "scroll", "scroll-speed", (v) => String(v));
hookInputSlider("in-delay", "delay", "repeat-delay", (v) => String(v));
hookInputSlider("in-rate", "rate", "repeat-rate", (v) => String(v));
$("in-repeat").addEventListener("change", (e) =>
  setKompact("repeat-rate", /** @type {HTMLInputElement} */ (e.target).checked ? /** @type {HTMLInputElement} */ ($("in-rate")).value || "25" : "0"),
);
document.querySelectorAll("[data-caps]").forEach((b) =>
  b.addEventListener("click", () => {
    const v = /** @type {HTMLElement} */ (b).dataset.caps;
    if (v && kset?.caps !== v) setKompact("caps", v);
  }),
);
$("in-add").addEventListener("input", renderLayoutSearch);

function renderWindows() {
  renderDefaults();
  renderAppearance();
  renderInput();
  renderLock();
  renderShots();
  const k = kset;
  const anim = /** @type {HTMLInputElement} */ ($("win-anim"));
  const ffm = /** @type {HTMLInputElement} */ ($("win-ffm"));
  const dbl = document.querySelectorAll("[data-dbl]");
  const locked = !!k?.locked.includes("animations");
  anim.checked = !!k?.animations;
  anim.disabled = !k || locked;
  $("anim-row").classList.toggle("off", locked);
  $("anim-hint").textContent = locked
    ? "Set by KOMPACT_ANIMATIONS for this session."
    : "Minimizing shrinks a window into its Dock icon; maximizing and snapping glide.";
  ffm.checked = !!k?.focus_follows_mouse;
  ffm.disabled = !k;
  dbl.forEach((b) => {
    b.setAttribute("aria-checked", String(k?.title_double_click === /** @type {HTMLElement} */ (b).dataset.dbl));
    /** @type {HTMLButtonElement} */ (b).disabled = !k;
  });
  if (k?.message) {
    const el = $(kompactStatus);
    el.textContent = k.message;
    el.classList.add("error");
  }
}

/** 12a fix: where a message from kompact goes: the status line of the pane whose setting was
 * changed last (it used to be Desktop & Dock's, whatever was changed). */
let kompactStatus = "dock-status";
const STATUS_OF = /** @type {Record<string, string>} */ ({
  appearance: "look-status", accent: "look-status", buttons: "look-status",
  "shot-banner": "shots-status", "shot-pointer": "shots-status", "shots-dir": "shots-status",
  "pointer-speed": "in-status", "pointer-accel": "in-status", "natural-scroll": "in-status", "left-handed": "in-status",
  "mouse-natural-scroll": "in-status", "scroll-speed": "in-status",
  "tap-to-click": "in-status", "repeat-delay": "in-status", "repeat-rate": "in-status", layouts: "in-status", caps: "in-status",
  "lock-after": "lk-status", "screen-off-after": "lk-status",
});

/** @param {string} key @param {string} value */
function setKompact(key, value) {
  if (!tauri) return;
  kompactStatus = STATUS_OF[key] || "dock-status";
  const el = $(kompactStatus);
  el.textContent = "";
  el.classList.remove("error");
  console.log(`settings: kompact ${key} = ${value}`);
  tauri.core.invoke("kompact_set", { key, value }).catch((/** @type {unknown} */ e) => {
    const el = $(kompactStatus);
    el.textContent = `kompact isn't reachable: ${e}`;
    el.classList.add("error");
    renderWindows();
  });
}

$("win-anim").addEventListener("change", (e) => setKompact("animations", /** @type {HTMLInputElement} */ (e.target).checked ? "on" : "off"));
$("win-ffm").addEventListener("change", (e) => setKompact("focus-follows-mouse", /** @type {HTMLInputElement} */ (e.target).checked ? "on" : "off"));
document.querySelectorAll("[data-dbl]").forEach((b) =>
  b.addEventListener("click", () => {
    const v = /** @type {HTMLElement} */ (b).dataset.dbl;
    if (v && kset?.title_double_click !== v) setKompact("title-double-click", v);
  }),
);
renderWindows();

// ---- 10d-2: Wallpaper. The shell keeps it and tells every page (`kompact-wallpaper-changed`).

/** @typedef {{kind: "gradient" | "color" | "image", color: string | null, image: string | null,
 *   version: number, locked: boolean, off: boolean}} WallpaperState */
/** @type {WallpaperState | null} */
let wp = null;
/** @type {{index: number, name: string, path: string}[] | null} */
let pictures = null;
const COLORS = ["#1d1d1f", "#3a3a3c", "#1c2b3a", "#24364f", "#0a66d9", "#2f7d6d", "#4f7a28", "#5b3f8c", "#8c3f6f", "#b23b3b", "#c7802b", "#d8c7a0"];

function renderWallpaper() {
  const s = wp;
  const cur = $("wp-current");
  cur.className = "wp-current";
  cur.style.backgroundImage = cur.style.backgroundColor = "";
  let name = "…";
  if (s?.kind === "image" && s.image) {
    cur.style.backgroundImage = `url("kompact-wallpaper://localhost/small?v=${s.version}")`;
    name = s.image.split("/").pop()?.replace(/\.[^.]+$/, "").replace(/[_-]/g, " ") || s.image;
  } else if (s?.kind === "color" && s.color) {
    cur.style.backgroundColor = s.color;
    name = `Colour ${s.color}`;
  } else if (s) {
    cur.classList.add("gradient");
    name = "kompact gradient";
  }
  $("wp-name").textContent = name;
  $("wp-hint").textContent = s?.off
    ? "Turned off for this session (KOMPACT_WALLPAPER=none)."
    : s?.locked
      ? "Set by KOMPACT_WALLPAPER for this session; Settings can't change it."
      : s?.kind === "image" && s.image
        ? s.image
        : "";
  const fixed = !s || s.locked;
  /** @type {HTMLButtonElement} */ ($("wp-choose")).disabled = fixed;
  /** @type {HTMLInputElement} */ ($("wp-custom")).disabled = fixed;
  const swatches = $("wp-colors");
  swatches.replaceChildren(
    ...[null, ...COLORS].map((c) => {
      const b = document.createElement("button");
      b.className = c ? "swatch" : "swatch gradient";
      if (c) b.style.background = c;
      b.title = c ? c : "kompact gradient";
      b.setAttribute("role", "radio");
      b.setAttribute("aria-checked", String(c ? s?.kind === "color" && s.color === c : s?.kind === "gradient"));
      b.disabled = fixed;
      b.addEventListener("click", () => setWallpaper(c ? "color" : "gradient", c));
      return b;
    }),
  );
  const grid = $("wp-pictures");
  grid.replaceChildren(
    ...(pictures || []).map((p) => {
      const b = document.createElement("button");
      b.className = "pic";
      b.title = p.path;
      b.setAttribute("role", "radio");
      b.setAttribute("aria-checked", String(s?.kind === "image" && s.image === p.path));
      b.disabled = fixed;
      const img = document.createElement("img");
      img.alt = "";
      img.loading = "lazy";
      img.decoding = "async";
      img.src = `kompact-wallpaper://localhost/preview/${p.index}`;
      const label = document.createElement("span");
      label.textContent = p.name;
      b.append(img, label);
      b.addEventListener("click", () => setWallpaper("image", p.path));
      return b;
    }),
  );
  if (pictures && !pictures.length) grid.textContent = "No pictures in /usr/share/backgrounds or ~/Pictures.";
}

/** @param {string} msg @param {boolean} [error] */
function wpStatus(msg, error = false) {
  const el = $("wp-status");
  el.textContent = msg;
  el.classList.toggle("error", error);
}

/** @param {"gradient" | "color" | "image"} kind @param {string | null} value */
function setWallpaper(kind, value) {
  if (!tauri) return;
  console.log(`settings: wallpaper ${kind} ${value || ""}`);
  tauri.core
    .invoke("set_wallpaper", { kind, value })
    .then((/** @type {WallpaperState} */ st) => {
      wp = st;
      wpStatus("");
      renderWallpaper();
    })
    .catch((/** @type {unknown} */ e) => wpStatus(`Couldn't change the wallpaper: ${e}`, true));
}

function loadPictures() {
  if (!tauri || pictures) return;
  pictures = [];
  tauri.core.invoke("wallpaper_pictures").then((/** @type {any} */ list) => {
    pictures = list || [];
    renderWallpaper();
  });
}

$("wp-choose").addEventListener("click", () => tauri?.core.invoke("choose_wallpaper"));
$("wp-custom").addEventListener("change", (e) => setWallpaper("color", /** @type {HTMLInputElement} */ (e.target).value));
renderWallpaper();

$("search").addEventListener("input", () => {
  // The search finds shortcuts.
  if (/** @type {HTMLInputElement} */ ($("search")).value) showPane("keys");
  render();
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && !$("dialog").hidden) {
    $("dialog").hidden = true;
    // 12b: Esc on a pairing question is a no (else BlueZ waits for its timeout).
    if (dialogFor) {
      void tauri?.core.invoke("bluetooth_answer", { id: dialogFor, yes: false }).catch(() => {});
      dialogFor = 0;
    }
  }
});
// Leaving the window while recording: stop, so the next combination isn't swallowed.
window.addEventListener("blur", () => {
  if (recording) {
    recording = null;
    keys("record-cancel");
    render();
  }
});

if (tauri) {
  tauri.event.listen("kompact-keymap", (/** @type {{payload: Keymap}} */ e) => {
    keymap = e.payload;
    render();
    renderShots();
  });
  tauri.event.listen("kompact-keys-result", (/** @type {{payload: any}} */ e) => onResult(e.payload));
  tauri.event.listen("kompact-recorded", (/** @type {{payload: any}} */ e) => onRecorded(e.payload));
  tauri.core.invoke("keymap_state").then((/** @type {Keymap | null} */ k) => {
    if (k && !keymap) {
      keymap = k;
      render();
    }
  });
  keys(""); // and a fresh one
  // 10d-1: the dock's right-click menu changes the same settings.
  tauri.event.listen("kompact-dock-changed", () => void loadDock());
  // 10d-1: opened for a pane ("Dock Settings…" in the dock's menu).
  const takePane = () =>
    tauri.core.invoke("take_settings_pane").then((/** @type {string | null} */ p) => {
      if (p && document.getElementById(`pane-${p}`)) showPane(p);
    });
  tauri.event.listen("kompact-settings-pane", () => void takePane());
  // 10d-2
  tauri.event.listen("kompact-wallpaper-changed", (/** @type {{payload: WallpaperState}} */ e) => {
    wp = e.payload;
    renderWallpaper();
  });
  tauri.event.listen("kompact-wallpaper-error", (/** @type {{payload: string}} */ e) => wpStatus(String(e.payload), true));
  tauri.core.invoke("wallpaper_state").then((/** @type {WallpaperState} */ st) => {
    wp = st;
    renderWallpaper();
  });
  // 12d: Do Not Disturb from Control Center or the Notification Center.
  tauri.event.listen("kompact-notify-prefs", (/** @type {{payload: NotifyPrefs}} */ e) => {
    nt = e.payload;
    renderNotify();
  });
  // 19c-2: a request it just handled.
  tauri.event.listen("kompact-assistant-activity", () => {
    if (paneShown === "assistant") void loadActivity();
  });
  // 12i-1
  tauri.event.listen("kompact-idle", (/** @type {{payload: {awake?: string[]}}} */ e) => {
    awakeApps = e.payload.awake || [];
    renderLock();
  });
  tauri.core.invoke("idle_state").then((/** @type {{awake?: string[]} | null} */ m) => {
    if (m) {
      awakeApps = m.awake || [];
      renderLock();
    }
  });
  // 12h-1
  tauri.event.listen("kompact-displays", (/** @type {{payload: {outputs?: DisplayInfo[], primary?: string, menu_bars?: string}}} */ e) => {
    dsTake(e.payload);
    if (!$("pane-displays").hidden) renderDisplays();
  });
  // 12c
  tauri.event.listen("kompact-sound-changed", () => {
    if (!$("pane-sound").hidden && !sdDragging) void loadSound();
  });
  // 12b
  tauri.event.listen("kompact-bluetooth-changed", () => {
    if (!$("pane-bluetooth").hidden) void loadBluetooth();
  });
  tauri.event.listen("kompact-bluetooth-ask", (/** @type {{payload: BtQuestion}} */ e) => btAsk(e.payload));
  tauri.event.listen("kompact-bluetooth-asked", (/** @type {{payload: number}} */ e) => {
    if (dialogFor === e.payload) {
      $("dialog").hidden = true;
      dialogFor = 0;
    }
  });
  tauri.core.invoke("bluetooth_pending").then((/** @type {BtQuestion[]} */ list) => list?.length && btAsk(list[list.length - 1]));
  // 12a
  tauri.event.listen("kompact-network-changed", () => {
    if (!$("pane-network").hidden && !netAsking) void loadNetwork();
  });
  tauri.event.listen("kompact-network-joined", (/** @type {{payload: {ssid: string, ok: boolean, message: string}}} */ e) => {
    netJoining = "";
    netResult = e.payload;
    const el = $("net-status");
    el.textContent = e.payload.ok ? e.payload.message : "";
    el.classList.remove("error");
    void loadNetwork();
  });
  // 10d-5: a share starting or ending (the menu bar's pill) changes what the pane says.
  tauri.event.listen("kompact-sharing", () => void setTimeout(loadSharing, 300));
  // 14b
  tauri.event.listen("kompact-users-changed", () => {
    if (!$("pane-users").hidden) void loadUsers();
  });
  tauri.event.listen("kompact-users-error", (/** @type {{payload: string}} */ e) => usStatus(e.payload, true));
  // 13d
  tauri.event.listen("kompact-menubar", (/** @type {{payload: Record<string, string>}} */ e) => renderMenuBar(e.payload));
  // 11c
  tauri.event.listen("kompact-settings", (/** @type {{payload: KompactSettings}} */ e) => {
    kset = e.payload;
    renderWindows();
    if (paneShown === "about") void loadAbout();
  });
  tauri.core.invoke("kompact_settings_state").then((/** @type {KompactSettings | null} */ k) => {
    if (k && !kset) {
      kset = k;
      renderWindows();
    }
  });
  void takePane();
  void loadDock();
}
render();
console.log("settings: page started");
