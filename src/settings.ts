import { ACCENTS, api, applyTheme, METER_AUDIO, validAccent, type AppSettings, type ChromeStatus, type DriverInfo, type HostInfo, type MemoryStatus, type Seat, type SeatView } from "./api";
import { iconFor } from "./brand";
import { lang, LANGUAGES, resolveLang, setLang, t, translateDom, type LangSetting, type MsgKey } from "./i18n";

type PageId = "seats" | "hotkeys" | "appearance" | "mic" | "memory" | "about";
const TITLES: Record<PageId, [MsgKey, MsgKey]> = {
  seats: ["nav.seats", "sub.seats"],
  hotkeys: ["nav.hotkeys", "sub.hotkeys"],
  appearance: ["nav.appearance", "sub.appearance"],
  mic: ["nav.mic", "sub.mic"],
  memory: ["nav.memory", "sub.memory"],
  about: ["nav.about", "sub.about"],
};

let settings: AppSettings;
let seats: SeatView[] = [];
let drivers: DriverInfo[] = [];
let host: HostInfo = { os: "unknown", vibrancy: false, material: undefined, version: "" };
let page: PageId = "seats";

const $ = <T extends HTMLElement>(sel: string) => document.querySelector(sel) as T;
const pageEl = $("#page");
const esc = (s: string) => s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]!));
const el = <K extends keyof HTMLElementTagNameMap>(tag: K, cls = "", html = "") => { const e = document.createElement(tag); if (cls) e.className = cls; if (html) e.innerHTML = html; return e; };

let toastTimer: number | undefined;
function toast(msg: string) {
  const t = $("#toast"); t.textContent = msg; t.classList.add("is-on");
  window.clearTimeout(toastTimer); toastTimer = window.setTimeout(() => t.classList.remove("is-on"), 1800);
}

async function saveSettings(patch: Partial<AppSettings>) {
  settings = { ...settings, ...patch };
  applySettingsLocally();
  await api.invoke("save_settings", { settings });
}
function applySettingsLocally() {
  applyLanguage();
  applyTheme(settings.theme);
  document.documentElement.toggleAttribute("data-reduce-motion", settings.reduce_motion);
  document.documentElement.style.setProperty("--material", String(settings.material_strength ?? 0.5));
  const accent = validAccent(settings.accent);
  if (accent) document.documentElement.style.setProperty("--accent", accent); else document.documentElement.style.removeProperty("--accent");
}

/** Switch the interface language: static markup at once, the current page on the next render. */
function applyLanguage(): boolean {
  const next = resolveLang(settings.language, host.locale);
  paintLangButton();
  if (next === lang() && document.documentElement.lang === next) return false;
  setLang(next);
  translateDom();
  paintLangButton();
  return true;
}

/** The sidebar language button: the language in use, by its own name. */
function paintLangButton() {
  const name = LANGUAGES.find(([v]) => v === lang())?.[1] ?? "";
  $("#lang-name").textContent = (settings.language ?? "system") === "system" ? `${name} · ${t("lang.system")}` : name;
}

async function chooseLanguage(v: LangSetting) {
  await saveSettings({ language: v });
  await show(page);
}

function openLangMenu(anchor: HTMLElement) {
  const cur = settings.language ?? "system";
  anchor.setAttribute("aria-expanded", "true");
  openMenu(anchor, LANGUAGES.map(([v, name]) => ({ label: `${v === cur ? "✓" : "\u2003"} ${v === "system" ? t("lang.system") : name}`, run: () => void chooseLanguage(v) })), "left");
}

/** 主题色: preset swatches plus a custom picker; the Dock repaints live as you choose. */
function accentPicker(): HTMLDivElement {
  const wrap = el("div", "swatches");
  const presets = new Set(ACCENTS.map(([c]) => c));
  const custom = el("label", "swatch custom") as HTMLLabelElement;
  custom.title = t("accent.custom");
  const input = el("input") as HTMLInputElement;
  input.type = "color";
  input.value = validAccent(settings.accent) ?? "#0a84ff";
  const paint = () => {
    const cur = validAccent(settings.accent);
    wrap.querySelectorAll<HTMLElement>(".swatch[data-c]").forEach((b) => b.classList.toggle("is-on", (b.dataset.c || null) === cur));
    const isCustom = !!cur && !presets.has(cur.toLowerCase());
    custom.classList.toggle("is-on", isCustom);
    if (isCustom) custom.style.setProperty("--c", cur!);
  };
  for (const [c, key] of ACCENTS) {
    const b = el("button", `swatch${c ? "" : " none"}`);
    b.type = "button";
    b.title = t(key);
    b.setAttribute("aria-label", t("accent.aria", { name: t(key) }));
    b.dataset.c = c ?? "";
    if (c) b.style.setProperty("--c", c);
    b.addEventListener("click", () => { void saveSettings({ accent: c }); paint(); });
    wrap.append(b);
  }
  let pending: number | undefined;
  input.addEventListener("input", () => {
    settings = { ...settings, accent: input.value };
    paint();
    window.clearTimeout(pending);
    pending = window.setTimeout(() => void saveSettings({ accent: input.value }), 90);
  });
  custom.append(input);
  wrap.append(custom);
  paint();
  return wrap;
}

function toggle(checked: boolean, on: (v: boolean) => void): HTMLButtonElement {
  const b = el("button", "switch"); b.setAttribute("role", "switch"); b.setAttribute("aria-checked", String(checked));
  b.addEventListener("click", () => { const v = b.getAttribute("aria-checked") !== "true"; b.setAttribute("aria-checked", String(v)); on(v); });
  return b;
}
function seg<T extends string>(opts: [T, string][], value: T, on: (v: T) => void): HTMLDivElement {
  const s = el("div", "seg");
  for (const [v, label] of opts) {
    const b = el("button", v === value ? "is-on" : "", esc(label)); b.type = "button";
    b.addEventListener("click", () => { s.querySelectorAll("button").forEach((x) => x.classList.remove("is-on")); b.classList.add("is-on"); on(v); });
    s.append(b);
  }
  return s;
}
function row(name: string, sub: string, control: HTMLElement | HTMLElement[]): HTMLDivElement {
  const r = el("div", "row");
  r.append(el("div", "row__text", `<div class="row__name">${name}</div>${sub ? `<div class="row__sub">${sub}</div>` : ""}`));
  const a = el("div", "actions"); a.append(...(Array.isArray(control) ? control : [control])); r.append(a);
  return r;
}
function group(title: string, ...rows: HTMLElement[]): HTMLDivElement {
  const g = el("div", "group"); if (title) g.append(el("div", "group__title", esc(title))); g.append(...rows); return g;
}
async function avatarFor(s: SeatView, lg = false): Promise<HTMLDivElement> {
  const a = el("div", `avatar${lg ? " lg" : ""}`);
  if (s.icon.kind === "custom") {
    const data = await api.invoke<string | null>("seat_icon_data", { seatId: s.id });
    a.innerHTML = data ? `<img src="${data}" alt="">` : await iconFor(s.id);
  } else a.innerHTML = await iconFor(s.icon.key);
  return a;
}

// ───────────── Agent（代码里仍叫 seat） ─────────────
// One line per seat: who it is, one short status, and at most one action that is actually needed
// right now. Everything occasional (test call, rename, icon, session, remove) lives in the ⋯ menu.
function statusLabel(s: SeatView): string {
  switch (s.status) {
    case "connected": return t("status.connected");
    case "needs_auth": return t(s.driver.kind === "app" ? "status.needsAuthApp" : "status.needsAuthWeb");
    case "pending": return t(driverOf(s.driver.driver)?.pending ? "status.placeholder" : "status.voiceClosed");
    case "unavailable": return t("status.unavailable");
    default: return t("status.unknown");
  }
}
const driverOf = (id: string) => drivers.find((d) => d.id === id);
/** What a driver's call targets are called: "Bot" in Grok Bot, "dot" in ChatGPT. */
const nounOf = (id: string) => driverOf(id)?.target_noun ?? t("noun.target");
const iconKey = (driverId: string) => (driverId.startsWith("chatgpt") ? "dot" : driverId.replace(/-(voice|app)$/, ""));
const seatWhere = (s: SeatView) => s.driver.kind === "web" ? new URL(s.driver.url).host.replace(/^www\./, "")
  : s.target && s.target !== s.name ? t("where.appTarget", { noun: nounOf(s.driver.driver), target: s.target }) : t("where.app");
const overlaps = (a: string, b: string) => a.toLowerCase().includes(b.toLowerCase()) || b.toLowerCase().includes(a.toLowerCase());

async function connectSeat(s: SeatView) {
  try {
    if (s.driver.kind === "web") toast(t("toast.opening"));
    const via = await api.invoke<string>("connect_account", { seatId: s.id });
    toast(t(via === "chrome" ? "toast.openedChrome" : s.driver.kind === "web" ? "toast.loginWindow" : "toast.axOpened"));
  } catch (e) { toast(String(e)); }
}

/** 登录方式: web Agents use the user's Chrome (its logins) when Chrome's remote debugging is on. */
async function chromeGroup(): Promise<HTMLDivElement> {
  const on = settings.web_via_chrome ?? true;
  let sub = t("chrome.off");
  if (on) {
    let st: ChromeStatus = { available: false, connected: false };
    try { st = await api.invoke<ChromeStatus>("chrome_status"); } catch { /* mock-less hosts */ }
    sub = t(st.connected ? "chrome.connected" : st.available ? "chrome.available" : "chrome.missing");
  }
  return group(t("chrome.group"), row(t("chrome.row"), sub, toggle(on, (v) => void saveSettings({ web_via_chrome: v }).then(() => { if (page === "seats") void renderSeats(); }))));
}
async function testSeat(s: SeatView) {
  toast(t("toast.testing"));
  const ok = await api.invoke<boolean>("test_call", { seatId: s.id });
  toast(t(ok ? "toast.testOk" : "toast.testFail"));
}
const saveSeat = (id: string, patch: Partial<Seat>) =>
  api.invoke("save_seats", { seats: seats.map((x) => (x.id === id ? { ...stripView(x), ...patch } : stripView(x))) });

function pickIcon(s: SeatView) {
  const file = el("input") as HTMLInputElement;
  file.type = "file"; file.accept = ".png,.svg,.webp,.gif,.apng,.jpg,.jpeg,image/*";
  file.addEventListener("change", async () => {
    const f = file.files?.[0]; if (!f) return;
    const buf = new Uint8Array(await f.arrayBuffer()); let bin = "";
    for (let i = 0; i < buf.length; i += 0x8000) bin += String.fromCharCode(...buf.subarray(i, i + 0x8000));
    try { await api.invoke("set_seat_icon", { seatId: s.id, fileName: f.name, dataBase64: btoa(bin) }); toast(t("toast.iconChanged")); } catch (e) { toast(String(e)); }
  });
  file.click();
}

/** Inline edit of the name (or which Bot it calls): the name turns into a field; Enter / blur
 *  saves, Esc cancels. An empty Bot means "whatever is open in the app". */
function renameInline(r: HTMLElement, s: SeatView, field: "name" | "target" = "name") {
  const nameEl = r.querySelector<HTMLElement>(".row__name"); if (!nameEl) return;
  const input = el("input", "inline-name") as HTMLInputElement;
  const cur = (field === "name" ? s.name : s.target) ?? "";
  input.type = "text"; input.value = cur;
  if (field === "target") input.placeholder = t("rename.targetPlaceholder", { service: s.service, noun: nounOf(s.driver.driver) });
  let done = false;
  const finish = async (save: boolean) => {
    if (done) return; done = true;
    input.blur();
    const v = input.value.trim();
    if (save && v !== cur && (v || field === "target")) await saveSeat(s.id, field === "name" ? { name: v } : { target: v || null });
    else void renderSeats();
  };
  input.addEventListener("keydown", (e) => { if (e.key === "Enter") void finish(true); if (e.key === "Escape") void finish(false); });
  input.addEventListener("blur", () => void finish(true));
  nameEl.replaceChildren(input);
  input.focus(); input.select();
}

// ── popover menu (macOS-style: glass card, accent highlight, two-step confirm for destructive items)
type MenuItem = { label: string; run: () => void; danger?: boolean; disabled?: boolean; confirm?: string };
let menuEl: HTMLDivElement | null = null;
function closeMenu() {
  const wasOpen = menuEl != null;
  menuEl?.remove(); menuEl = null;
  document.querySelector("#lang-btn")?.setAttribute("aria-expanded", "false");
  document.removeEventListener("pointerdown", onOutside, true);
  document.removeEventListener("keydown", onMenuKey, true);
  if (wasOpen) queueMicrotask(flushDeferred);
}

/** Typing a name or a menu open: a re-render now would throw the edit away, so it waits. */
const interacting = () => menuEl != null || !!document.activeElement?.closest("#page input, #page select, #page textarea");
function flushDeferred() {
  if (!deferredSeats || sorting || interacting()) return;
  deferredSeats = false;
  if (page === "seats" || page === "hotkeys") void show(page);
}
document.addEventListener("focusout", () => window.setTimeout(flushDeferred, 0));
function onOutside(e: Event) { if (menuEl && !menuEl.contains(e.target as Node)) closeMenu(); }
function onMenuKey(e: KeyboardEvent) { if (e.key === "Escape") closeMenu(); }
function openMenu(anchor: HTMLElement, items: (MenuItem | "-")[], align: "left" | "right" = "right") {
  closeMenu();
  const m = el("div", "menu"); m.setAttribute("role", "menu");
  for (const it of items) {
    if (it === "-") { m.append(el("div", "menu__sep")); continue; }
    const b = el("button", `menu__item${it.danger ? " danger" : ""}`, esc(it.label));
    b.type = "button"; b.disabled = !!it.disabled; b.setAttribute("role", "menuitem");
    b.addEventListener("click", () => {
      if (it.confirm && !b.classList.contains("is-armed")) { b.classList.add("is-armed"); b.textContent = it.confirm; return; }
      closeMenu(); it.run();
    });
    m.append(b);
  }
  document.body.append(m);
  const a = anchor.getBoundingClientRect(), mr = m.getBoundingClientRect();
  const below = a.bottom + 6 + mr.height < window.innerHeight - 8;
  m.style.left = `${align === "left" ? Math.min(a.left, window.innerWidth - mr.width - 8) : Math.max(8, a.right - mr.width)}px`;
  m.style.top = `${below ? a.bottom + 6 : a.top - 6 - mr.height}px`;
  m.style.transformOrigin = `${below ? "top" : "bottom"} ${align}`;
  menuEl = m;
  document.addEventListener("pointerdown", onOutside, true);
  document.addEventListener("keydown", onMenuKey, true);
}

function seatMenu(r: HTMLElement, s: SeatView): (MenuItem | "-")[] {
  const web = s.driver.kind === "web";
  const items: (MenuItem | "-")[] = [
    { label: t("menu.test"), run: () => void testSeat(s), disabled: !s.enabled || s.status === "pending" || s.status === "unavailable" },
  ];
  if (web) {
    if (s.status === "connected") items.push({ label: t("menu.relogin"), run: () => void connectSeat(s) });
    items.push({ label: t("menu.showPage"), run: () => void api.invoke("show_seat", { seatId: s.id, visible: true }).catch((e) => toast(String(e))) });
  }
  items.push("-", { label: t("menu.rename"), run: () => renameInline(r, s) });
  const noun = nounOf(s.driver.driver);
  if (driverOf(s.driver.driver)?.picks) items.push({ label: s.target ? t("menu.changeTarget", { noun, target: s.target }) : t("menu.pickTarget", { noun }), run: () => renameInline(r, s, "target") });
  items.push({ label: t("menu.changeIcon"), run: () => pickIcon(s) });
  if (s.icon.kind === "custom") items.push({ label: t("menu.resetIcon"), run: () => void api.invoke("reset_seat_icon", { seatId: s.id }) });
  items.push("-");
  if (web) items.push({ label: t("menu.signOut"), danger: true, confirm: t("menu.signOutConfirm"), run: async () => { await api.invoke("disconnect_seat", { seatId: s.id }); toast(t("toast.signedOut")); } });
  items.push({ label: t("menu.remove"), danger: true, confirm: t("menu.removeConfirm"), run: async () => { await api.invoke("save_seats", { seats: seats.filter((x) => x.id !== s.id).map(stripView) }); toast(t("toast.removed")); } });
  return items;
}

// 拖动排序用指针事件自己实现，不用 HTML5 拖放：Tauri 默认在原生层接管拖放（为了文件拖入），
// macOS 上 dragover / drop 根本到不了网页，所以真机里拖不动。这里按住任意一行（控件除外）上下拖，
// 被拖的行浮起跟手，其余行实时让位；松手后落到空位，再提交新顺序（后端同步 Dock 与 ⌥1–⌥9）。
let sorting = false;          // a re-render mid-drag would yank the row out from under the pointer
let deferredSeats = false;    // seats:changed arrived while sorting or editing → re-render once it's over
function enableReorder(list: HTMLElement) {
  list.addEventListener("pointerdown", (e) => {
    const t = e.target as HTMLElement;
    const row = t.closest<HTMLElement>(".row.seat");
    if (e.button !== 0 || !row || t.closest("button, input, select, label, a")) return;
    const rows = [...list.querySelectorAll<HTMLElement>(".row.seat")];
    const rects = rows.map((r) => r.getBoundingClientRect());
    const from = rows.indexOf(row), h = rects[from].height, startY = e.clientY;
    const minDy = rects[0].top - rects[from].top, maxDy = rects[rects.length - 1].bottom - rects[from].bottom;
    let to = from, dragging = false;
    const move = (ev: PointerEvent) => {
      const dy = ev.clientY - startY;
      if (!dragging) {
        if (Math.abs(dy) < 4) return;
        dragging = true;
        sorting = true;
        closeMenu();
        try { row.setPointerCapture(e.pointerId); } catch { /* window listeners still track the pointer */ }
        list.classList.add("is-sorting");
        row.classList.add("is-lifted");
      }
      const d = Math.max(minDy, Math.min(maxDy, dy));
      row.style.transform = `translateY(${d}px)`;
      const mid = rects[from].top + h / 2 + d;
      to = rects.findIndex((r) => mid < r.bottom);
      if (to < 0) to = rects.length - 1;
      rows.forEach((r, i) => {
        if (i === from) return;
        const shift = from < to && i > from && i <= to ? -h : from > to && i >= to && i < from ? h : 0;
        r.style.transform = shift ? `translateY(${shift}px)` : "";
      });
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", up);
      if (!dragging) return;
      // settle into the gap, then commit; the seats:changed re-render lands on the same layout
      const settle = to > from ? rects[to].bottom - rects[from].bottom : rects[to].top - rects[from].top;
      row.classList.remove("is-lifted");
      row.style.transform = `translateY(${settle}px)`;
      window.setTimeout(() => {
        sorting = false;
        if (to === from) {
          rows.forEach((r) => (r.style.transform = ""));
          list.classList.remove("is-sorting");
          if (deferredSeats) { deferredSeats = false; void renderSeats(); }
          return;
        }
        deferredSeats = false;
        const ids = rows.map((r) => r.dataset.id!);
        ids.splice(to, 0, ids.splice(from, 1)[0]);
        void api.invoke("reorder_seats", { ids });
      }, 200);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", up);
  });
}

async function renderSeats() {
  closeMenu();
  const list = el("div", "group seats");
  for (const s of seats) {
    const r = el("div", `row seat${s.enabled ? "" : " is-off"}`); r.dataset.id = s.id;
    r.append(el("span", "handle", "⋮⋮"));
    r.append(await avatarFor(s));
    const svc = overlaps(s.name, s.service) ? "" : `<span class="row__svc">${esc(s.service)}</span>`;
    const acct = s.account ? `<span class="sep">·</span>${esc(s.account)}` : "";
    const tip = esc(`${s.service} · ${s.driver.kind === "web" ? s.driver.url : t("seat.tipApp")}${s.driver_verified ? "" : ` · ${t("seat.tipUnverified")}`}`);
    r.append(el("div", "row__text", `<div class="row__name">${esc(s.name)}${svc}</div><div class="row__sub" title="${tip}"><span class="status" data-s="${s.status}">${statusLabel(s)}</span><span class="sep">·</span>${esc(seatWhere(s))}${acct}</div>`));

    const acts = el("div", "actions");
    // the one action worth showing: only when the seat is on and can't work yet
    const needsHand = s.enabled && s.status !== "connected" && s.status !== "unavailable" && s.status !== "pending";
    if (needsHand) {
      const b = el("button", "btn sm tint", t(s.driver.kind === "web" ? "btn.signIn" : "btn.grant"));
      b.addEventListener("click", () => void connectSeat(s));
      acts.append(b);
    }
    const more = el("button", "btn icon", `<svg viewBox="0 0 20 20" fill="currentColor"><circle cx="4.5" cy="10" r="1.6"/><circle cx="10" cy="10" r="1.6"/><circle cx="15.5" cy="10" r="1.6"/></svg>`);
    more.type = "button"; more.title = t("btn.more"); more.setAttribute("aria-label", t("btn.moreAria", { name: s.name }));
    more.addEventListener("click", (e) => { e.stopPropagation(); if (menuEl) closeMenu(); else openMenu(more, seatMenu(r, s)); });
    acts.append(more);
    acts.append(toggle(s.enabled, (v) => void saveSeat(s.id, { enabled: v })));
    r.append(acts);
    list.append(r);
  }
  enableReorder(list);

  // 添加: one row that opens the catalogue right below the list; no dropdown, nothing to type
  const add = el("button", "row add-toggle", `<span class="avatar plus">${addOpen ? MINUS_SVG : PLUS_SVG}</span><span class="row__name">${t(addOpen ? "add.close" : "add.open")}</span>`) as HTMLButtonElement;
  add.type = "button";
  add.setAttribute("aria-expanded", String(addOpen));
  add.addEventListener("click", () => {
    addOpen = !addOpen;
    // Bots are read as soon as the catalogue opens, so they are just there to tap
    if (addOpen) for (const d of drivers) if (d.discover && d.installed && !(d.id in found)) void readTargets(d.id);
    void renderSeats();
  });
  list.append(add);

  const help = el("details", "help");
  const tips: MsgKey[] = ["help.chrome", "help.builtin", "help.background", "help.mic", "help.apps", "help.bots", "help.chatgpt"];
  help.innerHTML = `<summary>${esc(t("help.summary"))}</summary><ul>${tips.map((k) => `<li>${esc(t(k))}</li>`).join("")}</ul>`;
  const parts: HTMLElement[] = [list];
  if (addOpen) parts.push(await addPanel());
  parts.push(await chromeGroup(), help);
  pageEl.replaceChildren(...parts);
}

const PLUS_SVG = `<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="M10 5v10M5 10h10"/></svg>`;
const MINUS_SVG = `<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="M5 10h10"/></svg>`;

async function addSeat(driverId: string, target = "") {
  const d = driverOf(driverId);
  if (!d) return;
  const seat: Seat = {
    id: `${d.id}-${Math.random().toString(36).slice(2, 6)}`, name: target || d.display_name, service: d.display_name,
    driver: d.kind === "web" ? { kind: "web", url: d.url ?? "", driver: d.id } : { kind: "app", bundle_id: d.bundle_id ?? d.id, driver: d.id },
    icon: { kind: "builtin", key: iconKey(d.id) }, enabled: true, status: d.pending ? "pending" : "unknown", account: null,
    target: d.picks && target ? target : null,
  };
  await api.invoke("save_seats", { seats: [...seats.map(stripView), seat] });
  toast(seat.target ? t("toast.addedTarget", { name: seat.target }) : d.pending ? t("toast.addedPlaceholder", { name: d.display_name }) : t(d.kind === "web" ? "toast.addedWeb" : "toast.added"));
}

/** The catalogue is open (kept across re-renders while the window is open). */
let addOpen = false;
/** Targets read from apps, per driver (Grok Bot's Bots); `[]` = read, nothing found. */
const found: Record<string, string[]> = {};
const reading = new Set<string>();
async function readTargets(driverId: string) {
  if (reading.has(driverId)) return;
  reading.add(driverId);
  void renderSeats();
  try { found[driverId] = await api.invoke<string[]>("app_targets", { driver: driverId }); } catch (e) { found[driverId] = []; toast(String(e)); }
  reading.delete(driverId);
  if (page === "seats") void renderSeats();
}

/** 添加联系人: every service as one row; apps with Bots list them as chips to tap. */
async function addPanel(): Promise<HTMLDivElement> {
  const g = el("div", "group add-panel");
  g.append(el("div", "group__title", esc(t("add.open"))));
  // what can be called first, placeholders after
  for (const d of [...drivers].sort((a, b) => Number(a.pending) - Number(b.pending))) {
    const noun = d.target_noun ?? t("noun.target");
    const r = el("div", `row tile${d.installed ? "" : " is-off"}`);
    r.append(el("div", "avatar", await iconFor(iconKey(d.id))));
    const sub = !d.installed ? t("add.notInstalled")
      : d.pending ? t("add.placeholder")
      : d.kind === "web" ? t((settings.web_via_chrome ?? true) ? "add.webChrome" : "add.webBuiltin")
      : d.discover ? t("add.appTargets", { noun }) : t("add.app");
    const kind = `<span class="kind">${t(d.kind === "web" ? "kind.web" : "kind.app")}</span>${d.pending ? `<span class="kind soon">${t("kind.soon")}</span>` : ""}`;
    r.append(el("div", "row__text", `<div class="row__name">${esc(d.display_name)}${kind}</div><div class="row__sub">${esc(sub)}</div>`));
    const acts = el("div", "actions");
    if (d.discover) {
      const b = el("button", "btn sm", esc(reading.has(d.id) ? t("add.reading") : d.id in found ? t("add.reread") : t("add.read", { noun }))) as HTMLButtonElement;
      b.type = "button"; b.disabled = reading.has(d.id) || !d.installed;
      b.addEventListener("click", () => void readTargets(d.id));
      acts.append(b);
    } else {
      // one contact per site, and per placeholder app; real apps can hold several (dots)
      const added = (d.kind === "web" || d.pending) && seats.some((s) => s.driver.driver === d.id);
      const b = el("button", `btn sm${added ? "" : " tint"}`, esc(t(added ? "add.added" : "add.add"))) as HTMLButtonElement;
      b.type = "button"; b.disabled = added || !d.installed;
      b.addEventListener("click", () => void addSeat(d.id));
      acts.append(b);
    }
    r.append(acts);
    g.append(r);
    const names = found[d.id];
    if (d.discover && names && !reading.has(d.id)) {
      if (!names.length) { g.append(el("p", "note indent", esc(t("add.noneFound", { noun, app: d.display_name })))); continue; }
      const have = new Set(seats.filter((s) => s.driver.driver === d.id && s.target).map((s) => s.target));
      const chips = el("div", "chips");
      for (const n of names) {
        const added = have.has(n);
        const c = el("button", `btn sm${added ? "" : " tint"}`, esc(added ? `✓ ${n}` : n)) as HTMLButtonElement;
        c.type = "button"; c.disabled = added; c.title = added ? t("add.inDock") : t("add.toDock", { name: n });
        c.addEventListener("click", () => void addSeat(d.id, n));
        chips.append(c);
      }
      g.append(chips);
    }
  }
  return g;
}
const stripView = (s: SeatView): Seat => ({ id: s.id, name: s.name, service: s.service, driver: s.driver, icon: s.icon, enabled: s.enabled, status: s.status, account: s.account, target: s.target ?? null });

// ───────────── 快捷键 ─────────────
function keyChip(value: string, conflict: boolean, onChange: (v: string) => void): HTMLDivElement {
  const k = el("div", `key${conflict ? " is-conflict" : ""}`);
  const render = (v: string) => { k.innerHTML = v.split("+").map((p) => `<kbd>${esc(pretty(p))}</kbd>`).join(""); };
  render(value);
  k.tabIndex = 0; k.title = t("hk.recordTip");
  k.addEventListener("click", () => { k.classList.add("is-rec"); k.innerHTML = `<kbd>${esc(t("hk.recording"))}</kbd>`; k.focus(); });
  k.addEventListener("keydown", (e) => {
    if (!k.classList.contains("is-rec")) return;
    e.preventDefault();
    if (e.key === "Escape") { k.classList.remove("is-rec"); render(value); return; }
    if (["Shift", "Control", "Alt", "Meta"].includes(e.key)) return;
    const mods = [e.metaKey && "Super", e.ctrlKey && "Control", e.altKey && "Alt", e.shiftKey && "Shift"].filter(Boolean) as string[];
    const key = e.code.startsWith("Key") ? e.code.slice(3) : e.code.startsWith("Digit") ? e.code.slice(5) : e.key === " " ? "Space" : e.key.length === 1 ? e.key.toUpperCase() : e.key;
    const combo = [...mods, key].join("+");
    k.classList.remove("is-rec"); value = combo; render(combo); onChange(combo);
  });
  k.addEventListener("blur", () => { if (k.classList.contains("is-rec")) { k.classList.remove("is-rec"); render(value); } });
  return k;
}
const pretty = (p: string) => ({ Alt: host.os === "macos" ? "⌥" : "Alt", Control: host.os === "macos" ? "⌃" : "Ctrl", Shift: "⇧", Super: host.os === "macos" ? "⌘" : "Win", Space: t("key.space"), Escape: "Esc" } as Record<string, string>)[p] ?? p;

async function renderHotkeys() {
  const conflicts = await api.invoke<string[]>("hotkey_conflicts", { settings });
  const c = (k: string) => conflicts.includes(k.toLowerCase());
  const h = settings.hotkeys;
  const g = group(t("hk.global"),
    row(t("hk.toggle"), t("hk.toggleSub"), keyChip(h.toggle, c(h.toggle), (v) => saveSettings({ hotkeys: { ...settings.hotkeys, toggle: v } }).then(renderHotkeys))),
    row(t("hk.mute"), "", keyChip(h.mute, c(h.mute), (v) => saveSettings({ hotkeys: { ...settings.hotkeys, mute: v } }).then(renderHotkeys))),
    row(t("hk.hangup"), "", keyChip(h.hangup, c(h.hangup), (v) => saveSettings({ hotkeys: { ...settings.hotkeys, hangup: v } }).then(renderHotkeys))),
    row(t("hk.orientation"), "", keyChip(h.orientation, c(h.orientation), (v) => saveSettings({ hotkeys: { ...settings.hotkeys, orientation: v } }).then(renderHotkeys))),
    row(t("hk.form"), t("hk.formSub"), keyChip(h.form, c(h.form), (v) => saveSettings({ hotkeys: { ...settings.hotkeys, form: v } }).then(renderHotkeys))),
  );
  const seatMod = el("select") as HTMLSelectElement;
  for (const m of ["Alt", "Control", "Super", "Control+Alt", "Super+Shift"]) seatMod.append(new Option(`${m.split("+").map(pretty).join("")} + 1…9`, m, false, m === h.seat_modifier));
  seatMod.addEventListener("change", () => saveSettings({ hotkeys: { ...settings.hotkeys, seat_modifier: seatMod.value } }).then(renderHotkeys));
  const order = seats.filter((s) => s.enabled).map((s, i) => `${pretty(h.seat_modifier)}${i + 1} ${esc(s.name)}`).join(" · ");
  const g2 = group(t("hk.agents"), row(t("hk.callNth"), order || t("hk.noAgents"), seatMod));
  const g3 = group(t("hk.walkie"), row(t("hk.ptt"), t("hk.pttSub"), toggle(h.push_to_talk, (v) => saveSettings({ hotkeys: { ...settings.hotkeys, push_to_talk: v } }))));
  const note = el("p", "note", esc(conflicts.length ? t("hk.conflicts", { n: conflicts.length }) : t("hk.note")));
  pageEl.replaceChildren(g, g2, g3, note);
}

// ───────────── 外观 ─────────────
function renderAppearance() {
  // 语言: native names in the menu, so the right one is findable from any language
  const langSel = el("select") as HTMLSelectElement;
  const curLang = settings.language ?? "system";
  for (const [v, name] of LANGUAGES) langSel.append(new Option(v === "system" ? t("lang.system") : name, v, false, v === curLang));
  langSel.addEventListener("change", () => void chooseLanguage(langSel.value as LangSetting));
  const gLang = group("", row(lang() === "en" ? t("lang.label") : `${t("lang.label")} · Language`, t("lang.sub"), langSel));

  const cards = el("div", "cards");
  const forms: [AppSettings["default_form"], MsgKey, string][] = [
    ["dot", "form.dot", '<i class="d"></i>'],
    ["capsule", "form.capsule", '<i class="p"></i>'],
    ["expanded", "form.expanded", '<i class="p" style="width:26px"></i><i class="c"></i><i class="c"></i>'],
    ["pet", "form.pet", '<i class="pet"></i><i class="p" style="width:18px;height:4px;opacity:.5"></i>'],
  ];
  for (const [v, label, pre] of forms) {
    const c = el("div", `card${settings.default_form === v ? " is-on" : ""}`, `<div class="card__pre">${pre}</div><span>${esc(t(label))}</span>`);
    c.addEventListener("click", () => { cards.querySelectorAll(".card").forEach((x) => x.classList.remove("is-on")); c.classList.add("is-on"); saveSettings({ default_form: v }); });
    cards.append(c);
  }
  const gForm = el("div", "group"); gForm.append(el("div", "group__title", esc(t("form.title"))), cards,
    el("p", "note", esc(t("form.note"))));

  const matStrength = el("input") as HTMLInputElement; matStrength.type = "range"; matStrength.min = "0"; matStrength.max = "1"; matStrength.step = "0.05"; matStrength.value = String(settings.material_strength);
  matStrength.addEventListener("input", () => saveSettings({ material_strength: Number(matStrength.value) }));
  matStrength.addEventListener("change", () => saveSettings({ material_strength: Number(matStrength.value) }));
  const gTheme = group(t("color.title"),
    row(t("color.theme"), "", seg<AppSettings["theme"]>([["light", t("theme.light")], ["dark", t("theme.dark")], ["system", t("theme.system")]], settings.theme, (v) => saveSettings({ theme: v }))),
    row(t("color.accent"), t("color.accentSub"), accentPicker()),
    row(t("color.glass"), host.material ? (host.material === "NSGlassEffectView" ? "Liquid Glass" : host.material === "NSVisualEffectView" ? t("glass.visual") : `Windows ${host.material}`) : t("glass.fallback"), matStrength),
  );
  const snap = el("select") as HTMLSelectElement;
  for (const v of ["free", "top_center", "bottom_center", "top_left", "top_right", "bottom_left", "bottom_right"] as AppSettings["snap"][]) snap.append(new Option(t(`snap.${v}` as MsgKey), v, false, v === settings.snap));
  snap.addEventListener("change", () => { const v = snap.value as AppSettings["snap"]; saveSettings(v === "free" ? { snap: v } : { snap: v, dock_anchor: null }); });
  const curScale = Math.min(1.6, Math.max(0.7, settings.dock_scale ?? 1));
  const scaleLabel = el("span", "chip", `${Math.round(curScale * 100)}%`);
  const scaleRange = el("input") as HTMLInputElement; scaleRange.type = "range"; scaleRange.min = "0.7"; scaleRange.max = "1.6"; scaleRange.step = "0.05"; scaleRange.value = String(curScale);
  const scaleReset = el("button", "btn sm", esc(t("btn.reset"))); scaleReset.type = "button";
  const onScale = (v: number) => {
    const clamped = Math.round(Math.min(1.6, Math.max(0.7, v)) * 20) / 20;
    scaleRange.value = String(clamped);
    scaleLabel.textContent = `${Math.round(clamped * 100)}%`;
    saveSettings({ dock_scale: clamped });
  };
  scaleRange.addEventListener("input", () => onScale(Number(scaleRange.value)));
  scaleRange.addEventListener("change", () => onScale(Number(scaleRange.value)));
  scaleReset.addEventListener("click", () => onScale(1));
  const gLayout = group(t("layout.title"),
    row(t("layout.size"), t("layout.sizeSub"), [scaleLabel, scaleRange, scaleReset]),
    row(t("layout.orientation"), esc(t("layout.orientationSub", { keys: settings.hotkeys.orientation.split("+").map(pretty).join("") })), seg<AppSettings["orientation"]>([["horizontal", t("orient.horizontal")], ["vertical", t("orient.vertical")]], settings.orientation ?? "horizontal", (v) => saveSettings({ orientation: v }))),
    row(t("layout.position"), t("layout.positionSub"), snap),
    row(t("layout.collapse"), t("layout.collapseSub"), seg<AppSettings["collapse_mode"]>([["auto", t("collapse.auto")], ["pinned_expanded", t("collapse.pinnedExpanded")], ["pinned_collapsed", t("collapse.pinnedCollapsed")]], settings.collapse_mode ?? "auto", (v) => saveSettings({ collapse_mode: v }))),
    row(t("layout.autoCollapse"), t("layout.autoCollapseSub"), (() => { const sel = el("select") as HTMLSelectElement; for (const v of [0, 3, 8, 15, 30]) sel.append(new Option(v ? t("time.secs", { n: v }) : t("time.never"), String(v), false, v === settings.auto_collapse_secs)); sel.addEventListener("change", () => saveSettings({ auto_collapse_secs: Number(sel.value) })); return sel; })()),
    row(t("layout.names"), "", seg<AppSettings["show_names"]>([["hover", t("names.hover")], ["always", t("names.always")], ["never", t("names.never")]], settings.show_names, (v) => saveSettings({ show_names: v }))),
    row(t("layout.reduceMotion"), t("layout.reduceMotionSub"), toggle(settings.reduce_motion, (v) => saveSettings({ reduce_motion: v }))),
  );
  // macOS only: the app's own icon. Two places at most, never none, so settings stay reachable.
  const gIcon = group(t("icon.title"), row(t("icon.row"), t("icon.sub"),
    seg<NonNullable<AppSettings["app_icon"]>>([["both", t("icon.both")], ["dock", t("icon.dock")], ["menu_bar", t("icon.menuBar")]], settings.app_icon ?? "both", (v) => saveSettings({ app_icon: v }))));
  pageEl.replaceChildren(...[gLang, gForm, gTheme, gLayout, ...(host.os === "macos" ? [gIcon] : [])]);
}

// ───────────── 麦克风 ─────────────
/** Lets go of the level meter's microphone: it is held only while the page is on screen. */
let stopLevel = () => {};
document.addEventListener("visibilitychange", () => { if (document.hidden) stopLevel(); else if (page === "mic") void renderMic(); });
async function renderMic() {
  stopLevel();
  const dev = el("select") as HTMLSelectElement; dev.append(new Option(t("mic.default"), ""));
  try {
    const list = (await navigator.mediaDevices.enumerateDevices()).filter((d) => d.kind === "audioinput");
    for (const d of list) dev.append(new Option(d.label || t("mic.device", { n: dev.options.length }), d.deviceId, false, d.deviceId === settings.input_device));
  } catch { /* no devices on this host */ }
  dev.addEventListener("change", () => saveSettings({ input_device: dev.value || null }));
  const level = el("div", "level", "<i></i>");
  const g = group(t("mic.input"),
    row(t("mic.inputDevice"), t("mic.inputDeviceSub"), dev),
    row(t("mic.level"), t("mic.levelSub"), level),
  );
  const g2 = group(t("mic.call"),
    row(t("mic.muteMode"), t("mic.muteModeSub"), seg<AppSettings["mute_mode"]>([["system", t("mute.system")], ["seat", t("mute.seat")]], settings.mute_mode, (v) => saveSettings({ mute_mode: v }))),
    row(t("mic.pauseMedia"), t("mic.pauseMediaSub"), toggle(settings.pause_media_in_call, (v) => saveSettings({ pause_media_in_call: v }))),
    row(t("mic.confirmSwitch"), t("mic.confirmSwitchSub"), toggle(settings.confirm_switch, (v) => saveSettings({ confirm_switch: v }))),
  );
  if (page !== "mic") return;
  pageEl.replaceChildren(g, g2);
  if (document.hidden) return;
  let live = true, raf = 0, release = () => {};
  const stop = () => { live = false; cancelAnimationFrame(raf); release(); release = () => {}; };
  stopLevel = stop;
  try {
    const stream = await navigator.mediaDevices.getUserMedia({ audio: { ...METER_AUDIO, ...(settings.input_device ? { deviceId: { exact: settings.input_device } } : {}) } });
    const ctx = new AudioContext();
    release = () => { stream.getTracks().forEach((t) => t.stop()); void ctx.close(); };
    // left the page (or hid the window) while the microphone was coming up
    if (!live || !level.isConnected) { stop(); return; }
    const an = ctx.createAnalyser(); an.fftSize = 256; ctx.createMediaStreamSource(stream).connect(an);
    const buf = new Uint8Array(an.frequencyBinCount); const bar = level.firstElementChild as HTMLElement;
    const tick = () => { if (!level.isConnected) { stop(); return; } an.getByteTimeDomainData(buf); let peak = 0; for (const v of buf) peak = Math.max(peak, Math.abs(v - 128)); bar.style.width = `${Math.min(100, (peak / 128) * 160)}%`; raf = requestAnimationFrame(tick); };
    tick();
  } catch { (level.firstElementChild as HTMLElement).style.width = "0%"; level.title = t("mic.noPermission"); }
}

// ───────────── 记忆 ─────────────
/** The backend reports its memory status reason in Chinese (everos.rs); show it in the UI language. */
const MEMORY_REASONS: Record<string, MsgKey> = {
  "未设置 EVEROS_API_KEY": "mem.reason.noKey",
  "已在设置中关闭": "mem.reason.off",
  "EVEROS_ENABLED 已关闭": "mem.reason.envOff",
  "EVEROS_BASE_URL 不是 http(s) 地址": "mem.reason.badUrl",
  "已启用，呼叫时读写记忆": "mem.reason.active",
};
async function renderMemory() {
  const mem = settings.memory ?? { enabled: false, user_id: "", inject_context: true, top_k: 5 };
  let status: MemoryStatus = {
    active: false, enabled: mem.enabled, key_present: false, base_url: "https://api.evermind.ai",
    user_id: mem.user_id, app_id: "agentdock", project_id: "default",
    reason: "未设置 EVEROS_API_KEY", inject_context: mem.inject_context, top_k: mem.top_k,
  };
  try { status = await api.invoke<MemoryStatus>("memory_status"); } catch { /* mock-less hosts */ }
  const reason = MEMORY_REASONS[status.reason] ? t(MEMORY_REASONS[status.reason]) : status.reason;
  const chip = el("span", "chip", esc(t(status.active ? "mem.on" : "mem.offChip")));
  const g = group("EverOS",
    row(t("mem.enable"), t("mem.enableSub"), toggle(mem.enabled, (v) => { void saveSettings({ memory: { ...mem, enabled: v } }).then(() => { if (page === "memory") void renderMemory(); }); })),
    row(t("mem.inject"), t("mem.injectSub"), toggle(mem.inject_context, (v) => saveSettings({ memory: { ...settings.memory, inject_context: v } }))),
  );
  const g2 = group(t("mem.status"),
    row(t("mem.state"), esc(reason), chip),
    row(t("mem.key"), t(status.key_present ? "mem.keyFound" : "mem.keyMissing"), el("span", "chip", esc(t(status.key_present ? "mem.yes" : "mem.no")))),
    row(t("mem.service"), esc(status.base_url), el("span", "chip", esc(status.app_id))),
    row(t("mem.user"), status.user_id ? esc(status.user_id) : t("mem.userAuto"), el("span", "chip", "user_id")),
  );
  const note = el("p", "note", esc(t("mem.note")));
  pageEl.replaceChildren(g, g2, note);
}

// ───────────── 关于 ─────────────
function renderAbout() {
  const g = group("AgentDock",
    row(esc(t("about.version", { v: host.version })), esc(t("about.runsOn", { os: host.os, material: host.material ?? t("about.noMaterial") })), el("span", "chip", "Tauri 2 · Rust")),
    row(t("about.privacy"), t("about.privacySub"), el("span", "chip", esc(t("about.local")))),
  );
  const d = el("div", "group"); d.append(el("div", "group__title", esc(t("about.drivers"))));
  for (const dr of drivers) {
    const st = el("span", "status", esc(t(dr.verified ? "about.verified" : "about.unverified"))); st.dataset.s = dr.verified ? "connected" : "needs_auth";
    d.append(row(`${esc(dr.display_name)} <span class="chip">v${dr.version}</span> <span class="chip">${t(dr.kind === "web" ? "kind.web" : "kind.app")}</span>`, dr.id, st));
  }
  d.append(el("p", "note", esc(t("about.driversNote"))));
  const q = el("button", "btn danger", esc(t("about.quitBtn"))); q.addEventListener("click", () => api.invoke("quit"));
  pageEl.replaceChildren(g, d, group("", row(t("about.quit"), t("about.quitSub"), q)));
}

// ───────────── nav ─────────────
async function show(p: PageId) {
  stopLevel();
  page = p;
  document.querySelectorAll<HTMLButtonElement>(".nav__item").forEach((b) => b.classList.toggle("is-active", b.dataset.page === p));
  const [title, sub] = TITLES[p]; $("#title").textContent = t(title);
  $("#subtitle").textContent = t(sub, { mod: pretty("Alt") });
  if (p === "seats") await renderSeats(); else if (p === "hotkeys") await renderHotkeys(); else if (p === "appearance") renderAppearance(); else if (p === "mic") await renderMic(); else if (p === "memory") await renderMemory(); else renderAbout();
}
document.querySelectorAll<HTMLButtonElement>(".nav__item").forEach((b) => b.addEventListener("click", () => show(b.dataset.page as PageId)));
const langBtn = $("#lang-btn");
langBtn.addEventListener("click", (e) => { e.stopPropagation(); if (menuEl) closeMenu(); else openLangMenu(langBtn); });

async function boot() {
  host = await api.invoke<HostInfo>("host_info");
  // The CSS keys off `[data-vibrancy="1"]`; toggleAttribute() would set an empty value and never match.
  if (host.vibrancy) document.documentElement.setAttribute("data-vibrancy", "1"); else document.documentElement.removeAttribute("data-vibrancy");
  document.documentElement.dataset.os = host.os;
  // keep the slab concentric with the real window corner (26pt on macOS 26 with the unified toolbar)
  const setRadius = (r: number) => { if (r > 0) document.documentElement.style.setProperty("--win-r", `${r}px`); };
  setRadius(host.window_radius ?? 0);
  void api.on<number>("settings:radius", setRadius);
  $("#ver").textContent = host.version;
  settings = await api.invoke<AppSettings>("get_settings"); applySettingsLocally();
  [seats, drivers] = await Promise.all([api.invoke<SeatView[]>("get_seats"), api.invoke<DriverInfo[]>("list_drivers")]);
  const initial = (new URLSearchParams(location.search).get("page") as PageId) || "seats";
  await show(TITLES[initial] ? initial : "seats");
  api.on<SeatView[]>("seats:changed", (s) => {
    seats = s;
    if (sorting || interacting()) { deferredSeats = true; return; }
    if (page === "seats" || page === "hotkeys") show(page);
  });
  api.on<AppSettings>("settings:changed", (s) => {
    const before = lang();
    settings = s; applySettingsLocally();
    // switched from elsewhere (or "follow system" resolved differently): redraw in the new language
    if (lang() !== before && !interacting()) void show(page);
  });
}
boot();
