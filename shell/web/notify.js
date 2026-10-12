// @ts-check
// 8c: the notification banners. The shell's D-Bus server (src-tauri/src/notify.rs) sends
// `kompact-notify` events: {kind: "show", ...banner} or {kind: "close", id}. Each banner leaves
// after its timeout (paused while the pointer is on it), on a click (the app's "default"
// action), on its close button or an action button, or when the app closes it; the shell is
// told why (`notify_closed`), and the window is sized to what's left (`notify_size`).

const tauri = /** @type {any} */ (window).__TAURI__;
const stack = /** @type {HTMLElement} */ (document.getElementById("stack"));
/** At most this many on screen; the oldest goes first. */
const MAX = 4;
/** Reasons, as in the spec. */
const EXPIRED = 1, DISMISSED = 2, CLOSED = 3;

/**
 * @typedef {{id: number, app: string, summary: string, body: string, actions: [string, string][],
 *   expire_ms: number | null, critical: boolean, icon: boolean}} Banner
 * @typedef {{banner: Banner, el: HTMLElement, timer: number | undefined, left: number | null, since: number}} Shown
 */
/** @type {Map<number, Shown>} */
const shown = new Map();

/** @param {string} cmd @param {Record<string, unknown>} [args] */
function invoke(cmd, args) {
  if (!tauri) return Promise.resolve(undefined);
  return tauri.core.invoke(cmd, args).catch((/** @type {unknown} */ e) => console.log(`notify: ${cmd} failed: ${e}`));
}

function resize() {
  // The window follows the cards' height (0 hides it). Not requestAnimationFrame: WebKit
  // doesn't run it while the window is hidden, and the first banner is what shows it.
  setTimeout(() => {
    const h = shown.size === 0 ? 0 : Math.ceil(stack.getBoundingClientRect().height);
    invoke("notify_size", { height: h });
  }, 0);
}

/** A stable colour per app for its letter badge (as in the dock). @param {string} s */
function hue(s) {
  let h = 0;
  for (const c of s) h = (h * 31 + c.charCodeAt(0)) % 360;
  return h;
}

/** @param {Banner} b */
function iconFor(b) {
  const badge = () => {
    const d = document.createElement("div");
    d.className = "icon badge";
    const h = hue(b.app || b.summary);
    d.style.background = `linear-gradient(160deg, hsl(${h} 55% 55%), hsl(${h} 55% 38%))`;
    d.textContent = (b.app || b.summary || "?").trim().charAt(0).toUpperCase();
    return d;
  };
  if (!b.icon) return badge();
  const img = document.createElement("img");
  img.className = "icon";
  img.alt = "";
  img.src = `kompact-icon://localhost/notify-${b.id}`;
  img.addEventListener("error", () => img.replaceWith(badge()), { once: true });
  return img;
}

/** @param {number} id @param {number} reason */
function remove(id, reason) {
  const s = shown.get(id);
  if (!s) return;
  shown.delete(id);
  clearTimeout(s.timer);
  invoke("notify_closed", { id, reason });
  s.el.classList.add("leaving");
  s.el.addEventListener("animationend", () => {
    s.el.remove();
    resize();
  }, { once: true });
  // In case the animation never runs (hidden window).
  setTimeout(() => { if (s.el.isConnected) { s.el.remove(); resize(); } }, 400);
}

/** @param {Shown} s */
function arm(s) {
  clearTimeout(s.timer);
  if (s.left == null) return;
  s.since = Date.now();
  s.timer = setTimeout(() => remove(s.banner.id, EXPIRED), s.left);
}

/** @param {Banner} b */
function show(b) {
  const old = shown.get(b.id);
  const el = document.createElement("div");
  el.className = "card";
  el.appendChild(iconFor(b));
  const text = document.createElement("div");
  text.className = "text";
  const head = document.createElement("div");
  head.className = "head";
  const app = document.createElement("span");
  app.className = "app";
  app.textContent = b.app || "Notification";
  const when = document.createElement("span");
  when.textContent = "now";
  head.append(app, when);
  const summary = document.createElement("div");
  summary.className = "summary";
  summary.textContent = b.summary;
  text.append(head, summary);
  if (b.body) {
    const body = document.createElement("div");
    body.className = "body";
    body.textContent = b.body;
    text.appendChild(body);
  }
  const buttons = b.actions.filter(([key]) => key !== "default");
  if (buttons.length) {
    const row = document.createElement("div");
    row.className = "actions";
    for (const [key, label] of buttons) {
      const btn = document.createElement("button");
      btn.textContent = label;
      btn.addEventListener("click", (e) => {
        e.stopPropagation();
        invoke("notify_action", { id: b.id, key });
        remove(b.id, DISMISSED);
      });
      row.appendChild(btn);
    }
    text.appendChild(row);
  }
  el.appendChild(text);
  const close = document.createElement("button");
  close.className = "close";
  close.textContent = "×";
  close.title = "Close";
  close.addEventListener("click", (e) => {
    e.stopPropagation();
    remove(b.id, DISMISSED);
  });
  el.appendChild(close);
  el.addEventListener("click", () => {
    if (b.actions.some(([key]) => key === "default")) invoke("notify_action", { id: b.id, key: "default" });
    remove(b.id, DISMISSED);
  });

  /** @type {Shown} */
  const s = { banner: b, el, timer: undefined, left: b.expire_ms, since: Date.now() };
  el.addEventListener("mouseenter", () => {
    if (s.left == null) return;
    clearTimeout(s.timer);
    s.left = Math.max(1500, s.left - (Date.now() - s.since));
  });
  el.addEventListener("mouseleave", () => arm(s));

  if (old) {
    // replaces_id: update in place.
    clearTimeout(old.timer);
    old.el.replaceWith(el);
  } else {
    stack.prepend(el);
  }
  shown.set(b.id, s);
  arm(s);
  // Too many: the oldest leaves (as if it expired).
  const ids = [...shown.keys()];
  while (ids.length > MAX) remove(/** @type {number} */ (ids.shift()), EXPIRED);
  console.log(`notify: showing ${b.id} from ${b.app || "?"} (${b.expire_ms == null ? "until dismissed" : b.expire_ms + " ms"})`);
  resize();
}

if (tauri) {
  tauri.event.listen("kompact-notify", (/** @type {{payload: any}} */ e) => {
    const p = e.payload;
    if (p.kind === "show") show(p);
    else if (p.kind === "close") remove(p.id, CLOSED);
  });
  invoke("notify_ready").then((/** @type {Banner[] | undefined} */ queued) => {
    for (const b of queued || []) show(b);
    console.log(`notify: page ready (${(queued || []).length} waiting)`);
  });
}
