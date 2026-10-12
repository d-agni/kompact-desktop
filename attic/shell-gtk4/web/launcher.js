// @ts-check
// Launcher (6c-2). kompact-shell calls `window.kompactLauncher.open(apps)` each time it shows the
// launcher; the page reports back with "launch:<desktop id>" or "close-launcher".
"use strict";

/** @typedef {{id: string, name: string, comment: string, keywords: string}} App */

const search = /** @type {HTMLInputElement} */ (document.getElementById("search"));
const list = /** @type {HTMLUListElement} */ (document.getElementById("list"));
const empty = /** @type {HTMLDivElement} */ (document.getElementById("empty"));
const backdrop = /** @type {HTMLDivElement} */ (document.getElementById("backdrop"));

/** @type {App[]} */
let apps = [];
/** @type {App[]} */
let shown = [];
let selected = 0;

/** @param {string} msg */
function post(msg) {
  // @ts-ignore: injected by WebKit for the handler kompact-shell registers.
  const h = window.webkit && window.webkit.messageHandlers && window.webkit.messageHandlers.kompact;
  if (h) h.postMessage(msg);
  else console.log(`launcher: no kompact message handler (${msg})`);
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

/** A stable colour per app for its letter badge. @param {string} s */
function hue(s) {
  let h = 0;
  for (const c of s) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return h % 360;
}

function render() {
  const q = search.value.trim().toLowerCase();
  shown = apps
    .map((app, i) => ({ app, i, s: score(app, q) }))
    .filter((x) => x.s >= 0)
    .sort((a, b) => a.s - b.s || a.i - b.i)
    .map((x) => x.app);
  selected = Math.min(selected, Math.max(0, shown.length - 1));
  list.replaceChildren(
    ...shown.map((app, i) => {
      const li = document.createElement("li");
      li.setAttribute("role", "option");
      if (i === selected) li.className = "selected";
      const badge = document.createElement("span");
      badge.className = "badge";
      badge.style.background = `hsl(${hue(app.id)} 45% 42%)`;
      badge.textContent = (app.name.trim()[0] || "?").toUpperCase();
      const text = document.createElement("span");
      text.className = "text";
      const name = document.createElement("div");
      name.className = "name";
      name.textContent = app.name;
      const comment = document.createElement("div");
      comment.className = "comment";
      comment.textContent = app.comment;
      text.append(name, comment);
      li.append(badge, text);
      li.addEventListener("click", () => launch(app));
      return li;
    }),
  );
  empty.hidden = shown.length > 0;
}

/** @param {number} i */
function select(i) {
  if (shown.length === 0) return;
  selected = (i + shown.length) % shown.length;
  list.querySelectorAll("li").forEach((li, j) => li.classList.toggle("selected", j === selected));
  list.children[selected]?.scrollIntoView({ block: "nearest" });
}

/** @param {App} app */
function launch(app) {
  console.log(`launcher: launch ${app.id}`);
  post(`launch:${app.id}`);
}

search.addEventListener("input", () => {
  selected = 0;
  render();
});

document.addEventListener("keydown", (e) => {
  switch (e.key) {
    case "Escape":
      e.preventDefault();
      post("close-launcher");
      break;
    case "ArrowDown":
      e.preventDefault();
      select(selected + 1);
      break;
    case "ArrowUp":
      e.preventDefault();
      select(selected - 1);
      break;
    case "PageDown":
      e.preventDefault();
      select(Math.min(selected + 8, shown.length - 1));
      break;
    case "PageUp":
      e.preventDefault();
      select(Math.max(selected - 8, 0));
      break;
    case "Enter":
      e.preventDefault();
      if (shown[selected]) launch(shown[selected]);
      break;
  }
});

// A click outside the card closes the launcher.
backdrop.addEventListener("mousedown", (e) => {
  if (e.target === backdrop) post("close-launcher");
});

// @ts-ignore: our own global, called by kompact-shell.
window.kompactLauncher = {
  /** @param {App[]} items */
  open(items) {
    apps = items;
    search.value = "";
    selected = 0;
    render();
    search.focus();
    console.log(`launcher: open, ${apps.length} apps`);
  },
};
console.log("launcher: page ready");
