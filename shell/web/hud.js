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
const typeForm = /** @type {HTMLFormElement} */ ($("type"));
const typeInput = /** @type {HTMLInputElement} */ ($("type-input"));
const canvas = /** @type {HTMLCanvasElement} */ ($("wave"));
const ctx = /** @type {CanvasRenderingContext2D} */ (canvas.getContext("2d"));

/** @typedef {"hidden" | "waking" | "listening" | "thinking" | "reply" | "typing"} State */
/** @typedef {{text: string, state: string}} PlanItem */
/** @typedef {{text: string, mood: string, undo: number | null, confirm: {id: number, text: string, button: string} | null, plan?: PlanItem[] | null}} Reply */

/** @type {State} */
let state = "hidden";
/** Bumped by each new listen, typing or close, so a late answer for an old one is dropped. */
let session = 0;
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

const STATUS = /** @type {Record<string, string>} */ ({ speaking: "", happy: "DONE", sorry: "COULDN'T DO THAT", concerned: "NEEDS YOUR OK", idle: "" });

/** @param {State} s @param {string} status */
function setState(s, status) {
  state = s;
  document.body.className = `s-${s}${speaking ? " talking" : ""}${planEl.childElementCount ? " planning" : ""}${lastRaw ? " can-fix" : ""}`;
  hud.hidden = s === "hidden";
  statusEl.textContent = status;
  animate();
  updateInput();
}

function clearHide() {
  clearTimeout(hideTimer);
  hideTimer = 0;
}

/** Close in `ms`, unless the pointer is on it (then once it leaves). @param {number} ms */
function hideIn(ms) {
  clearHide();
  hideTimer = setTimeout(() => (hovering || speaking ? hideIn(2000) : close()), ms);
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
  updateInput();
}

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
  const my = ++session;
  void keyboard(false);
  reset();
  lastRaw = "";
  fixing = "";
  micEl.textContent = "";
  setState("waking", "WAKING THE MIC · WAIT FOR THE CHIME");
  const r = await invoke("assistant_listen");
  if (my === session) showReply(r);
}

/** @param {string} [prefill] what to start from @param {string} [fix] what was heard, to learn from */
async function openType(prefill, fix) {
  const my = ++session;
  invoke("voice_cancel");
  reset();
  lastRaw = "";
  fixing = fix || "";
  typeInput.value = prefill || "";
  typeForm.hidden = false;
  setState("typing", fixing ? "WHAT DID YOU SAY? I'LL REMEMBER" : "TYPE TO ME");
  await keyboard(true);
  if (my === session) typeInput.focus();
}

typeForm.addEventListener("submit", async (e) => {
  e.preventDefault();
  const text = typeInput.value.trim();
  if (!text) return;
  const my = ++session;
  void keyboard(false);
  typeForm.hidden = true;
  heardEl.textContent = `“${text}”`;
  setState("thinking", "WORKING");
  const heard = fixing || null;
  fixing = "";
  const r = await invoke("assistant_ask", { text, heard });
  if (my === session) showReply(r);
});
typeInput.addEventListener("keydown", (e) => {
  if (e.key === "Escape") close();
});

/** @param {Reply | undefined} r */
function showReply(r) {
  if (!r) r = { text: "Sorry, something went wrong inside me.", mood: "sorry", undo: null, confirm: null };
  if (r.mood === "idle" && !r.text) return close();
  document.body.dataset.mood = r.mood;
  replyEl.textContent = r.text;
  if (r.plan) renderPlan(r.plan);
  buttonsEl.replaceChildren();
  card = 0;
  const c = r.confirm;
  if (c) {
    card = c.id;
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
  const undoId = r.undo;
  if (undoId != null) {
    buttonsEl.append(
      button("Undo", "", async () => {
        const my = ++session;
        setState("thinking", "UNDOING");
        const u = await invoke("assistant_undo", { id: undoId });
        if (my === session) showReply(u);
      }),
    );
  }
  setState("reply", STATUS[r.mood] ?? "");
  hideIn((r.text.length > 90 ? 9000 : 6000) + (undoId != null ? 3000 : 0) + planEl.childElementCount * 600);
}

/** @param {number} id @param {boolean} yes */
async function answer(id, yes) {
  const my = ++session;
  card = 0;
  setState("thinking", yes ? "WORKING" : "CANCELLING");
  const r = await invoke("assistant_answer", { id, confirm: yes });
  if (my === session) showReply(r);
}

/** @param {string} label @param {string} cls @param {() => void} click */
function button(label, cls, click) {
  const b = document.createElement("button");
  b.type = "button";
  b.textContent = label;
  if (cls) b.className = cls;
  b.addEventListener("click", () => {
    clearHide();
    click();
  });
  return b;
}

function close() {
  session++;
  clearHide();
  if (card) invoke("assistant_answer", { id: card, confirm: false });
  card = 0;
  void keyboard(false);
  setState("hidden", "");
  invoke("hud_hide");
}

$("close").addEventListener("click", close);
$("done").addEventListener("click", () => invoke("voice_stop"));
$("stop").addEventListener("click", () => {
  statusEl.textContent = "STOPPING";
  invoke("assistant_stop");
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
      setState("thinking", "WORKING");
    }
  });
  // 19e: each step of a plan as it goes.
  tauri.event.listen("kompact-assistant-plan", (/** @type {{payload: PlanItem[]}} */ e) => {
    if (state === "thinking") renderPlan(e.payload);
  });
  tauri.event.listen("kompact-assistant-level", (/** @type {{payload: number}} */ e) => {
    target = Math.max(0, Math.min(1, Number(e.payload) || 0));
  });
}
draw();
console.log("hud: page up");
