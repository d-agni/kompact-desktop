// @ts-check
// 19h: the assistant's HUD (src-tauri/src/assistant/hud.rs). The menu bar's assistant item opens
// it listening (`assistant_listen`: it knows when you've finished speaking); what was heard shows
// at once, then the reply, with Undo or a Confirm/Cancel card (only a click confirms). It goes
// away by itself a few seconds after a reply (not while the pointer is on it), or with ✕.
// The wave and the ring move only while it listens, works or (19h-2) speaks its reply.
"use strict";

// @ts-ignore: injected by Tauri (app.withGlobalTauri).
const tauri = window.__TAURI__;
/** @param {string} cmd @param {Record<string, unknown>} [args] @returns {Promise<any>} */
const invoke = (cmd, args) =>
  tauri ? tauri.core.invoke(cmd, args).catch((/** @type {unknown} */ e) => console.log(`hud: ${cmd}: ${e}`)) : Promise.resolve(undefined);

const $ = (/** @type {string} */ id) => /** @type {HTMLElement} */ (document.getElementById(id));
const hud = $("hud");
const statusEl = $("status");
const heardEl = $("heard");
const replyEl = $("reply");
const buttonsEl = $("buttons");
const micEl = $("mic");
const planEl = $("plan");
const planSum = $("plan-sum");
const countEl = $("count");
const recentEl = $("recent");
const recentList = $("recent-list");
const typeForm = /** @type {HTMLFormElement} */ ($("type"));
const typeInput = /** @type {HTMLInputElement} */ ($("type-input"));
const canvas = /** @type {HTMLCanvasElement} */ ($("wave"));
const ctx = /** @type {CanvasRenderingContext2D} */ (canvas.getContext("2d"));

/** @typedef {"hidden" | "waking" | "listening" | "thinking" | "reply" | "typing"} State */
/** @typedef {{text: string, state: string}} PlanItem */
/** @typedef {{text: string, mood: string, undo: number | null, confirm: {id: number, text: string, button: string} | null, plan?: PlanItem[] | null, choices?: {label: string, request: string}[]}} Reply */

/** @type {State} */
let state = "hidden";
let hideTimer = 0;
let hovering = false;
/** The confirm card on screen (its id), to cancel it if the HUD is closed. */
let card = 0;
let typing = false;
/** 19j: what was heard for the request on screen (before Siva's own corrections), for ✎. */
let lastRaw = "";
/** 19j: typing to put `lastRaw` right: the pair is learnt. */
let fixing = "";
/** 19h-2: its reply is being said aloud (the wave follows the voice). */
let speaking = false;

// ---- 20c: tasks. Each request is a task; the newest one ("focused") fills the HUD, the others
// run on in the strip above the buttons, each with its own Stop, and say when they're done.

/** @typedef {{id: number, text: string, running: boolean, line: string, reply: Reply | null}} Task */
/** @type {Map<number, Task>} */
const tasks = new Map();
let taskSeq = (Date.now() % 1000000) * 1000;
/** The task the HUD shows. */
let focused = 0;
const tasksEl = $("tasks");

/** 20c-2: finished tasks leave (their answer was seen, and Recent keeps it); running ones stay.
 * @param {number} [keep] a task to keep anyway */
function dropFinished(keep) {
  for (const t of [...tasks.values()]) if (!t.running && t.id !== keep) tasks.delete(t.id);
}

/** A new task, now the one shown. @param {string} text */
function newTask(text) {
  dropFinished();
  if (!recentEl.hidden) showRecent(false, true);
  const id = ++taskSeq;
  tasks.set(id, { id, text, running: true, line: "", reply: null });
  focused = id;
  renderTasks();
  return id;
}

function running() {
  return [...tasks.values()].filter((t) => t.running).length;
}

/** Task `id` is done with `r`: shown if it's the one on screen; else in the strip (and the HUD
 * comes back for a card that needs you). @param {number} id @param {Reply | undefined} r */
function finishTask(id, r) {
  const t = tasks.get(id);
  if (!t) return;
  t.running = false;
  t.reply = r || null;
  remember(t.text, r);
  if (id === focused) {
    showReply(r);
  } else {
    t.line = r?.text || "";
    if (r?.confirm) {
      // It needs your OK: bring it to the front of the HUD.
      setMini(false);
      focusTask(id);
      invoke("hud_show");
    }
    setTimeout(() => {
      if (id !== focused) {
        tasks.delete(id);
        renderTasks();
      }
    }, 8000);
  }
  renderTasks();
}

/** Show task `id` (a finished one from the strip) in the HUD. @param {number} id */
function focusTask(id) {
  const t = tasks.get(id);
  if (!t) return;
  dropFinished(id);
  focused = id;
  reset();
  heardEl.textContent = `“${t.text}”`;
  if (t.running) setState("thinking", "WORKING");
  else showReply(t.reply || undefined);
  renderTasks();
}

/** The other tasks: running ones with their step and Stop, finished ones with their answer. */
function renderTasks() {
  const others = [...tasks.values()].filter((t) => t.id !== focused);
  tasksEl.replaceChildren(
    ...others.map((t) => {
      const row = document.createElement("div");
      row.className = `task ${t.running ? "running" : t.reply?.mood === "sorry" ? "failed" : "done"}`;
      const what = Object.assign(document.createElement("span"), { className: "task-text", textContent: t.text });
      const line = Object.assign(document.createElement("span"), { className: "task-line", textContent: t.line || (t.running ? "working…" : "") });
      row.append(what, line);
      if (t.running) {
        row.append(button("Stop", "small", () => invoke("assistant_stop", { task: t.id })));
      } else {
        row.addEventListener("click", () => focusTask(t.id));
        row.title = "Show this one";
      }
      return row;
    }),
  );
  const more = others.filter((t) => t.running).length;
  countEl.textContent = more ? `+${more} RUNNING` : "";
  updateInput();
}

/** @param {State} s @param {string} status */
function setState(s, status) {
  state = s;
  const body = document.body.classList;
  body.remove(...[...body].filter((c) => c.startsWith("s-")));
  body.add(`s-${s}`);
  body.toggle("talking", speaking);
  body.toggle("planning", planEl.childElementCount > 0);
  body.toggle("can-fix", !!lastRaw);
  hud.hidden = s === "hidden";
  statusEl.textContent = status;
  animate();
  updateInput();
}

function clearHide() {
  clearTimeout(hideTimer);
  hideTimer = 0;
}

/** Close in `ms`, unless the pointer is on it, it's speaking, or tasks still run (then later);
 * never while it's kept open (20c-2). @param {number} ms */
function hideIn(ms) {
  clearHide();
  if (pinned) return;
  hideTimer = setTimeout(() => (hovering || speaking || running() > 0 ? hideIn(2000) : close()), ms);
}

function keyboard(/** @type {boolean} */ on) {
  if (typing === on) return Promise.resolve();
  typing = on;
  return invoke("hud_keyboard", { on });
}

/** 19e: the plan's lines. @param {PlanItem[] | null | undefined} items */
function renderPlan(items) {
  planEl.replaceChildren(
    ...(items || []).map((it) => {
      const li = document.createElement("li");
      li.className = it.state;
      li.textContent = it.text;
      return li;
    }),
  );
  document.body.classList.toggle("planning", planEl.childElementCount > 0);
  document.body.classList.remove("folded");
  planSum.hidden = true;
  updateInput();
}

/** 20c-2: a finished plan folds into one line ("✓ 5 steps done"); click it to see the steps. */
function foldPlan() {
  const items = [...planEl.children];
  if (!items.length) return;
  const n = (/** @type {string} */ c) => items.filter((li) => li.className === c).length;
  const done = n("done");
  const bad = n("failed") + n("stopped");
  planSum.textContent = done === items.length ? `✓ ${done} step${done === 1 ? "" : "s"} done` : `${done} of ${items.length} steps done${bad ? ` · ${bad} didn't` : ""}`;
  planSum.hidden = false;
  planSum.setAttribute("aria-expanded", "false");
  document.body.classList.add("folded");
}
planSum.addEventListener("click", () => {
  const open = document.body.classList.toggle("folded") === false;
  planSum.setAttribute("aria-expanded", String(open));
  clearHide();
  hideIn(8000);
  updateInput();
});

/** The main area, empty (the strip stays). */
function reset() {
  clearHide();
  renderPlan(null);
  heardEl.textContent = "";
  replyEl.textContent = "";
  buttonsEl.replaceChildren();
  typeForm.hidden = true;
  delete document.body.dataset.mood;
  card = 0;
}

async function listen() {
  setMini(false);
  void keyboard(false);
  reset();
  lastRaw = "";
  fixing = "";
  micEl.textContent = "";
  const id = newTask("…");
  setState("waking", "WAKING THE MIC · WAIT FOR THE CHIME");
  const r = await invoke("assistant_listen", { task: id });
  finishTask(id, r);
}

/** @param {string} [prefill] what to start from @param {string} [fix] what was heard, to learn from */
async function openType(prefill, fix) {
  setMini(false);
  invoke("voice_cancel");
  reset();
  lastRaw = "";
  fixing = fix || "";
  typeInput.value = prefill || "";
  typeForm.hidden = false;
  setState("typing", fixing ? "WHAT DID YOU SAY? I'LL REMEMBER" : "TYPE TO ME");
  await keyboard(true);
  if (state === "typing") typeInput.focus();
}

typeForm.addEventListener("submit", (e) => {
  e.preventDefault();
  const text = typeInput.value.trim();
  if (!text) return;
  const heard = fixing || null;
  fixing = "";
  void askText(text, heard);
});

/** A request as if typed (the Type box, or a question's button, 20a). @param {string} text
 * @param {string | null} [heard] what was heard, when this puts it right (19j) */
async function askText(text, heard = null) {
  void keyboard(false);
  typeForm.hidden = true;
  reset();
  lastRaw = "";
  heardEl.textContent = `“${text}”`;
  const id = newTask(text);
  setState("thinking", "WORKING");
  const r = await invoke("assistant_ask", { text, heard, task: id });
  finishTask(id, r);
}
typeInput.addEventListener("keydown", (e) => {
  if (e.key === "Escape") close();
});

/** @param {Reply | undefined} r */
function showReply(r) {
  if (!r) r = { text: "Sorry, something went wrong inside me.", mood: "sorry", undo: null, confirm: null };
  if (r.mood === "idle" && !r.text) {
    // Nothing to show (a cancelled listen): close, unless other tasks still run.
    if (running() > 0) {
      setState("reply", "");
      return;
    }
    return close();
  }
  document.body.dataset.mood = r.mood;
  replyEl.textContent = r.text;
  if (r.plan) renderPlan(r.plan);
  if (!r.confirm) foldPlan();
  buttonsEl.replaceChildren();
  card = 0;
  const c = r.confirm;
  if (c) {
    card = c.id;
    setMini(false);
    buttonsEl.append(
      button("Cancel", "", () => answer(c.id, false)),
      button(c.button, "danger", () => answer(c.id, true)),
    );
    setState("reply", STATUS.concerned);
    // Left alone, it's a no.
    clearHide();
    hideTimer = setTimeout(() => {
      if (card === c.id) answer(c.id, false);
    }, 30000);
    return;
  }
  // 20a: a question's answers ("What should I do with Firefox?").
  const choices = r.choices || [];
  if (choices.length) {
    setMini(false);
    buttonsEl.append(...choices.map((c) => button(c.label, "", () => void askText(c.request))));
    setState("reply", "");
    hideIn(15000);
    return;
  }
  const undoId = r.undo;
  if (undoId != null) {
    buttonsEl.append(
      button("Undo", "", async () => {
        const id = newTask("Undo");
        reset();
        setState("thinking", "UNDOING");
        const u = await invoke("assistant_undo", { id: undoId, task: id });
        finishTask(id, u);
      }),
    );
  }
  setState("reply", STATUS[r.mood] ?? "");
  hideIn((r.text.length > 90 ? 9000 : 6000) + (undoId != null ? 3000 : 0));
}

/** The card's answer, as the same task going on. @param {number} cardId @param {boolean} yes */
async function answer(cardId, yes) {
  const id = focused;
  const t = tasks.get(id);
  if (t) t.running = true;
  card = 0;
  setState("thinking", yes ? "WORKING" : "CANCELLING");
  const r = await invoke("assistant_answer", { id: cardId, confirm: yes, task: id });
  finishTask(id, r);
}

/** @param {string} label @param {string} cls @param {() => void} click */
function button(label, cls, click) {
  const b = document.createElement("button");
  b.type = "button";
  b.textContent = label;
  if (cls) b.className = cls;
  b.addEventListener("click", (e) => {
    e.stopPropagation();
    clearHide();
    click();
  });
  return b;
}

/** Hide the HUD. Running tasks go on (and say when they're done); a card on screen is a no. */
function close() {
  clearHide();
  if (card) invoke("assistant_answer", { id: card, confirm: false, task: focused });
  card = 0;
  void keyboard(false);
  setState("hidden", "");
  invoke("hud_hide");
}

$("close").addEventListener("click", close);

// ---- 20c-2: keep open, make small, recent ----

/** Kept open: it doesn't close by itself (remembered for next time). */
let pinned = false;
try {
  pinned = localStorage.getItem("hud-pinned") === "1";
} catch {}
const pinBtn = $("pin");
function showPin() {
  pinBtn.setAttribute("aria-pressed", String(pinned));
  pinBtn.title = pinned ? "Close by itself again" : "Keep open";
  document.body.classList.toggle("pinned", pinned);
}
showPin();
pinBtn.addEventListener("click", () => {
  pinned = !pinned;
  try {
    localStorage.setItem("hud-pinned", pinned ? "1" : "0");
  } catch {}
  showPin();
  if (pinned) clearHide();
  else if (state === "reply" && !card) hideIn(4000);
});

/** Small: only the top bar (status, the wave, how many run). It grows again for a new request,
 * a question or a card. @param {boolean} on */
function setMini(on) {
  document.body.classList.toggle("mini", on);
  $("mini").setAttribute("aria-pressed", String(on));
  $("mini").title = on ? "Make big" : "Make small";
  updateInput();
}
$("mini").addEventListener("click", () => setMini(!document.body.classList.contains("mini")));
hud.querySelector(".top")?.addEventListener("click", (e) => {
  if (document.body.classList.contains("mini") && !(/** @type {HTMLElement} */ (e.target).closest("button"))) setMini(false);
});

/** @typedef {{text: string, answer: string, mood: string}} Past */
/** The last requests and their answers, newest first (this session only; Settings → Assistant
 * keeps the full activity). @type {Past[]} */
const recent = [];

/** @param {string} text @param {Reply | undefined} r */
function remember(text, r) {
  if (!text || text === "…" || text === "Undo" || !r || (r.mood === "idle" && !r.text)) return;
  const i = recent.findIndex((p) => p.text.toLowerCase() === text.toLowerCase());
  if (i >= 0) recent.splice(i, 1);
  recent.unshift({ text, answer: r.text, mood: r.mood });
  recent.length = Math.min(recent.length, 8);
  if (!recentEl.hidden) renderRecent();
}

function renderRecent() {
  recentList.replaceChildren(
    ...(recent.length
      ? recent.map((p) => {
          const li = document.createElement("li");
          const b = Object.assign(document.createElement("button"), { type: "button", title: "Ask again" });
          b.className = p.mood === "sorry" ? "failed" : "";
          b.append(
            Object.assign(document.createElement("span"), { className: "r-text", textContent: p.text }),
            Object.assign(document.createElement("span"), { className: "r-answer", textContent: p.answer }),
          );
          b.addEventListener("click", () => void askText(p.text));
          li.append(b);
          return li;
        })
      : [Object.assign(document.createElement("li"), { className: "r-none", textContent: "Nothing yet." })]),
  );
}

/** @param {boolean} on @param {boolean} [quiet] a new request closes it: no hide timer then */
function showRecent(on, quiet = false) {
  recentEl.hidden = !on;
  $("recent-btn").setAttribute("aria-expanded", String(on));
  if (on) {
    renderRecent();
    setMini(false);
    clearHide();
  } else if (!quiet && state === "reply" && !card) {
    hideIn(6000);
  }
  updateInput();
}
$("recent-btn").addEventListener("click", () => showRecent(recentEl.hidden));
$("recent-clear").addEventListener("click", () => {
  recent.length = 0;
  renderRecent();
});
$("done").addEventListener("click", () => invoke("voice_stop"));
$("stop").addEventListener("click", () => {
  statusEl.textContent = "STOPPING";
  invoke("assistant_stop", { task: focused });
});
$("speak").addEventListener("click", () => void listen());
$("kbd").addEventListener("click", () => void openType());
$("fix").addEventListener("click", () => {
  const shown = heardEl.textContent.replace(/^“|”$/g, "");
  void openType(shown, lastRaw);
});
hud.addEventListener("pointerenter", () => (hovering = true));
hud.addEventListener("pointerleave", () => (hovering = false));

// ---- the wave ----

let level = 0;
let target = 0;
let phase = 0;
let raf = 0;
const still = matchMedia("(prefers-reduced-motion: reduce)");

function moving() {
  return (state === "waking" || state === "listening" || state === "thinking" || (state === "reply" && speaking)) && !still.matches;
}

function animate() {
  if (moving()) {
    if (!raf) raf = requestAnimationFrame(frame);
  } else {
    if (raf) cancelAnimationFrame(raf);
    raf = 0;
    target = level = 0;
    draw();
  }
}

/** At most 30 frames a second: each one is a commit kompact composites (measured 2026-10-24: ~38
 * a second at the display's rate), and a wave doesn't need more. */
const FRAME_MS = 1000 / 30;
let lastFrame = 0;

/** @param {number} now */
function frame(now) {
  raf = 0;
  if (now - lastFrame >= FRAME_MS - 2) {
    lastFrame = now;
    level += (target - level) * 0.35;
    phase += state === "thinking" ? 0.3 : 0.17;
    draw();
  }
  if (moving()) raf = requestAnimationFrame(frame);
}

/** Three sine waves under one envelope: their height is your voice (or a calm swell while it
 * works). */
function draw() {
  const scale = window.devicePixelRatio || 1;
  const w = 300;
  const h = 56;
  if (canvas.width !== w * scale) {
    canvas.width = w * scale;
    canvas.height = h * scale;
  }
  ctx.setTransform(scale, 0, 0, scale, 0, 0);
  ctx.clearRect(0, 0, w, h);
  const accent = getComputedStyle(document.body).getPropertyValue("--accent").trim() || "#5ad8ff";
  const amp =
    state === "thinking"
      ? 7 + 5 * Math.sin(phase * 0.7)
      : state === "listening" || speaking
        ? 2 + level * 24
        : state === "waking"
          ? 1.2 + Math.sin(phase * 2) * 0.6
          : 0.6;
  document.body.style.setProperty("--lvl", state === "listening" || speaking ? level.toFixed(2) : "0");
  for (let i = 0; i < 3; i++) {
    ctx.beginPath();
    ctx.lineWidth = i === 0 ? 2 : 1.2;
    ctx.strokeStyle = accent;
    ctx.globalAlpha = [0.95, 0.5, 0.3][i];
    if (i === 0) {
      ctx.shadowColor = accent;
      ctx.shadowBlur = 8;
    } else {
      ctx.shadowBlur = 0;
    }
    for (let x = 0; x <= w; x += 3) {
      const t = x / w;
      const envelope = Math.sin(Math.PI * t) ** 1.6;
      const y = h / 2 + Math.sin(t * Math.PI * (2.2 + i * 1.3) + phase * (1 + i * 0.45)) * amp * envelope * (1 - i * 0.25);
      if (x === 0) ctx.moveTo(x, y);
      else ctx.lineTo(x, y);
    }
    ctx.stroke();
  }
  ctx.globalAlpha = 1;
  ctx.shadowBlur = 0;
}

// ---- the pointer takes only the HUD ----

function updateInput() {
  requestAnimationFrame(() => {
    const r = hud.getBoundingClientRect();
    const rects = hud.hidden || r.width === 0 ? [] : [[r.x, r.y, r.width, r.height]];
    invoke("hud_input", { rects });
  });
}
new ResizeObserver(updateInput).observe(hud);

// ---- what the shell says ----

if (tauri) {
  tauri.event.listen("kompact-assistant-open", (/** @type {{payload: string}} */ e) => {
    if (e.payload === "type") void openType();
    else void listen();
  });
  tauri.event.listen("kompact-assistant", (/** @type {{payload: {state: string, detail: string}}} */ e) => {
    const { state: s, detail } = e.payload;
    if (s === "speaking" || s === "spoken") {
      speaking = s === "speaking";
      document.body.classList.toggle("talking", speaking);
      animate();
      return;
    }
    if (s === "listening" && state === "waking") micEl.textContent = detail;
    else if (s === "ready" && state === "waking") setState("listening", "LISTENING · SPEAK NOW");
    else if (s === "thinking" && (state === "listening" || state === "waking")) {
      heardEl.textContent = `“${detail}”`;
      lastRaw = /** @type {any} */ (e.payload).raw || detail;
      const t = tasks.get(focused);
      if (t) t.text = detail;
      setState("thinking", "WORKING");
    }
  });
  // 19e: each step of a plan as it goes.
  tauri.event.listen("kompact-assistant-plan", (/** @type {{payload: {task: number, items: PlanItem[]}}} */ e) => {
    const { task, items } = e.payload;
    if (task === focused && state === "thinking") renderPlan(items);
    // 20c: a task in the strip shows the step it's on.
    const t = tasks.get(task);
    if (t && task !== focused) {
      const now = items.find((i) => i.state === "now") || items.find((i) => i.state === "todo");
      const done = items.filter((i) => i.state === "done").length;
      t.line = now ? `${done}/${items.length} · ${now.text}` : `${done}/${items.length}`;
      renderTasks();
    }
  });
  tauri.event.listen("kompact-assistant-level", (/** @type {{payload: number}} */ e) => {
    target = Math.max(0, Math.min(1, Number(e.payload) || 0));
  });
}
draw();
console.log("hud: page up");
