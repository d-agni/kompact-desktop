// @ts-check
// 13d: which status items the menu bar shows, as macOS does. Each item is Always, When Active or
// Never (Settings → Control Center, `menubar.rs`); Control Center has them all anyway, so
// "active" means "something worth seeing at a glance". Pure, so `menubar.test.js` checks it
// under gjs; panel.js applies it.
"use strict";

/** @typedef {"always" | "active" | "never"} MenuBarMode */
/** @typedef {{
 *   network?: {icon: string, has_wifi: boolean} | null,
 *   bluetooth?: {adapter: string | null, powered?: boolean, connected: number} | null,
 *   sound?: {output: boolean, muted?: boolean, playing?: string[]} | null,
 *   input?: {layouts: number} | null,
 *   spaces?: {current: number, others_in_use: boolean} | null,
 *   assistant?: {active: boolean} | null,
 * }} MenuBarState */

/** @type {Record<string, MenuBarMode>} */
const MENU_BAR_DEFAULTS = { network: "active", bluetooth: "active", sound: "active", input: "active", spaces: "active", spotlight: "always", assistant: "always" };

/** When the bar is too narrow, these fold away first (all but Desktops and Spotlight are in
 * Control Center; those two have their shortcuts). */
const MENU_BAR_FOLD = ["spaces", "spotlight", "input", "sound", "bluetooth", "network"];

/** Whether `item` shows, for its `mode` and what's going on now.
 * @param {string} item @param {MenuBarMode | undefined} mode @param {MenuBarState} st */
function menuBarShows(item, mode, st) {
  mode = mode || MENU_BAR_DEFAULTS[item] || "always";
  if (mode === "never") return false;
  const always = mode === "always";
  switch (item) {
    case "network": {
      const n = st.network;
      if (!n) return false;
      // Wired and working, no Wi-Fi to pick: nothing to see. Wi-Fi, offline or connecting: show.
      return always || n.has_wifi || n.icon !== "ethernet";
    }
    case "bluetooth": {
      const b = st.bluetooth;
      if (!b || !b.adapter) return false;
      return always || (!!b.powered && b.connected > 0);
    }
    case "sound": {
      const s = st.sound;
      if (!s || !s.output) return false;
      return always || !!s.muted || (s.playing || []).length > 0;
    }
    case "input": {
      const n = st.input?.layouts || 0;
      return always ? n >= 1 : n >= 2;
    }
    case "spaces": {
      const w = st.spaces;
      if (always) return true;
      return !!w && (w.current !== 1 || w.others_in_use);
    }
    case "spotlight":
      return true;
    // 19h: When Active: while it listens or works (its shortcut opens it, 19d-2).
    case "assistant":
      return always || !!st.assistant?.active;
    default:
      return true;
  }
}

/** The items to fold, given what shows and how many pixels too wide the bar is: the first ones
 * of `MENU_BAR_FOLD` that show, until their widths cover `over`.
 * @param {Record<string, number>} widths of the items that show @param {number} over */
function menuBarFolds(widths, over) {
  /** @type {string[]} */
  const out = [];
  let freed = 0;
  for (const item of MENU_BAR_FOLD) {
    if (freed >= over) break;
    if (!(item in widths)) continue;
    out.push(item);
    freed += widths[item];
  }
  return out;
}
