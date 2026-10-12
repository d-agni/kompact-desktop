// @ts-check
// Panel clock. Shows hours and minutes only and wakes once a minute, so an idle desktop still
// costs kompact about one frame a minute (the Phase 4 budget is 0 flips while idle).
"use strict";

/** @param {Date} now */
function clockText(now) {
  const date = now.toLocaleDateString(undefined, { weekday: "short", day: "numeric", month: "short" });
  const time = now.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
  return `${date}  ${time}`;
}

/** Milliseconds until the next minute starts (at least 1 s, so a late timer can't spin).
 * @param {Date} now */
function msToNextMinute(now) {
  return Math.max(1000, 60000 - (now.getSeconds() * 1000 + now.getMilliseconds()));
}

function tick() {
  const el = /** @type {HTMLTimeElement} */ (document.getElementById("clock"));
  const now = new Date();
  el.textContent = clockText(now);
  el.dateTime = now.toISOString();
  setTimeout(tick, msToNextMinute(now));
}

tick();
console.log("panel: clock started");

