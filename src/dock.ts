import { api, applyTheme, fmtTime, METER_AUDIO, validAccent, type AppSettings, type DockState, type HostInfo, type SeatView } from "./api";
import { iconFor, tintFor } from "./brand";
import { resolveLang, setLang, t } from "./i18n";

const dock = document.getElementById("dock") as HTMLDivElement;
const bar = document.getElementById("bar") as HTMLDivElement;
const collapsed = document.getElementById("collapsed") as HTMLDivElement;

let state: DockState = { phase: { phase: "idle" }, muted: false, expanded: false };
let seats: SeatView[] = [];
let settings: AppSettings | null = null;
let hoverSeat: string | null = null;
const iconCache = new Map<string, string>();

const SVG = {
  collapse: `<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M10 4v6M6.2 6.2a5.4 5.4 0 1 0 7.6 0"/></svg>`,
  mic: `<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><rect x="7" y="2.5" width="6" height="10" rx="3"/><path d="M4.5 9.5a5.5 5.5 0 0 0 11 0M10 15v2.5"/></svg>`,
  micOff: `<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><rect x="7" y="2.5" width="6" height="10" rx="3"/><path d="M4.5 9.5a5.5 5.5 0 0 0 11 0M10 15v2.5M3 3l14 14"/></svg>`,
  // phone.down.fill proportions: a thick receiver band with rounded, drooping ear/mouth pads
  hangup: `<svg viewBox="3 5 18 14" fill="currentColor"><path d="M12 8.4c3.4 0 6.5 1.3 8.6 3.5.8.8.8 2.1 0 3l-1.3 1.3a2 2 0 0 1-2.5.3l-1.4-.9a1.9 1.9 0 0 1-.9-1.8l.1-1c-1.7-.5-3.5-.5-5.2 0l.1 1c.1.7-.3 1.4-.9 1.8l-1.4.9a2 2 0 0 1-2.5-.3l-1.3-1.3c-.8-.9-.8-2.2 0-3C5.5 9.7 8.6 8.4 12 8.4z"/></svg>`,
  eye: `<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M10 4.75c-3.4 0-6.1 2.3-7.4 4.5a1.4 1.4 0 0 0 0 1.5c1.3 2.2 4 4.5 7.4 4.5s6.1-2.3 7.4-4.5a1.4 1.4 0 0 0 0-1.5C16.1 7.05 13.4 4.75 10 4.75z"/><circle cx="10" cy="10" r="2.6"/></svg>`,
  plus: `<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M10 4v12M4 10h12"/></svg>`,
};

// Collapsed glyphs: an SF-style waveform (bars breathe; in a call they talk), or mic.slash when muted.
const DOT_WAVE = `<svg class="dot-glyph" viewBox="0 0 20 20" aria-hidden="true">${[[3, 4.5], [6.5, 8.5], [10, 13], [13.5, 8.5], [17, 4.5]]
  .map(([x, h], i) => `<rect class="b b${i}" x="${x - 0.95}" y="${10 - h / 2}" width="1.9" height="${h}" rx="0.95"/>`).join("")}</svg>`;
const DOT_MUTED = `<svg class="dot-glyph dot-mic" viewBox="0 0 20 20" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="7" y="2.5" width="6" height="10" rx="3"/><path d="M4.5 9.5a5.5 5.5 0 0 0 11 0M10 15v2.5M3 3l14 14"/></svg>`;

async function seatIcon(s: SeatView): Promise<string> {
  const key = s.icon.kind === "custom" ? `custom:${s.id}:${s.icon.path}` : `builtin:${s.icon.key}`;
  if (iconCache.has(key)) return iconCache.get(key)!;
  let html: string;
  if (s.icon.kind === "custom") {
    const data = await api.invoke<string | null>("seat_icon_data", { seatId: s.id });
    html = data ? `<img src="${data}" alt="">` : await iconFor(s.id);
  } else {
    html = await iconFor(s.icon.key);
  }
  iconCache.set(key, html);
  return html;
}

const seatById = (id: string) => seats.find((s) => s.id === id);
const enabledSeats = () => seats.filter((s) => s.enabled);

function currentSeatId(): string | null {
  const p = state.phase;
  return p.phase === "idle" ? null : p.phase === "switching" ? p.from : p.seat;
}

// ───────── layout: the window is resized to exactly the glass shape ─────────
const H = 56;
const BTN = 44, PAD = 6;
let liveScale: number | null = null;
const currentScale = () => Math.min(1.6, Math.max(0.7, liveScale ?? settings?.dock_scale ?? 1));

const vertical = () => dock.dataset.orient === "vertical";

/** Length of the bar along its main axis, measured from the real flex content so text fits. */
function measureBar(s = currentScale()): number {
  const axis = vertical() ? "height" : "width";
  bar.style[axis] = "max-content";
  const pad = Math.round(PAD * s);
  const btn = Math.round(BTN * s);
  const len = Math.ceil(vertical() ? bar.scrollHeight : bar.scrollWidth) + pad * 2;
  bar.style[axis] = "";
  return Math.max(len, btn + pad * 2);
}

let lastSize = { w: 0, h: 0 };
let pendingShrink: number | undefined;
let booting = true;
// macOS: the native window springs between shapes (Rust), and the pill just fills it (CSS 100vw/vh).
const nativeMorph = () => document.documentElement.dataset.morph === "1";
async function fit(form: "dot" | "pet" | "bar") {
  const s = currentScale();
  const hBar = Math.round(H * s);
  let w: number, h: number;
  if (form === "dot") { w = Math.round(36 * s); h = Math.round(36 * s); }
  else if (form === "pet") { w = Math.round(92 * s); h = Math.round(40 * s); }
  else if (vertical()) { w = hBar; h = measureBar(s); }
  else { w = measureBar(s); h = hBar; }
  if (nativeMorph()) { dock.style.removeProperty("width"); dock.style.removeProperty("height"); }
  else { dock.style.width = `${w}px`; dock.style.height = `${h}px`; }
  if (w === lastSize.w && h === lastSize.h) return;   // hover re-layouts must not spam resize_dock
  const grow = w > lastSize.w || h > lastSize.h;
  window.clearTimeout(pendingShrink);
  const minSide = Math.round(36 * 0.7);
  // direct manipulation (edge-resize) and the first layout snap; everything else springs
  const animate = !booting && liveScale === null && !settings?.reduce_motion;
  const resize = () => api.invoke("resize_dock", { width: Math.max(w, minSide), height: Math.max(h, minSide), animate });
  if (api.isTauri || grow || liveScale !== null) await resize();
  else pendingShrink = window.setTimeout(resize, settings?.reduce_motion ? 140 : 480);
  lastSize = { w, h };
}

// ───────── render ─────────
function el<K extends keyof HTMLElementTagNameMap>(tag: K, cls = "", html = ""): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (html) e.innerHTML = html;
  return e;
}

function button(cls: string, svg: string, label: string, onClick: () => void): HTMLButtonElement {
  const b = el("button", `btn ${cls}`, svg);
  b.type = "button";
  b.dataset.k = label;   // stable identity across 1 Hz re-renders, so the lens/hover state carries over
  b.setAttribute("aria-label", label);
  b.addEventListener("click", (e) => { e.stopPropagation(); onClick(); });
  return b;
}

async function seatButton(s: SeatView, small: boolean, active: boolean): Promise<HTMLButtonElement> {
  const b = button(`seat${small ? " sm" : ""}${active ? " active" : ""}${s.enabled ? "" : " disabled"}`, "", s.name, () => {
    // pending = on the Dock as a placeholder; there is nothing to dial yet
    if (!s.enabled || s.status === "pending") return;
    api.invoke("call_seat", { seatId: s.id });
  });
  b.dataset.seat = s.id;
  b.dataset.k = `seat:${s.id}`;
  b.dataset.status = s.status;
  if (settings?.show_names !== "never") b.title = `${s.name} · ${seatHint(s)}`;
  b.style.setProperty("--seat-tint", tintFor(seatBrand(s)));
  const mark = el("div", "mark", await seatIcon(s));
  b.append(mark, el("i", "badge"));
  b.addEventListener("mouseenter", () => { hoverSeat = s.id; renderStatusText(); });
  b.addEventListener("mouseleave", () => { hoverSeat = null; renderStatusText(); });
  return b;
}

let prevForm = ""; let prevIdle = true; let prevPhase = "";
let lastDotGlyph = "";
let statusEl: HTMLDivElement | null = null;
function renderStatusText() {
  if (!statusEl || state.phase.phase !== "idle") return;
  // The reference pill carries no text while idle; seat names are tooltips (see seatButton).
  // "always" keeps the hovered name inline, measured into the pill width so it never overlaps.
  const s = hoverSeat ? seatById(hoverSeat) : null;
  if (s && settings?.show_names === "always") {
    statusEl.innerHTML = `<div class="name">${escapeHtml(s.name)}</div><div class="sub">${seatHint(s)}</div>`;
    statusEl.classList.remove("idle");
  } else {
    statusEl.innerHTML = "";
    statusEl.classList.add("idle");
  }
  if (dock.dataset.form === "bar") void fit("bar");
}

/** The brand a seat wears (added seats get ids like "gemini-app-x1y2"). */
const seatBrand = (s: SeatView) => (s.icon.kind === "builtin" ? s.icon.key : s.id);
const seatHint = (s: SeatView) => t(!s.enabled ? "dock.disabled" : s.status === "pending" ? "dock.pending" : s.status === "needs_auth" ? "dock.needsAuth" : s.silent ? "dock.silent" : "dock.appCall");
const escapeHtml = (s: string) => s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]!));

async function render() {
  const p = state.phase;
  dock.dataset.phase = p.phase;
  dock.dataset.muted = String(state.muted);
  const cur = currentSeatId();
  const curSeat = cur ? seatById(cur) : null;
  dock.style.setProperty("--seat-tint", curSeat ? tintFor(seatBrand(curSeat)) : "");

  const form: "dot" | "pet" | "bar" = state.expanded ? "bar" : settings?.default_form === "pet" ? "pet" : "dot";
  dock.dataset.form = form;
  if ((form === "bar") !== (prevForm === "bar") || (p.phase === "idle") !== prevIdle) touchIdle();   // became idle+expanded → start the countdown
  // controls bloom from the centre when the pill opens or its layout changes phase, not on 1 Hz ticks
  const entering = form === "bar" && (prevForm !== "bar" || p.phase !== prevPhase);
  prevForm = form; prevIdle = p.phase === "idle"; prevPhase = p.phase;

  // collapsed content
  if (form === "pet") {
    const pet = curSeat ?? enabledSeats()[0];
    collapsed.innerHTML = "";
    const av = el("div", "pet-avatar", pet ? await seatIcon(pet) : "");
    const lines = el("div", "pet-lines", "<i></i><i></i>");
    collapsed.append(av, lines);
    lastDotGlyph = "";
    collapsed.title = p.phase === "in_call" ? `${pet?.name} · ${fmtTime(p.elapsed_secs)}` : t("dock.expand");
  } else {
    // only swap the glyph when it changes, so the breathing bars don't restart every tick
    const glyph = state.muted ? DOT_MUTED : DOT_WAVE;
    if (glyph !== lastDotGlyph) { collapsed.innerHTML = glyph; lastDotGlyph = glyph; }
    collapsed.title = p.phase === "in_call" ? t("dock.inCallExpand", { time: fmtTime(p.elapsed_secs) }) : t("dock.expand");
  }

  // folded away, the waveform is gone and so is any reason to hold the microphone
  if (form !== "bar") { stopWave(); await fit(form); kickLens(); return; }
  collapsed.removeAttribute("title");

  // expanded bar
  const frag = document.createDocumentFragment();
  const micBtn = button(state.muted ? "amber" : "", state.muted ? SVG.micOff : SVG.mic, t(state.muted ? "dock.unmute" : "dock.mute"), () => api.invoke("toggle_mute"));
  micBtn.id = "mic";
  micBtn.dataset.k = "mic";

  if (p.phase === "idle") {
    frag.append(button("ghost", SVG.collapse, t("dock.collapse"), () => api.invoke("set_expanded", { expanded: false })));
    statusEl = el("div", "status idle");
    frag.append(statusEl);
    frag.append(el("div", "divider"));
    for (const s of seats.filter((s) => s.enabled || s.status === "pending")) frag.append(await seatButton(s, false, false));
    frag.append(button("ghost add", SVG.plus, t("dock.addAgent"), () => api.invoke("open_settings")));
    frag.append(grip());
    renderStatusTextInto(statusEl);
  } else if (p.phase === "calling" || p.phase === "in_call") {
    const s = seatById(p.seat);
    const av = el("div", "avatar", s ? await seatIcon(s) : "");
    av.dataset.k = "avatar";
    av.title = t("dock.collapse");
    av.addEventListener("click", () => api.invoke("set_expanded", { expanded: false }));
    frag.append(av);
    const st = el("div", "status");
    st.innerHTML = `<div class="name">${escapeHtml(s?.name ?? p.seat)}</div><div class="sub">${p.phase === "calling" ? escapeHtml(t("dock.calling")) : `${escapeHtml(s?.service ?? "")} · ${fmtTime(p.elapsed_secs)}`}</div>`;
    frag.append(st);
    const wave = el("canvas", "wave") as HTMLCanvasElement;
    if (vertical()) { wave.width = 68; wave.height = 168; } else { wave.width = 168; wave.height = 68; }
    frag.append(wave);
    waveCanvas = wave;
    frag.append(micBtn);
    frag.append(button("red", SVG.hangup, t("dock.hangup"), () => api.invoke("hangup")));
    const others = enabledSeats().filter((x) => x.id !== p.seat);
    if (others.length) frag.append(el("div", "divider"));
    for (const o of others) frag.append(await seatButton(o, true, false));
  } else if (p.phase === "switching") {
    const to = seatById(p.to), from = seatById(p.from);
    const av = el("div", "avatar", to ? await seatIcon(to) : "");
    frag.append(av);
    const st = el("div", "status");
    st.innerHTML = `<div class="name">${escapeHtml(t("dock.switchTo", { name: to?.name ?? p.to }))}</div><div class="sub">${escapeHtml(t("dock.hangingUp", { name: from?.name ?? p.from }))}${state.muted ? ` · ${escapeHtml(t("dock.muted"))}` : ""}</div>`;
    frag.append(st);
    const undo = el("button", "undo", escapeHtml(t("dock.undo", { n: p.undo_secs_left })));
    undo.type = "button";
    undo.dataset.k = "undo";
    undo.addEventListener("click", () => api.invoke("undo_switch"));
    frag.append(undo);
    frag.append(micBtn);
    frag.append(button("red", SVG.hangup, t("dock.hangupAll"), () => api.invoke("hangup")));
    frag.append(el("div", "divider"));
    for (const o of enabledSeats()) frag.append(await seatButton(o, true, o.id === p.to));
  } else if (p.phase === "needs_human") {
    const s = seatById(p.seat);
    frag.append(el("div", "avatar", s ? await seatIcon(s) : ""));
    const st = el("div", "status");
    // the state machine's own reason is a key; driver reasons arrive already in the user's language
    const reason = p.reason === "call_timeout" ? t("dock.callTimeout") : p.reason;
    st.innerHTML = `<div class="name">${escapeHtml(t("dock.needsYou"))}</div><div class="sub">${escapeHtml(reason)}</div>`;
    st.title = reason;
    frag.append(st);
    frag.append(button("", SVG.eye, t("dock.showPage"), () => api.invoke("show_seat", { seatId: p.seat, visible: true })));
    frag.append(button("red", SVG.hangup, t("dock.hangup"), () => api.invoke("hangup")));
  }

  bar.replaceChildren(frag);
  if (entering && !settings?.reduce_motion) {
    const kids = Array.from(bar.children) as HTMLElement[];
    const mid = (kids.length - 1) / 2;
    kids.forEach((k, i) => { k.style.setProperty("--d", String(Math.abs(i - mid))); k.classList.add("enter"); });
  }
  carryLensAcrossRender();
  await fit("bar");
  kickLens();
  if (p.phase === "in_call" || p.phase === "calling") startWave(); else stopWave();
}

function renderStatusTextInto(e: HTMLDivElement) { statusEl = e; renderStatusText(); }

// ───────── waveform: real mic level when available, otherwise a gentle idle motion ─────────
let waveCanvas: HTMLCanvasElement | null = null;
let analyser: AnalyserNode | null = null;
let raf = 0;
const bars = new Float32Array(16);
// The microphone is held only while a call is on the waveform: one try per call, let go after.
let micOn = false;
let micGen = 0;
let releaseMic = () => {};

async function ensureAnalyser() {
  if (micOn || !navigator.mediaDevices?.getUserMedia) return;
  micOn = true;
  const gen = micGen;
  try {
    const stream = await navigator.mediaDevices.getUserMedia({ audio: METER_AUDIO });
    const ctx = new AudioContext();
    const release = () => { stream.getTracks().forEach((t) => t.stop()); void ctx.close(); };
    // the call ended while the microphone was coming up
    if (gen !== micGen) { release(); return; }
    const an = ctx.createAnalyser();
    an.fftSize = 64;
    an.smoothingTimeConstant = .7;
    ctx.createMediaStreamSource(stream).connect(an);
    analyser = an;
    releaseMic = () => { analyser = null; release(); };
  } catch { /* no microphone access: the bars keep their gentle motion */ }
}

function startWave() {
  ensureAnalyser();
  cancelAnimationFrame(raf);
  const buf = new Uint8Array(32);
  const draw = (t: number) => {
    const c = waveCanvas; if (!c || !c.isConnected) return;
    const ctx = c.getContext("2d")!;
    const dpr = 2; const W = c.width / dpr, Hh = c.height / dpr;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, W, Hh);
    // bars run along the pill's main axis; in the vertical pill the canvas is taller than wide
    const vert = Hh > W;
    const L = vert ? Hh : W, T = vert ? W : Hh;
    const n = bars.length, gap = 2.6, bw = (L - gap * (n - 1)) / n;
    if (analyser && !state.muted) { analyser.getByteFrequencyData(buf); }
    for (let i = 0; i < n; i++) {
      let target: number;
      if (state.muted) target = .06;
      else if (analyser) target = Math.min(1, (buf[i + 2] / 255) * 1.6 + .06);
      else target = .18 + .14 * Math.sin(t / 420 + i * .9) + .08 * Math.sin(t / 190 + i * 1.7);
      bars[i] += (target - bars[i]) * .25;
      const len = Math.max(2.5, bars[i] * (T - 6));
      const along = i * (bw + gap), across = (T - len) / 2;
      ctx.fillStyle = getComputedStyle(dock).getPropertyValue("--sym").trim() || "#5c6b99";
      ctx.globalAlpha = state.muted ? .35 : .92;
      if (vert) roundRect(ctx, across, along, len, bw, bw / 2); else roundRect(ctx, along, across, bw, len, bw / 2);
    }
    raf = requestAnimationFrame(draw);
  };
  raf = requestAnimationFrame(draw);
}
function roundRect(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number) {
  ctx.beginPath(); ctx.moveTo(x + r, y); ctx.arcTo(x + w, y, x + w, y + h, r); ctx.arcTo(x + w, y + h, x, y + h, r); ctx.arcTo(x, y + h, x, y, r); ctx.arcTo(x, y, x + w, y, r); ctx.closePath(); ctx.fill();
}
function stopWave() {
  cancelAnimationFrame(raf); raf = 0;
  if (!micOn) return;
  micOn = false; micGen++;
  releaseMic(); releaseMic = () => {};
}

// ───────── wiring ─────────
collapsed.addEventListener("click", () => {
  if (Date.now() - lastDragEnd > 300) void api.invoke("set_expanded", { expanded: true });
});
collapsed.addEventListener("keydown", (e) => { if (e.key === "Enter" || e.key === " ") api.invoke("set_expanded", { expanded: true }); });
dock.addEventListener("mouseleave", () => { if (hoverSeat) { hoverSeat = null; renderStatusText(); } });
window.addEventListener("blur", () => { if (hoverSeat) { hoverSeat = null; renderStatusText(); } });
dock.addEventListener("contextmenu", (e) => { e.preventDefault(); api.invoke("dock_menu"); });
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && state.expanded && state.phase.phase === "idle") api.invoke("set_expanded", { expanded: false });
});

// ───────── grip: the grabber at the pill's end — tap flips 横向/竖向, hold-and-drag moves the Dock ─────────
// The symbol shows what a tap turns the pill into: ⇅ "stand it up" on the horizontal bar, ⇄ "lay it flat" on the vertical one.
const GRIP_SVG = {
  toVertical: '<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M7 16V4M4.5 6.5 7 4l2.5 2.5M13 4v12M10.5 13.5 13 16l2.5-2.5"/></svg>',
  toHorizontal: '<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M4 7h12M13.5 4.5 16 7l-2.5 2.5M16 13H4M6.5 10.5 4 13l2.5 2.5"/></svg>',
};
function grip(): HTMLDivElement {
  const g = el("div", "grip", vertical() ? GRIP_SVG.toHorizontal : GRIP_SVG.toVertical);
  g.dataset.k = "grip";
  g.title = t(vertical() ? "dock.gripVertical" : "dock.gripHorizontal");
  g.addEventListener("click", () => { if (Date.now() - lastDragEnd > 300) void api.invoke("toggle_orientation"); });
  return g;
}

// ───────── auto-collapse: an idle, un-hovered pill folds back into the dot/pet after N s ─────────
// A deadline rather than a bare timeout: the 1 Hz state ticks re-render the pill, and re-arming a
// timeout on every render would mean it never fires.
let idleTimer: number | undefined;
let idleDeadline = 0;
let hovering = false;
// Auto: fold after N s. Pinned-expanded: never by itself. Pinned-collapsed: hover is only a preview.
function idleDelayMs(): number {
  const mode = settings?.collapse_mode ?? "auto";
  if (mode === "pinned_expanded") return 0;
  if (mode === "pinned_collapsed") return 600;
  return (settings?.auto_collapse_secs ?? 0) * 1000;
}
function touchIdle() {
  idleDeadline = Date.now() + idleDelayMs();
  scheduleIdleCheck();
}
function scheduleIdleCheck() {
  window.clearTimeout(idleTimer);
  if (!idleDelayMs()) return;
  idleTimer = window.setTimeout(checkIdle, Math.max(50, idleDeadline - Date.now()));
}
async function checkIdle() {
  if (!state.expanded || state.phase.phase !== "idle") return;
  if (Date.now() < idleDeadline) { scheduleIdleCheck(); return; }
  // The webview's enter/leave tracking is lost when the window resizes under the pointer, so ask
  // the OS where the cursor really is instead of trusting `hovering`.
  const over = dragging || (api.isTauri ? await api.invoke<boolean>("cursor_over_dock") : hovering);
  if (over) { touchIdle(); return; }
  void api.invoke("set_expanded", { expanded: false });
}
function pointerEntered() {
  if (hovering) return;
  hovering = true;
  window.clearTimeout(idleTimer);
}
function pointerLeft() {
  if (!hovering) return;
  hovering = false;
  touchIdle();
}
dock.addEventListener("pointerenter", pointerEntered);
dock.addEventListener("pointerleave", pointerLeft);
// macOS: the webview gets no pointer events while the Dock isn't the key window (it never is, by
// design), so Rust polls the OS cursor against the window rect and reports enter/leave here, and
// streams the local position while inside (drives the liquid lens below).
void api.on<boolean>("dock:hover", (over) => {
  if (over) { pointerEntered(); return; }
  pointerLeft();
  ptr = null; kickLens();
});
void api.on<[number, number]>("dock:pointer", ([x, y]) => { ptr = { x, y }; kickLens(); });
dock.addEventListener("pointermove", (e) => { ptr = { x: e.clientX, y: e.clientY }; kickLens(); });
dock.addEventListener("pointerleave", () => { ptr = null; kickLens(); });

// ───────── liquid lens: one glass droplet that follows the pointer between controls ─────────
// Each edge of the droplet is its own spring. The edge facing the direction of travel is stiff and
// the trailing edge soft, so sliding between buttons stretches the lens into a capsule that pulls
// free and snaps back — the "sticky" Liquid Glass feel. While stretched it thins a little across
// (volume-preserving); a press squashes it and pools light under the pointer, release overshoots.
const lens = el("div", "lens");
lens.setAttribute("aria-hidden", "true");
dock.insertBefore(lens, bar);
type Spring = { x: number; v: number };
const spring = (x = 0): Spring => ({ x, v: 0 });
const edge = { l: spring(), r: spring(), t: spring(), b: spring() };
const vis = spring(), squeeze = spring(1);
let ptr: { x: number; y: number } | null = null;
let hot: HTMLElement | null = null;          // control under the pointer right now
let anchor: HTMLElement | null = null;       // where the lens is (stays put while it fades out)
let pressedEl: HTMLElement | null = null;
let lensRaf = 0, lensLast = 0;
const clamp01 = (v: number) => Math.min(1, Math.max(0, v));

function stepSpring(s: Spring, target: number, k: number, zeta: number, dt: number) {
  s.v += (-k * (s.x - target) - 2 * zeta * Math.sqrt(k) * s.v) * dt;
  s.x += s.v * dt;
}
const near = (s: Spring, target: number, eps: number) => Math.abs(s.x - target) < eps && Math.abs(s.v) < eps * 6;

function lensTargetAt(x: number, y: number): HTMLElement | null {
  if (dock.dataset.form !== "bar" || dragging || resizing) return null;
  const t = (document.elementFromPoint(x, y) as HTMLElement | null)?.closest<HTMLElement>(".btn, .grip, .avatar, .undo");
  if (t && bar.contains(t)) return t;
  const d = dock.getBoundingClientRect();
  if (x < d.left || x > d.right || y < d.top || y > d.bottom) return null;
  const maxGap = 14 * currentScale();
  let best: HTMLElement | null = null, bestDist = maxGap;
  for (const c of bar.querySelectorAll<HTMLElement>(".btn, .grip, .avatar, .undo")) {
    const r = c.getBoundingClientRect();
    const dx = x < r.left ? r.left - x : x > r.right ? x - r.right : 0;
    const dy = y < r.top ? r.top - y : y > r.bottom ? y - r.bottom : 0;
    const dist = Math.hypot(dx, dy);
    if (dist <= bestDist) { bestDist = dist; best = c; }
  }
  return best;
}

/** After a re-render swaps the DOM, re-attach hover state to the control with the same identity. */
function carryLensAcrossRender() {
  const k = (hot ?? anchor)?.dataset.k;
  if (!k) return;
  const n = Array.from(bar.querySelectorAll<HTMLElement>("[data-k]")).find((e) => e.dataset.k === k) ?? null;
  if (hot && n) { n.classList.add("is-hot"); hot = n; }
  if (anchor) anchor = n;
}

function kickLens() {
  if (lensRaf) return;
  lensLast = performance.now();
  lensRaf = requestAnimationFrame(lensFrame);
}

function lensFrame(now: number) {
  lensRaf = 0;
  const elapsed = Math.min(1 / 30, Math.max(1 / 240, (now - lensLast) / 1000));
  lensLast = now;
  const target = ptr ? lensTargetAt(ptr.x, ptr.y) : null;
  if (target !== hot) {
    hot?.classList.remove("is-hot");
    target?.classList.add("is-hot");
    hot = target;
    const nextSeat = target?.dataset.seat ?? null;
    if (nextSeat !== hoverSeat) { hoverSeat = nextSeat; renderStatusText(); }
  }
  dock.classList.toggle("hot", !!ptr && dock.dataset.form !== "bar" && !dragging);
  if (anchor && !anchor.isConnected) anchor = null;
  if (target) anchor = target;

  const d = dock.getBoundingClientRect();
  let goal: { l: number; r: number; t: number; b: number } | null = null;
  if (anchor) {
    const r = anchor.getBoundingClientRect();
    const capsule = anchor.classList.contains("undo");
    const side = Math.max(r.width, r.height);
    const w = capsule ? r.width + 6 : side, h = capsule ? r.height + 6 : side;
    const cx = r.left + r.width / 2 - d.left, cy = r.top + r.height / 2 - d.top;
    goal = { l: cx - w / 2, r: cx + w / 2, t: cy - h / 2, b: cy + h / 2 };
  }
  // filled buttons (hang up, muted mic) are already a disc: the lens slides in and dissolves into them
  const wantVis = target && !target.matches(".red, .amber") ? 1 : 0;
  const reduce = !!settings?.reduce_motion;
  const horiz = !vertical();
  if (goal && (vis.x < 0.03 || reduce)) {
    // appearing (or reduced motion): materialise in place instead of sliding in from a stale spot
    edge.l = spring(goal.l); edge.r = spring(goal.r); edge.t = spring(goal.t); edge.b = spring(goal.b);
  }
  const steps = Math.ceil(elapsed * 240), dt = elapsed / steps;
  for (let i = 0; i < steps; i++) {
    if (goal && !reduce) {
      // main axis = along the pill; cross axis = its thickness
      const lo = horiz ? edge.l : edge.t, hi = horiz ? edge.r : edge.b;
      const glo = horiz ? goal.l : goal.t, ghi = horiz ? goal.r : goal.b;
      const clo = horiz ? edge.t : edge.l, chi = horiz ? edge.b : edge.r;
      const gclo = horiz ? goal.t : goal.l, gchi = horiz ? goal.b : goal.r;
      const travel = (glo + ghi) / 2 - (lo.x + hi.x) / 2;
      const dir = Math.abs(travel) < 0.5 ? 0 : Math.sign(travel);
      const STIFF = 900, SOFT = 210, MID = 520;
      stepSpring(hi, ghi, dir > 0 ? STIFF : dir < 0 ? SOFT : MID, dir < 0 ? 0.95 : 0.78, dt);
      stepSpring(lo, glo, dir < 0 ? STIFF : dir > 0 ? SOFT : MID, dir > 0 ? 0.95 : 0.78, dt);
      stepSpring(clo, gclo, MID, 0.82, dt);
      stepSpring(chi, gchi, MID, 0.82, dt);
    }
    stepSpring(vis, wantVis, 420, 1, dt);
    stepSpring(squeeze, pressedEl ? 0.86 : 1, 760, pressedEl ? 0.9 : 0.38, dt);
  }
  if (!target && vis.x < 0.02) anchor = null;

  let x = edge.l.x, y = edge.t.x, w = edge.r.x - edge.l.x, h = edge.b.x - edge.t.x;
  if (goal) {
    const len = horiz ? w : h, base = horiz ? goal.r - goal.l : goal.b - goal.t;
    const thin = 1 - Math.min(0.24, Math.max(0, len / Math.max(1, base) - 1) * 0.3);
    if (horiz) { const nh = h * thin; y += (h - nh) / 2; h = nh; } else { const nw = w * thin; x += (w - nw) / 2; w = nw; }
  }
  const shown = clamp01(vis.x);
  lens.style.width = `${Math.max(0, w)}px`;
  lens.style.height = `${Math.max(0, h)}px`;
  lens.style.transform = `translate(${x}px, ${y}px) scale(${(0.7 + 0.3 * shown) * squeeze.x})`;
  lens.style.opacity = String(shown);
  if (ptr && w > 0 && h > 0) {
    lens.style.setProperty("--hx", `${clamp01((ptr.x - d.left - x) / w) * 100}%`);
    lens.style.setProperty("--hy", `${clamp01((ptr.y - d.top - y) / h) * 100}%`);
  }
  lens.style.setProperty("--glow", String(clamp01((1 - squeeze.x) / 0.12)));

  const settled = near(vis, wantVis, 0.004) && near(squeeze, pressedEl ? 0.86 : 1, 0.002)
    && (!goal || (near(edge.l, goal.l, 0.2) && near(edge.r, goal.r, 0.2) && near(edge.t, goal.t, 0.2) && near(edge.b, goal.b, 0.2)));
  if (!settled) lensRaf = requestAnimationFrame(lensFrame);
}

dock.addEventListener("pointerdown", (e) => {
  if (e.button !== 0) return;
  const t = lensTargetAt(e.clientX, e.clientY);
  ptr = { x: e.clientX, y: e.clientY };
  if (t) { pressedEl = t; t.classList.add("is-pressed"); }
  else if (dock.dataset.form !== "bar") dock.classList.add("pressing");
  kickLens();
});
const releasePress = () => {
  pressedEl?.classList.remove("is-pressed");
  pressedEl = null;
  dock.classList.remove("pressing");
  kickLens();
};
window.addEventListener("pointerup", releasePress);
window.addEventListener("pointercancel", releasePress);

// ───────── drag: grab the glass itself (any non-button area, or the dot) and put the Dock anywhere ─────────
// The window follows the OS cursor (one IPC per frame; Rust reads the real cursor position) instead
// of the OS move loop, which behaves differently per window manager. A 4px threshold keeps plain
// clicks on the dot working; on release the new centre is persisted as the anchor (snap → "free").
const uiLog = (level: "debug" | "warn" | "error", msg: string) => { if (api.isTauri) void api.invoke("ui_log", { level, msg }); };
window.addEventListener("error", (e) => uiLog("error", `${e.message} @${e.filename}:${e.lineno}`));
window.addEventListener("unhandledrejection", (e) => uiLog("error", `unhandled: ${String((e as PromiseRejectionEvent).reason)}`));
let grab: { x: number; y: number } | null = null;   // pointer-down point, in window px
let dragging = false;
let resizeGrab: { sx: number; sy: number; startScale: number; signX: number; signY: number } | null = null;
let resizing = false;
let lastDragEnd = 0;                                // lets the grip tell a tap from the end of a drag
let dragRaf = 0;
function followCursor() {
  dragRaf = 0;
  if (grab && dragging) void api.invoke("drag_dock", { offsetX: grab.x, offsetY: grab.y });
}
function detectResizeEdge(e: PointerEvent): "ew" | "ns" | null {
  const r = dock.getBoundingClientRect();
  if (r.width < 10 || r.height < 10) return null;
  const nearRight = r.right - e.clientX <= 7;
  const nearBottom = r.bottom - e.clientY <= 7;
  if (nearRight) return "ew";
  if (nearBottom) return "ns";
  return null;
}
dock.addEventListener("pointerdown", (e) => {
  if (e.button !== 0 || (e.target as HTMLElement).closest("button, canvas")) return;
  const edge = !(e.target as HTMLElement).closest(".grip") ? detectResizeEdge(e) : null;
  if (edge || e.altKey || e.metaKey) {
    const r = dock.getBoundingClientRect();
    const signX = e.clientX >= r.left + r.width / 2 ? 1 : -1;
    const signY = e.clientY >= r.top + r.height / 2 ? 1 : -1;
    resizeGrab = { sx: e.screenX, sy: e.screenY, startScale: currentScale(), signX, signY };
    return;
  }
  grab = { x: e.clientX, y: e.clientY };
});
window.addEventListener("pointermove", (e) => {
  if (!grab && !resizeGrab) {
    const edge = detectResizeEdge(e);
    if (edge) dock.dataset.resizeEdge = edge; else delete dock.dataset.resizeEdge;
    return;
  }
  if (resizeGrab) {
    const dx = (e.screenX - resizeGrab.sx) * resizeGrab.signX;
    const dy = (e.screenY - resizeGrab.sy) * resizeGrab.signY;
    if (!resizing) {
      if (Math.hypot(dx, dy) < 3) return;
      resizing = true;
      dock.setPointerCapture(e.pointerId);
      dock.classList.add("resizing");
    }
    const delta = (dx + dy) / 220;
    const next = Math.round(Math.min(1.6, Math.max(0.7, resizeGrab.startScale + delta)) * 40) / 40;
    if (next !== liveScale) {
      liveScale = next;
      document.documentElement.style.setProperty("--scale", String(next));
      const form = (dock.dataset.form as "dot" | "pet" | "bar") || "bar";
      void fit(form);
    }
    return;
  }
  if (!grab) return;
  if (!dragging) {
    if (Math.hypot(e.clientX - grab.x, e.clientY - grab.y) < 4) return;
    dragging = true;
    dock.setPointerCapture(e.pointerId);
    dock.classList.add("dragging");
  }
  if (!dragRaf) dragRaf = requestAnimationFrame(followCursor);
});
const endDrag = () => {
  grab = null;
  if (resizeGrab) {
    const finalScale = liveScale;
    resizeGrab = null;
    if (resizing) {
      resizing = false;
      lastDragEnd = Date.now();
      dock.classList.remove("resizing");
      if (finalScale !== null && settings) {
        settings = { ...settings, dock_scale: finalScale };
        liveScale = null;
        void api.invoke("save_settings", { settings });
      } else {
        liveScale = null;
      }
      touchIdle();
    }
    return;
  }
  if (!dragging) return;
  dragging = false;
  lastDragEnd = Date.now();
  dock.classList.remove("dragging");
  cancelAnimationFrame(dragRaf);
  dragRaf = 0;
  void api.invoke("remember_dock_position");
  touchIdle();
};
window.addEventListener("pointerup", endDrag);
window.addEventListener("pointercancel", endDrag);

// Wheel / trackpad pinch over the pill: proportional zoom in [0.70, 1.60].
let wheelSaveTimer: number | undefined;
dock.addEventListener("wheel", (e) => {
  e.preventDefault();
  if (!settings) return;
  const dir = e.deltaY < 0 ? 1 : e.deltaY > 0 ? -1 : 0;
  if (!dir) return;
  const step = e.ctrlKey ? 0.03 : 0.05;
  const next = Math.round(Math.min(1.6, Math.max(0.7, currentScale() + dir * step)) * 20) / 20;
  if (next === currentScale()) return;
  settings = { ...settings, dock_scale: next };
  document.documentElement.style.setProperty("--scale", String(next));
  const form = (dock.dataset.form as "dot" | "pet" | "bar") || "bar";
  void fit(form);
  window.clearTimeout(wheelSaveTimer);
  wheelSaveTimer = window.setTimeout(() => { if (settings) void api.invoke("save_settings", { settings }); }, 220);
}, { passive: false });

/** The OS's preferred language, for "follow the system". */
let hostLocale: string | undefined;

function applySettings(s: AppSettings) {
  settings = s;
  setLang(resolveLang(s.language, hostLocale));
  collapsed.setAttribute("aria-label", t("dock.expand"));
  dock.dataset.orient = s.orientation ?? "horizontal";
  touchIdle();
  applyTheme(s.theme);
  document.documentElement.toggleAttribute("data-reduce-motion", s.reduce_motion);
  document.documentElement.style.setProperty("--material", String(s.material_strength));
  const accent = validAccent(s.accent);
  document.documentElement.style.setProperty("--tint", accent ?? "transparent");
  if (accent) document.documentElement.dataset.accent = "on"; else delete document.documentElement.dataset.accent;
  document.documentElement.style.setProperty("--scale", String(Math.min(1.6, Math.max(0.7, s.dock_scale ?? 1))));
}

async function boot() {
  const host = await api.invoke<HostInfo>("host_info");
  hostLocale = host.locale;
  // The CSS keys off `[data-vibrancy="1"]`; toggleAttribute() would set an empty value and never match.
  if (host.vibrancy) document.documentElement.setAttribute("data-vibrancy", "1"); else document.documentElement.removeAttribute("data-vibrancy");
  document.documentElement.dataset.os = host.os;
  if (api.isTauri && host.os === "macos") document.documentElement.dataset.morph = "1";
  // glyph colours follow the glass's backdrop-adaptive appearance (see data-glass in dock.css)
  const setGlass = (scheme: string | null) => { if (scheme) document.documentElement.dataset.glass = scheme; };
  setGlass(await api.invoke<string | null>("glass_scheme"));
  void api.on<string>("dock:glass", setGlass);
  applySettings(await api.invoke<AppSettings>("get_settings"));
  seats = await api.invoke<SeatView[]>("get_seats");
  state = await api.invoke<DockState>("get_state");
  lastSize = { w: 0, h: 0 };
  await render();
  booting = false;
  api.on<DockState>("dock:state", (s) => { state = s; render(); });
  api.on<SeatView[]>("seats:changed", (s) => { seats = s; iconCache.clear(); render(); });
  api.on<AppSettings>("settings:changed", (s) => { applySettings(s); render(); });
  // Demo mode in a plain browser: expose a tiny console API.
  if (!api.isTauri) (window as any).agentdock = { call: (id: string) => api.invoke("call_seat", { seatId: id }), hangup: () => api.invoke("hangup"), mute: () => api.invoke("toggle_mute") };
}
boot();
