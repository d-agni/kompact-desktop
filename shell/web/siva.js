// @ts-check
// 19b: Siva on the desktop (src-tauri/src/assistant/siva.rs). His moods are classes on the SVG
// (`e-<mood>`, siva.css). Calm and Sleepy get one-offs every few seconds (a blink, a breath, a
// glance, a z) and draw nothing in between. A click says hello, a right-click opens his menu,
// a drag moves him (the shell places the surface on release), Do Not Disturb puts him to sleep.
// 19c: the click opens a box to type a request (`assistant_ask`); replies come with Undo or a
// Confirm/Cancel card.
"use strict";

// @ts-ignore: injected by Tauri (app.withGlobalTauri).
const tauri = window.__TAURI__;
/** @param {string} cmd @param {Record<string, unknown>} [args] */
const invoke = (cmd, args) => tauri?.core.invoke(cmd, args).catch((/** @type {unknown} */ e) => console.log(`siva: ${cmd}: ${e}`));

const $ = (/** @type {string} */ id) => /** @type {HTMLElement} */ (document.getElementById(id));
const svg = /** @type {SVGSVGElement} */ (/** @type {unknown} */ ($("figure-svg")));
const sivaEl = $("siva");
const bubble = $("bubble");
const bubbleText = $("bubble-text");
const bubbleButtons = $("bubble-buttons");
const ask = /** @type {HTMLFormElement} */ ($("ask"));
const askInput = /** @type {HTMLInputElement} */ ($("ask-input"));
const menu = $("menu");

/** @typedef {"idle" | "listening" | "thinking" | "speaking" | "happy" | "concerned" | "sorry" | "sleepy"} Mood */

/** What he says in the mood demo. @type {[Mood, string][]} */
const DEMO = [
  ["idle", "Vanakkam! I'm Siva. These are my moods."],
  ["listening", "Curious: I'm listening to you…"],
  ["thinking", "Focused: working it out…"],
  ["speaking", "Talking: here's what I did."],
  ["happy", "Happy: that went well!"],
  ["concerned", "Careful: I ask before anything I can't undo."],
  ["sorry", "Sorry: something didn't work."],
  ["sleepy", "Sleepy: Do Not Disturb is on."],
];

let dnd = false;
let less = false;
/** A mood shown for a while (a greeting, the demo) over the resting one. @type {Mood | null} */
let passing = null;
let passTimer = 0;
let demoTimer = 0;

/** The mood when nothing is going on. @returns {Mood} */
const resting = () => (dnd ? "sleepy" : "idle");

/** @param {Mood} m */
function setMood(m) {
  svg.setAttribute("class", `e-${m}`);
}

function refresh() {
  setMood(passing ?? resting());
}

/** Say `text` with `mood` for `ms`, then back to resting; `buttons` under it, `after` once it
 * goes. @param {string} text @param {Mood} mood @param {number} ms
 * @param {{label: string, cls?: string, click: () => void}[]} [buttons] @param {() => void} [after] */
function say(text, mood, ms, buttons, after) {
  menu.hidden = true;
  closeAsk();
  bubbleText.textContent = text;
  bubbleButtons.replaceChildren(
    ...(buttons || []).map((b) => {
      const el = document.createElement("button");
      el.type = "button";
      el.textContent = b.label;
      if (b.cls) el.className = b.cls;
      el.addEventListener("click", () => {
        clearTimeout(passTimer);
        b.click();
      });
      return el;
    }),
  );
  bubbleButtons.hidden = !buttons?.length;
  bubble.hidden = false;
  passing = mood;
  refresh();
  updateInput();
  clearTimeout(passTimer);
  passTimer = setTimeout(() => {
    passing = null;
    bubble.hidden = true;
    refresh();
    updateInput();
    after?.();
  }, ms);
}

// ---- 19c: typing a request ----

function openAsk() {
  clearTimeout(passTimer);
  menu.hidden = true;
  bubbleText.textContent = "";
  bubbleButtons.hidden = true;
  ask.hidden = false;
  bubble.hidden = false;
  passing = "listening";
  refresh();
  updateInput();
  // His surface takes the keyboard only while the box is open.
  invoke("siva_keyboard", { on: true }).then(() => askInput.focus());
}

function closeAsk() {
  if (ask.hidden) return;
  ask.hidden = true;
  askInput.value = "";
  invoke("siva_keyboard", { on: false });
}

askInput.addEventListener("keydown", (e) => {
  if (e.key === "Escape") {
    closeAsk();
    bubble.hidden = true;
    passing = null;
    refresh();
    updateInput();
  }
});

ask.addEventListener("submit", async (e) => {
  e.preventDefault();
  const text = askInput.value.trim();
  if (!text) return;
  closeAsk();
  bubbleText.textContent = "…";
  passing = "thinking";
  refresh();
  /** @type {{text: string, mood: Mood, undo: number | null, confirm: {id: number, text: string, button: string} | null} | undefined} */
  const r = await invoke("assistant_ask", { text });
  showReply(r);
});

/** @param {{text: string, mood: Mood, undo: number | null, confirm: {id: number, text: string, button: string} | null} | undefined} r */
function showReply(r) {
  if (!r) return say("Sorry, something went wrong inside me.", "sorry", 4000);
  const card = r.confirm;
  if (card) {
    // Only a click (or Enter on the button) says yes; left alone, it's a no.
    let answered = false;
    const answer = async (/** @type {boolean} */ yes) => {
      answered = true;
      passing = yes ? "thinking" : null;
      refresh();
      showReply(await invoke("assistant_answer", { id: card.id, confirm: yes }));
    };
    say(card.text, "concerned", 30000, [
      { label: "Cancel", click: () => answer(false) },
      { label: card.button, cls: "danger", click: () => answer(true) },
    ], () => {
      if (!answered) invoke("assistant_answer", { id: card.id, confirm: false });
    });
    return;
  }
  const undoId = r.undo;
  const buttons = undoId == null ? [] : [{ label: "Undo", click: async () => showReply(await invoke("assistant_undo", { id: undoId })) }];
  const long = r.text.length > 90 ? 9000 : 6000;
  say(r.text, r.mood, undoId == null ? long : long + 4000, buttons);
}

// ---- one-offs while resting ----

/** @param {string} cls @param {number} ms */
function oneOff(cls, ms) {
  svg.classList.add(cls);
  setTimeout(() => svg.classList.remove(cls), ms);
}

function rest() {
  // Every 6–12 s, mostly a blink (a quarter of a second); nothing while hidden, moving less, or
  // in another mood. Each frame drawn is one kompact composites, so on the GT 710 resting stays
  // at a few frames a second on average (kompact's `uploads:` line, `kompact-siva`).
  setTimeout(rest, 6000 + Math.random() * 6000);
  if (less || passing || document.hidden) return;
  const r = Math.random();
  if (resting() === "sleepy") {
    if (r < 0.5) oneOff("do-z", 2100);
    return;
  }
  oneOff("do-blink", 240);
  if (r < 0.15) oneOff("do-breathe", 2100);
  else if (r < 0.23) oneOff("do-look-l", 1700);
  else if (r < 0.31) oneOff("do-look-r", 1700);
  else if (r < 0.35) oneOff("do-flow", 1300);
}
setTimeout(rest, 2000);

// ---- the pointer takes only what's drawn ----

function updateInput() {
  // After layout, so hidden ones have no box.
  requestAnimationFrame(() => {
    const rects = [sivaEl, bubble, menu]
      .filter((el) => !el.hidden)
      .map((el) => el.getBoundingClientRect())
      .filter((r) => r.width > 0 && r.height > 0)
      .map((r) => [r.x, r.y, r.width, r.height]);
    invoke("siva_input", { rects });
  });
}

// ---- click, drag, right-click ----

/** Where the button went down (surface px), and whether it has moved enough to be a drag. */
let press = /** @type {{x: number, y: number, dragging: boolean} | null} */ (null);

sivaEl.addEventListener("pointerdown", (e) => {
  if (e.button !== 0) return;
  press = { x: e.clientX, y: e.clientY, dragging: false };
  sivaEl.setPointerCapture(e.pointerId);
});
sivaEl.addEventListener("pointermove", (e) => {
  if (!press || press.dragging) return;
  if (Math.hypot(e.clientX - press.x, e.clientY - press.y) > 6) {
    press.dragging = true;
    sivaEl.classList.add("carried");
    menu.hidden = true;
    bubble.hidden = true;
    passing = "happy";
    refresh();
  }
});
sivaEl.addEventListener("pointerup", async (e) => {
  const p = press;
  press = null;
  if (!p) return;
  if (!p.dragging) {
    // A click opens the typing box; a click while it's open closes it.
    if (!ask.hidden) {
      closeAsk();
      bubble.hidden = true;
      passing = null;
      refresh();
      updateInput();
    } else {
      openAsk();
    }
    return;
  }
  sivaEl.classList.remove("carried");
  passing = null;
  refresh();
  const left = await invoke("siva_drop", { gx: p.x, gy: p.y, rx: e.clientX, ry: e.clientY });
  if (typeof left === "boolean") side(left);
});
sivaEl.addEventListener("contextmenu", (e) => {
  e.preventDefault();
  openMenu();
});

/** @param {boolean} left */
function side(left) {
  document.body.classList.toggle("left", left);
  document.body.classList.toggle("right", !left);
  updateInput();
}

// ---- his menu ----

let menuTimer = 0;

function openMenu() {
  closeAsk();
  bubble.hidden = true;
  $("menu-motion").textContent = less ? "More movement" : "Less movement";
  menu.hidden = false;
  updateInput();
  // No keyboard here: it closes once the pointer has been away for a moment.
  clearTimeout(menuTimer);
  menuTimer = setTimeout(closeMenu, 6000);
}

function closeMenu() {
  menu.hidden = true;
  updateInput();
}

menu.addEventListener("pointerenter", () => clearTimeout(menuTimer));
menu.addEventListener("pointerleave", () => {
  clearTimeout(menuTimer);
  menuTimer = setTimeout(closeMenu, 1500);
});
menu.addEventListener("click", async (e) => {
  const b = /** @type {HTMLElement} */ (e.target).closest("button");
  if (!b) return;
  closeMenu();
  if (b.dataset.corner) {
    const left = await invoke("siva_corner", { corner: b.dataset.corner });
    if (typeof left === "boolean") side(left);
  } else if (b.dataset.act === "moods") {
    demo(0);
  } else if (b.dataset.act === "motion") {
    invoke("siva_motion", { less: !less });
  } else if (b.dataset.act === "hide") {
    say("Bye for now! Bring me back from the Control Center.", "happy", 1600);
    setTimeout(() => invoke("siva_show", { on: false }), 1700);
  }
});

/** @param {number} i */
function demo(i) {
  clearTimeout(demoTimer);
  if (i >= DEMO.length) return;
  const [mood, text] = DEMO[i];
  say(text, mood, 2900);
  demoTimer = setTimeout(() => demo(i + 1), 3000);
}

// ---- settings and Do Not Disturb ----

/** @param {{shown: boolean, less_motion: boolean, left: boolean} | undefined} s */
function apply(s) {
  if (!s) return;
  less = s.less_motion;
  document.body.classList.toggle("less", less);
  side(s.left);
}

invoke("siva_state").then(apply);
invoke("notify_prefs").then((/** @type {{dnd?: boolean} | undefined} */ p) => {
  dnd = !!p?.dnd;
  refresh();
});
if (tauri) {
  tauri.event.listen("kompact-siva", (/** @type {{payload: any}} */ e) => apply(e.payload));
  tauri.event.listen("kompact-notify-prefs", (/** @type {{payload: {dnd?: boolean}}} */ e) => {
    dnd = !!e.payload?.dnd;
    refresh();
  });
}
document.addEventListener("visibilitychange", () => {
  if (!document.hidden) {
    refresh();
    updateInput();
  }
});
window.addEventListener("resize", updateInput);
refresh();
updateInput();
console.log("siva: page up");
