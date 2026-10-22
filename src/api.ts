import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { LangSetting, MsgKey } from "./i18n";

export type Phase =
  | { phase: "idle" }
  | { phase: "calling"; seat: string }
  | { phase: "in_call"; seat: string; elapsed_secs: number }
  | { phase: "switching"; from: string; to: string; undo_secs_left: number }
  | { phase: "needs_human"; seat: string; reason: string };

export interface DockState { phase: Phase; muted: boolean; expanded: boolean }

export type DriverKind =
  | { kind: "web"; url: string; driver: string }
  | { kind: "app"; bundle_id: string; driver: string };
export type SeatIcon = { kind: "builtin"; key: string } | { kind: "custom"; path: string };
export type SeatStatus = "connected" | "needs_auth" | "pending" | "unavailable" | "unknown";

export interface ProbeResult {
  in_call: boolean; muted: boolean; needs_human: boolean; logged_in: boolean;
  account: string | null; url: string | null;
}
export interface Seat {
  id: string; name: string; service: string; driver: DriverKind; icon: SeatIcon;
  enabled: boolean; status: SeatStatus; account: string | null;
  /** What to pick inside the service before calling, e.g. one Bot in Grok Bot. */
  target?: string | null;
}
export interface SeatView extends Seat { probe: ProbeResult | null; driver_verified: boolean; silent: boolean }

export type DefaultForm = "dot" | "capsule" | "expanded" | "pet";
export type Theme = "system" | "light" | "dark";
export type Snap = "free" | "top_center" | "bottom_center" | "top_left" | "top_right" | "bottom_left" | "bottom_right";
export type Orientation = "horizontal" | "vertical";
export type CollapseMode = "auto" | "pinned_expanded" | "pinned_collapsed";
export interface DockAnchor { x: number; y: number }
export interface Hotkeys { toggle: string; mute: string; hangup: string; orientation: string; form: string; seat_modifier: string; push_to_talk: boolean }
export interface MemorySettings {
  enabled: boolean; user_id: string; inject_context: boolean; top_k: number;
}
export interface MemoryStatus {
  active: boolean; enabled: boolean; key_present: boolean; base_url: string; user_id: string;
  app_id: string; project_id: string; reason: string; inject_context: boolean; top_k: number;
}
export interface AppSettings {
  default_form: DefaultForm; theme: Theme; snap: Snap; orientation: Orientation; dock_anchor: DockAnchor | null; auto_collapse_secs: number; collapse_mode: CollapseMode; show_names: "hover" | "always" | "never";
  reduce_motion: boolean; material_strength: number; dock_scale?: number; accent?: string | null; hotkeys: Hotkeys; mute_mode: "system" | "seat";
  pause_media_in_call: boolean; input_device: string | null; confirm_switch: boolean;
  web_via_chrome: boolean;
  /** macOS: where AgentDock's own icon lives; the Dock icon opens settings. */
  app_icon?: "both" | "dock" | "menu_bar";
  /** Interface language; "system" follows the OS (HostInfo.locale). */
  language?: LangSetting;
  memory: MemorySettings;
}
export interface ChromeStatus { available: boolean; connected: boolean }
export interface DriverInfo {
  id: string; display_name: string; version: number; verified: boolean; kind: "web" | "app";
  url: string | null; bundle_id: string | null; installed: boolean;
  /** The app can list what to call (its Bots). */ discover: boolean;
  /** A call picks something inside the service first. */ picks: boolean;
  target_noun: string | null;
  /** Placeholder: icon and contact only, calling not wired up yet. */ pending: boolean;
}
export interface HostInfo { os: string; vibrancy: boolean; material?: string; version: string; window_radius?: number; locale?: string }

const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/** Browser fallback so the UI can be developed and screenshotted outside Tauri. */
const mock = (() => {
  let state: DockState = { phase: { phase: "idle" }, muted: false, expanded: true };
  const listeners = new Map<string, Set<(p: unknown) => void>>();
  const emit = (ev: string, payload: unknown) => listeners.get(ev)?.forEach((f) => f(payload));
  const seats: SeatView[] = [
    { id: "dot", name: "dot", service: "ChatGPT dot", driver: { kind: "web", url: "https://chatgpt.com/", driver: "chatgpt-dot" }, icon: { kind: "builtin", key: "dot" }, enabled: true, status: "connected", account: "weijia", probe: null, driver_verified: false, silent: true },
    { id: "grok-bot", name: "Grok Bot", service: "Grok Bot", driver: { kind: "app", bundle_id: "com.anysphere.sand", driver: "grok-bot" }, icon: { kind: "builtin", key: "grok-bot" }, enabled: true, status: "needs_auth", account: null, probe: null, driver_verified: false, silent: false },
    { id: "copilot", name: "Copilot", service: "Copilot Voice", driver: { kind: "web", url: "https://copilot.microsoft.com/", driver: "copilot-voice" }, icon: { kind: "builtin", key: "copilot" }, enabled: true, status: "needs_auth", account: null, probe: null, driver_verified: false, silent: true },
    { id: "devin", name: "Devin", service: "Devin", driver: { kind: "web", url: "https://app.devin.ai/", driver: "devin-voice" }, icon: { kind: "builtin", key: "devin" }, enabled: false, status: "unknown", account: null, probe: null, driver_verified: false, silent: true },
    { id: "muse", name: "Muse", service: "Meta Muse", driver: { kind: "web", url: "https://muse.ai/", driver: "muse-voice" }, icon: { kind: "builtin", key: "muse" }, enabled: false, status: "pending", account: null, probe: null, driver_verified: false, silent: true },
  ];
  let settings: AppSettings = {
    default_form: "dot", theme: "system", snap: "top_center", orientation: "horizontal", dock_anchor: null, auto_collapse_secs: 8, collapse_mode: "auto", show_names: "hover", reduce_motion: false, material_strength: 0.5, dock_scale: 1, accent: null,
    hotkeys: { toggle: "Alt+Space", mute: "Alt+M", hangup: "Alt+Escape", orientation: "Alt+O", form: "Alt+P", seat_modifier: "Alt", push_to_talk: false },
    mute_mode: "system", pause_media_in_call: true, input_device: null, confirm_switch: false, web_via_chrome: true, app_icon: "both", language: "system",
    memory: { enabled: false, user_id: "", inject_context: true, top_k: 5 },
  };
  if (typeof window !== "undefined") (window as any).__mockSettings = settings;
  let started = 0; let timer: number | undefined; let undoTimer: number | undefined;
  const setState = (s: DockState) => { state = s; emit("dock:state", state); };
  const startCall = (seat: string) => {
    setState({ ...state, phase: { phase: "calling", seat }, expanded: true });
    window.setTimeout(() => {
      started = Date.now();
      setState({ ...state, phase: { phase: "in_call", seat, elapsed_secs: 0 } });
      window.clearInterval(timer);
      timer = window.setInterval(() => {
        if (state.phase.phase === "in_call") setState({ ...state, phase: { ...state.phase, elapsed_secs: Math.floor((Date.now() - started) / 1000) } });
      }, 1000);
    }, 1200);
  };
  const cmds: Record<string, (a: any) => unknown> = {
    get_state: () => state,
    get_seats: () => seats,
    get_settings: () => settings,
    save_settings: (a) => { settings = a.settings; (window as any).__mockSettings = settings; emit("settings:changed", settings); },
    save_seats: (a) => { seats.splice(0, seats.length, ...a.seats.map((s: Seat) => ({ ...s, probe: null, driver_verified: false, silent: s.driver.kind === "web" }))); emit("seats:changed", seats); },
    reorder_seats: (a) => { const ids: string[] = a.ids; seats.sort((x, y) => ids.indexOf(x.id) - ids.indexOf(y.id)); emit("seats:changed", seats); },
    list_drivers: () => {
      const web = (id: string, display_name: string, url: string) => ({ id, display_name, version: 1, verified: false, kind: "web", url, bundle_id: null, installed: true, discover: false, picks: false, target_noun: null, pending: false });
      const app = (id: string, display_name: string, bundle_id: string, discover: boolean, target_noun: string) => ({ id, display_name, version: 1, verified: false, kind: "app", url: null, bundle_id, installed: true, discover, picks: true, target_noun, pending: false });
      const soon = (id: string, display_name: string, bundle_id: string) => ({ ...app(id, display_name, bundle_id, false, ""), picks: false, target_noun: null, pending: true });
      return [
        soon("antigravity", "Antigravity", "com.google.antigravity"),
        app("chatgpt-app", "ChatGPT dot", "com.openai.codex", false, "dot"),
        web("chatgpt-dot", "ChatGPT dot", "https://chatgpt.com/"),
        soon("claude-app", "Claude", "com.anthropic.claudefordesktop"),
        web("copilot-voice", "Copilot Voice", "https://copilot.microsoft.com/"),
        soon("cursor", "Cursor", "com.todesktop.230313mzl4w4u92"),
        web("devin-voice", "Devin voice", "https://app.devin.ai/"),
        soon("gemini-app", "Gemini", "com.google.GeminiMacOS"),
        app("grok-bot", "Grok Bot", "com.anysphere.sand", true, "Bot"),
        web("muse-voice", "Meta Muse voice", "https://muse.ai/"),
      ];
    },
    call_seat: (a) => {
      const p = state.phase;
      if ((p.phase === "in_call" || p.phase === "calling") && p.seat !== a.seatId) {
        const from = p.seat; let left = 3;
        setState({ ...state, phase: { phase: "switching", from, to: a.seatId, undo_secs_left: left } });
        window.clearInterval(undoTimer);
        undoTimer = window.setInterval(() => {
          left -= 1;
          if (state.phase.phase !== "switching") { window.clearInterval(undoTimer); return; }
          if (left <= 0) { window.clearInterval(undoTimer); startCall(a.seatId); } else setState({ ...state, phase: { ...state.phase, undo_secs_left: left } });
        }, 1000);
      } else if (p.phase === "idle" || p.phase === "needs_human") startCall(a.seatId);
    },
    hangup: () => { window.clearInterval(timer); setState({ ...state, phase: { phase: "idle" }, muted: false }); },
    toggle_mute: () => { if (state.phase.phase !== "idle") setState({ ...state, muted: !state.muted }); },
    set_muted: (a) => { if (state.phase.phase !== "idle") setState({ ...state, muted: a.muted }); },
    toggle_expanded: () => setState({ ...state, expanded: !state.expanded }),
    set_expanded: (a) => setState({ ...state, expanded: a.expanded }),
    undo_switch: () => { if (state.phase.phase === "switching") { window.clearInterval(undoTimer); setState({ ...state, phase: { phase: "in_call", seat: state.phase.from, elapsed_secs: Math.floor((Date.now() - started) / 1000) } }); } },
    resize_dock: () => undefined,
    drag_dock: (a) => { const w = window as any; w.__mockDrags = (w.__mockDrags || 0) + 1; w.__mockGrab = { x: a.offsetX, y: a.offsetY }; },
    remember_dock_position: () => { (window as any).__mockAnchored = true; settings = { ...settings, snap: "free" }; (window as any).__mockSettings = settings; emit("settings:changed", settings); },
    ui_log: () => undefined,
    cursor_over_dock: () => false,
    toggle_orientation: () => { settings = { ...settings, orientation: settings.orientation === "vertical" ? "horizontal" : "vertical" }; (window as any).__mockSettings = settings; emit("settings:changed", settings); },
    toggle_collapsed_form: () => { settings = { ...settings, default_form: settings.default_form === "pet" ? "dot" : "pet" }; (window as any).__mockSettings = settings; emit("settings:changed", settings); },
    dock_menu: () => { (window as any).__mockMenuOpened = ((window as any).__mockMenuOpened || 0) + 1; },
    connect_account: (a) => { const s = seats.find((x) => x.id === a.seatId); if (s) { s.status = "connected"; s.account = "demo@example.com"; emit("seats:changed", seats); } return s?.driver.kind === "app" ? "app" : "webview"; },
    chrome_status: () => ({ available: false, connected: false }),
    app_targets: () => ["Analytics", "Radar", "Content", "Code"],
    disconnect_seat: (a) => { const s = seats.find((x) => x.id === a.seatId); if (s) { s.status = "needs_auth"; s.account = null; emit("seats:changed", seats); } },
    show_seat: () => undefined,
    test_call: () => new Promise((r) => setTimeout(() => r(true), 1500)),
    probe_seat: () => undefined,
    set_seat_icon: (a) => { const s = seats.find((x) => x.id === a.seatId); if (s) { s.icon = { kind: "custom", path: `data:${a.fileName}` }; (s as any)._data = a.dataBase64; emit("seats:changed", seats); } return "icons/x"; },
    reset_seat_icon: (a) => { const s = seats.find((x) => x.id === a.seatId); if (s) { s.icon = { kind: "builtin", key: s.id }; emit("seats:changed", seats); } },
    seat_icon_data: (a) => { const s: any = seats.find((x) => x.id === a.seatId); return s?._data ? `data:image/png;base64,${s._data}` : null; },
    hotkey_conflicts: (a) => { const h = a.settings.hotkeys; const all = [h.toggle, h.mute, h.hangup, h.orientation, h.form].map((k: string) => k.toLowerCase()); return all.filter((k: string, i: number) => all.indexOf(k) !== i); },
    open_settings: () => window.open("/settings.html", "_blank"),
    quit: () => undefined,
    // ?os=macos previews the macOS-only rows (screenshots)
    // ?lang=ja picks the "system" language for screenshots; the mock defaults to Chinese like the tests
    host_info: () => ({ os: new URLSearchParams(location.search).get("os") ?? "browser", vibrancy: false, version: "0.1.0-dev", locale: new URLSearchParams(location.search).get("lang") ?? "zh-CN" }),
    glass_scheme: () => null,
    memory_status: () => ({
      active: false, enabled: !!settings.memory?.enabled, key_present: false,
      base_url: "https://api.evermind.ai", user_id: settings.memory?.user_id || "",
      app_id: "agentdock", project_id: "default", reason: settings.memory?.enabled ? "未设置 EVEROS_API_KEY" : "已在设置中关闭",
      inject_context: settings.memory?.inject_context !== false, top_k: settings.memory?.top_k || 5,
    }),
  };
  return {
    invoke: async <T,>(cmd: string, args: any = {}): Promise<T> => cmds[cmd]?.(args) as T,
    listen: async (ev: string, cb: (e: { payload: unknown }) => void): Promise<UnlistenFn> => {
      const f = (p: unknown) => cb({ payload: p });
      if (!listeners.has(ev)) listeners.set(ev, new Set());
      listeners.get(ev)!.add(f);
      return () => { listeners.get(ev)!.delete(f); };
    },
  };
})();

if (!inTauri && typeof window !== "undefined") {
  // test hook for tests/e2e_mock.py (a dynamic import would get a fresh module instance under Vite HMR)
  (window as any).__mockInvoke = (cmd: string, args?: Record<string, unknown>) => mock.invoke(cmd, args);
}

export const api = {
  isTauri: inTauri,
  invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
    return inTauri ? invoke<T>(cmd, args) : mock.invoke<T>(cmd, args);
  },
  on<T>(event: string, cb: (payload: T) => void): Promise<UnlistenFn> {
    return inTauri
      ? listen<T>(event, (e) => cb(e.payload))
      : mock.listen(event, (e) => cb(e.payload as T));
  },
};

export const fmtTime = (s: number) => `${String(Math.floor(s / 60)).padStart(2, "0")}:${String(s % 60).padStart(2, "0")}`;

/** Level meters want the raw input: echo cancellation would switch on macOS voice processing for
 *  the microphone, which ducks other apps' audio and is no use to a meter. */
export const METER_AUDIO: MediaTrackConstraints = { echoCancellation: false, noiseSuppression: false, autoGainControl: false };

/** Theme-colour presets (设置 › 外观 › 主题色). `null` = untinted system glass. */
export const ACCENTS: [string | null, MsgKey][] = [
  [null, "accent.none"], ["#0a84ff", "accent.blue"], ["#5e5ce6", "accent.indigo"], ["#bf5af2", "accent.purple"], ["#ff375f", "accent.pink"],
  ["#ff9f0a", "accent.orange"], ["#30d158", "accent.green"], ["#64d2ff", "accent.teal"], ["#8e8e93", "accent.graphite"],
];
export const validAccent = (c: string | null | undefined): string | null => (c && /^#[0-9a-f]{6}$/i.test(c) ? c : null);

export function applyTheme(theme: Theme) {
  const root = document.documentElement;
  if (theme === "system") root.removeAttribute("data-theme"); else root.setAttribute("data-theme", theme);
}
