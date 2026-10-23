//! AgentDock core: wires the pure state machine, seats, drivers, mic and hotkeys into Tauri.

pub mod drivers;
pub mod everos;
pub mod hotkeys;
pub mod i18n;
pub mod memory;
pub mod mic;
pub mod seats;
pub mod settings;
pub mod state;

use drivers::{app::AppDriver, web, DriverError, ProbeResult, Registry};
use hotkeys::HotkeyAction;
use seats::{DriverKind, Seat, SeatIcon, SeatStatus, SeatsConfig};
use serde::{Deserialize, Serialize};
use settings::AppSettings;
use state::{DockState, Effect, Event, Machine, Phase};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, Runtime, WebviewWindow};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

pub struct AppState {
    pub machine: Mutex<Machine>,
    pub seats: Mutex<SeatsConfig>,
    pub settings: Mutex<AppSettings>,
    pub registry: Registry,
    pub mic: mic::Mic,
    pub config_dir: PathBuf,
    pub probes: Mutex<HashMap<String, ProbeResult>>,
    pub hotkey_map: Mutex<Vec<(Shortcut, HotkeyAction)>>,
    /// Seats whose login window is currently shown; auto-hidden once a probe says logged in.
    pub logging_in: Mutex<std::collections::HashSet<String>>,
    /// Held while drivers call / hang up / mute, so those presses never interleave.
    pub driver_lane: Mutex<()>,
    /// Authoritative logical size of the dock pill so drags never inherit an OS-resized frame.
    pub dock_size: Mutex<(f64, f64)>,
    /// Open EverOS sessions, keyed by seat. Pure bookkeeping; HTTP happens off this lock.
    pub memory: Mutex<memory::CallBook>,
    /// `EVEROS_API_KEY` + settings. Rebuilt when settings are saved. Missing key → inactive.
    pub everos: Mutex<everos::Client>,
    /// The connection to the user's Chrome, opened on the first web call that goes there.
    pub chrome: drivers::chrome::Chrome,
}

#[derive(Debug, Clone, Serialize)]
pub struct SeatView {
    #[serde(flatten)]
    pub seat: Seat,
    pub probe: Option<ProbeResult>,
    pub driver_verified: bool,
    pub silent: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DriverInfo {
    pub id: String,
    pub display_name: String,
    pub version: u32,
    pub verified: bool,
    pub kind: String,
    pub url: Option<String>,
    pub bundle_id: Option<String>,
    /// App drivers: whether the app is on this Mac. Web drivers: always true.
    pub installed: bool,
    /// The app can list what to call (Bots), see `app_targets`.
    pub discover: bool,
    /// The call picks something inside the service (`{target}` in its steps).
    pub picks: bool,
    /// What a target is called in the UI ("Bot", "dot").
    pub target_noun: Option<String>,
    /// Placeholder: icon and contact only, calling not wired up yet.
    pub pending: bool,
}

type CmdResult<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

// ───────────────────────── state & events ─────────────────────────

fn emit_state<R: Runtime>(app: &AppHandle<R>, st: &AppState) {
    let snapshot = st.machine.lock().unwrap().state.clone();
    let _ = app.emit("dock:state", &snapshot);
}

fn emit_seats<R: Runtime>(app: &AppHandle<R>, st: &AppState) {
    let _ = app.emit("seats:changed", &seat_views(st));
}

fn seat_views(st: &AppState) -> Vec<SeatView> {
    let seats = st.seats.lock().unwrap();
    let probes = st.probes.lock().unwrap();
    seats
        .seats
        .iter()
        .map(|s| {
            let driver_id = match &s.driver {
                DriverKind::Web { driver, .. } | DriverKind::App { driver, .. } => driver.as_str(),
            };
            SeatView {
                seat: s.clone(),
                probe: probes.get(&s.id).cloned(),
                driver_verified: st.registry.get(driver_id).map(|d| d.verified).unwrap_or(false),
                silent: matches!(s.driver, DriverKind::Web { .. }),
            }
        })
        .collect()
}

fn dispatch<R: Runtime>(app: &AppHandle<R>, ev: Event) {
    let st = app.state::<AppState>();
    let before = st.machine.lock().unwrap().state.phase.clone();
    let effects = st.machine.lock().unwrap().handle(ev);
    let after = st.machine.lock().unwrap().state.phase.clone();
    emit_state(app, &st);
    memory_on_phase(app, &before, &after);
    let (driver_work, rest): (Vec<Effect>, Vec<Effect>) = effects.into_iter().partition(|fx| matches!(fx, Effect::DriverCall(_) | Effect::DriverHangup(_)));
    for fx in rest {
        apply_effect(app, fx);
    }
    run_driver_effects(app, driver_work);
}

/// One event's driver work runs on one blocking task, in the machine's order, and one event at a
/// time: a switch hangs up before it dials (most apps hold a single call), and a hang-up pressed
/// while a call is being placed lands after it instead of racing it.
fn run_driver_effects<R: Runtime>(app: &AppHandle<R>, effects: Vec<Effect>) {
    if effects.is_empty() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let st = app.state::<AppState>();
        let _lane = st.driver_lane.lock().unwrap_or_else(|e| e.into_inner());
        for fx in effects {
            match fx {
                Effect::DriverCall(seat) => place_call(&app, seat),
                Effect::DriverHangup(seat) => {
                    if let Err(e) = run_driver(&app, &seat, DriverOp::Hangup) {
                        tracing::warn!("hangup {seat} failed: {e}");
                    }
                }
                Effect::SystemMute(_) => {}
            }
        }
    });
}

/// Place a call; a failure turns into "needs you" with a reason a person can act on.
fn place_call<R: Runtime>(app: &AppHandle<R>, seat: String) {
    let Err(e) = run_driver(app, &seat, DriverOp::Call) else { return };
    tracing::warn!("call {seat} failed: {e}");
    let (name, service, web, target) = app.state::<AppState>().seats.lock().unwrap().get(&seat)
        .map(|s| (s.name.clone(), s.service.clone(), matches!(s.driver, DriverKind::Web { .. }), s.target.clone()))
        .unwrap_or_else(|| (seat.clone(), seat.clone(), false, None));
    let reason = match e {
        DriverError::NeedsAuth => i18n::t("call.needs_auth"),
        DriverError::NotFound(_) if web => i18n::tf("call.web_not_found", &[("name", &name)]),
        DriverError::NotFound(ref m) if target.as_deref().is_some_and(|t| m.contains(t)) => {
            i18n::tf("call.target_not_found", &[("service", &service), ("target", &target.unwrap_or_default())])
        }
        DriverError::NotFound(_) => i18n::tf("call.voice_not_found", &[("name", &name), ("service", &service)]),
        DriverError::Chrome(m) | DriverError::NotCallable(m) => m,
        other => i18n::tf("call.failed", &[("error", &other.to_string())]),
    };
    dispatch(app, Event::ProbeNeedsHuman { seat, reason });
}

/// Search before the driver places the call, and write turns when the phase says the call
/// connected or ended. EverOS failures are logged inside the spawned tasks; they never change
/// the effects the state machine already produced.
fn memory_on_phase<R: Runtime>(app: &AppHandle<R>, before: &Phase, after: &Phase) {
    let st = app.state::<AppState>();
    let client = st.everos.lock().unwrap().clone();
    if !client.is_active() {
        return;
    }
    let user_id = client.user_id().to_string();
    let actions = {
        let mut book = st.memory.lock().unwrap();
        memory::on_phase(
            &mut book,
            before,
            after,
            |id| {
                st.seats.lock().unwrap().get(id).map(|s| memory::SeatMeta {
                    name: s.name.clone(),
                    service: s.service.clone(),
                })
            },
            memory::now_ms(),
            &user_id,
        )
    };
    for action in actions {
        match action {
            memory::Action::Recall(recall) => {
                let app = app.clone();
                let client = client.clone();
                tauri::async_runtime::spawn(async move { recall_and_inject(app, client, recall).await });
            }
            memory::Action::Write(batch) => {
                let client = client.clone();
                tauri::async_runtime::spawn(async move { write_batch(client, batch).await });
            }
        }
    }
}

async fn recall_and_inject<R: Runtime>(app: AppHandle<R>, client: everos::Client, recall: memory::Recall) {
    let top_k = client.status().top_k;
    let query = recall.query.clone();
    let client_for_call = client.clone();
    let brief = match tokio::task::spawn_blocking(move || client_for_call.recall(&query, top_k)).await {
        Ok(Ok(text)) => text,
        Ok(Err(e)) => {
            tracing::warn!("everos search failed (call continues): {e}");
            return;
        }
        Err(e) => {
            tracing::warn!("everos search task failed (call continues): {e}");
            return;
        }
    };
    tracing::info!("everos recall for {}: {} chars", recall.seat_id, brief.chars().count());
    if brief.is_empty() || !client.status().inject_context {
        return;
    }
    let selectors = {
        let st = app.state::<AppState>();
        let driver_id = {
            let seats = st.seats.lock().unwrap();
            match seats.get(&recall.seat_id).map(|s| &s.driver) {
                Some(DriverKind::Web { driver, .. }) => driver.clone(),
                _ => return,
            }
        };
        st.registry.get(&driver_id).map(|spec| spec.memory.composer.clone()).unwrap_or_default()
    };
    let script = drivers::inject_memory_script(&brief, &selectors);
    // A Chrome tab only shows up once the call steps (which hold the Chrome line) are done.
    for _ in 0..200 {
        if let Some(win) = app.get_webview_window(&web::window_label(&recall.seat_id)) {
            if let Err(e) = win.eval(&script) {
                tracing::warn!("everos inject failed (call continues): {e}");
            }
            return;
        }
        if app.state::<AppState>().chrome.has_tab(&recall.seat_id) {
            let (app, seat, script) = (app.clone(), recall.seat_id.clone(), script.clone());
            let r = tokio::task::spawn_blocking(move || app.state::<AppState>().chrome.eval(&seat, "", &script, false)).await;
            if let Ok(Err(e)) = r {
                tracing::warn!("everos inject failed (call continues): {e}");
            }
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tracing::info!("everos recall for {} had no webview to inject into", recall.seat_id);
}

async fn write_batch(client: everos::Client, batch: memory::Persist) {
    if !batch.messages.is_empty() {
        let client_for_add = client.clone();
        let session_id = batch.session_id.clone();
        let messages = batch.messages.clone();
        match tokio::task::spawn_blocking(move || client_for_add.add(&session_id, &messages)).await {
            Ok(Ok(status)) => tracing::info!("everos add {} ({} msgs): {status}", batch.session_id, batch.messages.len()),
            Ok(Err(e)) => tracing::warn!("everos add failed (call continues): {e}"),
            Err(e) => tracing::warn!("everos add task failed (call continues): {e}"),
        }
    }
    if batch.flush {
        // A flush issued in the same instant as a queued add sees an empty buffer (`no_extraction`).
        // Give the write a moment to land; a miss is still not a failed call.
        tokio::time::sleep(memory::FLUSH_DELAY).await;
        let session_id = batch.session_id.clone();
        match tokio::task::spawn_blocking(move || client.flush(&session_id)).await {
            Ok(Ok(status)) => tracing::info!("everos flush {}: {status}", batch.session_id),
            Ok(Err(e)) => tracing::warn!("everos flush failed (call continues): {e}"),
            Err(e) => tracing::warn!("everos flush task failed (call continues): {e}"),
        }
    }
}

fn apply_effect<R: Runtime>(app: &AppHandle<R>, fx: Effect) {
    let app = app.clone();
    match fx {
        Effect::SystemMute(m) => {
            let st = app.state::<AppState>();
            if st.settings.lock().unwrap().mute_mode == settings::MuteMode::System {
                if let Err(e) = st.mic.set_muted(m) {
                    tracing::warn!("system mute failed: {e}");
                }
            }
            let current = st.machine.lock().unwrap().current_seat().map(str::to_string);
            drop(st);
            if let Some(seat) = current {
                tauri::async_runtime::spawn_blocking(move || {
                    let st = app.state::<AppState>();
                    let _lane = st.driver_lane.lock().unwrap_or_else(|e| e.into_inner());
                    let _ = run_driver(&app, &seat, if m { DriverOp::Mute } else { DriverOp::Unmute });
                });
            }
        }
        // Drivers block (AX walks, Chrome round trips, waiting for the user to allow Chrome).
        Effect::DriverCall(_) | Effect::DriverHangup(_) => run_driver_effects(&app, vec![fx]),
    }
}

#[derive(Debug, Clone, Copy)]
enum DriverOp {
    Call,
    Hangup,
    Mute,
    Unmute,
    Probe,
}

fn run_driver<R: Runtime>(app: &AppHandle<R>, seat_id: &str, op: DriverOp) -> Result<(), DriverError> {
    let st = app.state::<AppState>();
    let seat = st.seats.lock().unwrap().get(seat_id).cloned().ok_or_else(|| DriverError::Unknown(seat_id.into()))?;
    let driver_id = match &seat.driver {
        DriverKind::Web { driver, .. } | DriverKind::App { driver, .. } => driver,
    };
    // Placeholders: nothing to probe or press yet; a call says so instead of failing somewhere.
    if let Some(spec) = st.registry.get(driver_id).ok().filter(|d| d.pending) {
        return match op {
            DriverOp::Call => Err(DriverError::NotCallable(i18n::tf("call.pending", &[("name", &spec.display_name)]))),
            _ => Ok(()),
        };
    }
    match &seat.driver {
        DriverKind::Web { driver, url } => {
            let spec = st.registry.get(driver)?;
            if use_chrome(&st) {
                return chrome_op(app, &st.chrome, seat_id, spec, spec.url.as_deref().unwrap_or(url), op);
            }
            let win = web::ensure_webview(app, seat_id, spec)?;
            match op {
                DriverOp::Call => web::run_steps(&win, &spec.call),
                DriverOp::Hangup => web::run_steps(&win, &spec.hangup),
                DriverOp::Mute => web::run_steps(&win, &spec.mute),
                DriverOp::Unmute => web::run_steps(&win, &spec.unmute),
                DriverOp::Probe => web::probe(&win, spec, seat_id),
            }
        }
        DriverKind::App { driver, bundle_id } => {
            let spec = st.registry.get(driver)?;
            if !AppDriver::is_installed(bundle_id) {
                set_seat_status(app, seat_id, SeatStatus::Unavailable);
                return Err(DriverError::Unsupported);
            }
            if let (DriverOp::Call, Some(why)) = (op, seat.target.as_deref().and_then(|t| AppDriver::refusal(spec, t))) {
                return Err(DriverError::NotCallable(why));
            }
            let target_id = match op {
                DriverOp::Call => seat.target.as_deref().and_then(|t| AppDriver::target_id(spec, t)),
                _ => None,
            };
            let steps = |s: &[drivers::Step]| drivers::bind_target(s, seat.target.as_deref(), target_id.as_deref());
            let res = match op {
                DriverOp::Call => AppDriver::place_call(spec, &steps(&spec.call)),
                DriverOp::Hangup => AppDriver::hang_up(spec, &steps(&spec.hangup)),
                DriverOp::Mute => AppDriver::run_steps(spec, &steps(&spec.mute)),
                DriverOp::Unmute => AppDriver::run_steps(spec, &steps(&spec.unmute)),
                DriverOp::Probe => AppDriver::probe(spec).map(|r| {
                    ingest_probe(app, seat_id, r);
                }),
            };
            if matches!(res, Err(DriverError::NeedsAuth)) {
                set_seat_status(app, seat_id, SeatStatus::NeedsAuth);
            }
            res
        }
    }
}

/// Web seats go to the user's Chrome when remote debugging is on there (and the setting allows),
/// or once a Chrome connection exists; otherwise to AgentDock's own hidden webview.
fn use_chrome(st: &AppState) -> bool {
    st.settings.lock().unwrap().web_via_chrome && (st.chrome.is_connected() || drivers::chrome::available())
}

/// The same driver scripts, run in the seat's own background tab in Chrome. Only a call opens the
/// tab (and, the first time, the connection Chrome asks the user to allow); the rest use it if it
/// exists. Hang up / mute wait for a running call script; a probe skips a beat instead.
fn chrome_op<R: Runtime>(app: &AppHandle<R>, chrome: &drivers::chrome::Chrome, seat_id: &str, spec: &drivers::DriverSpec, url: &str, op: DriverOp) -> Result<(), DriverError> {
    let steps = match op {
        DriverOp::Call => return chrome.eval(seat_id, url, &drivers::steps_script(&spec.call), true).map(drop),
        DriverOp::Hangup => &spec.hangup,
        DriverOp::Mute => &spec.mute,
        DriverOp::Unmute => &spec.unmute,
        DriverOp::Probe => {
            if !chrome.has_tab(seat_id) {
                return Ok(());
            }
            let v = chrome.eval(seat_id, url, &drivers::probe_script(&spec.probe, seat_id), false)?;
            let r = serde_json::from_value(v).map_err(|e| DriverError::Step(e.to_string()))?;
            ingest_probe(app, seat_id, r);
            return Ok(());
        }
    };
    if steps.is_empty() {
        return Ok(());
    }
    chrome.eval(seat_id, url, &drivers::steps_script(steps), false).map(drop)
}

fn set_seat_status<R: Runtime>(app: &AppHandle<R>, seat_id: &str, status: SeatStatus) {
    let st = app.state::<AppState>();
    let changed = {
        let mut seats = st.seats.lock().unwrap();
        match seats.get_mut(seat_id) {
            Some(s) if s.status != status => {
                s.status = status;
                true
            }
            _ => false,
        }
    };
    if changed {
        persist_seats(&st);
        emit_seats(app, &st);
    }
}

fn persist_seats(st: &AppState) {
    let seats = st.seats.lock().unwrap();
    if let Err(e) = seats.save(&seats::seats_path(&st.config_dir)) {
        tracing::warn!("saving seats failed: {e}");
    }
}

/// A probe result arrived (from a hidden webview via IPC, or from an app driver).
fn ingest_probe<R: Runtime>(app: &AppHandle<R>, seat_id: &str, result: ProbeResult) {
    let st = app.state::<AppState>();
    st.probes.lock().unwrap().insert(seat_id.to_string(), result.clone());

    let was_pending = st.seats.lock().unwrap().get(seat_id).map(|s| s.status == SeatStatus::Pending).unwrap_or(false);
    let new_status = if result.logged_in {
        SeatStatus::Connected
    } else if was_pending {
        SeatStatus::Pending
    } else {
        SeatStatus::NeedsAuth
    };
    let changed = {
        let mut seats = st.seats.lock().unwrap();
        match seats.get_mut(seat_id) {
            Some(s) => {
                let mut changed = false;
                if s.status != new_status && !(was_pending && !result.logged_in) {
                    s.status = new_status;
                    changed = true;
                }
                if result.account.is_some() && s.account != result.account {
                    s.account = result.account.clone();
                    changed = true;
                }
                changed
            }
            None => false,
        }
    };
    // Only a real change goes out: every `seats:changed` re-renders the settings list, and a
    // probe every few seconds would close an open dropdown or menu under the user's pointer.
    if changed {
        persist_seats(&st);
        emit_seats(app, &st);
    }

    // Login window auto-hides once the page reports a session.
    if result.logged_in && st.logging_in.lock().unwrap().remove(seat_id) {
        if let Some(w) = app.get_webview_window(&web::window_label(seat_id)) {
            let _ = web::hide(&w);
        }
    }

    let _ = app.emit("seat:probe", serde_json::json!({ "seatId": seat_id, "result": result }));

    if result.needs_human {
        dispatch(app, Event::ProbeNeedsHuman { seat: seat_id.to_string(), reason: i18n::t("probe.needs_human") });
    } else {
        dispatch(app, Event::ProbeInCall { seat: seat_id.to_string(), in_call: result.in_call });
    }
}

// ───────────────────────── commands: dock ─────────────────────────

#[tauri::command]
fn get_state(st: tauri::State<AppState>) -> DockState {
    st.machine.lock().unwrap().state.clone()
}

#[tauri::command]
fn call_seat(app: AppHandle, seat_id: String) {
    dispatch(&app, Event::Call(seat_id));
}

#[tauri::command]
fn hangup(app: AppHandle) {
    dispatch(&app, Event::Hangup);
}

#[tauri::command]
fn toggle_mute(app: AppHandle) {
    dispatch(&app, Event::ToggleMute);
}

#[tauri::command]
fn set_muted(app: AppHandle, muted: bool) {
    dispatch(&app, Event::SetMuted(muted));
}

#[tauri::command]
fn toggle_expanded(app: AppHandle) {
    dispatch(&app, Event::ToggleExpanded);
}

#[tauri::command]
fn set_expanded(app: AppHandle, expanded: bool) {
    dispatch(&app, Event::SetExpanded(expanded));
}

#[tauri::command]
fn undo_switch(app: AppHandle) {
    dispatch(&app, Event::Undo);
}

/// The dock window resizes itself to the exact glass shape so the rest of the screen stays
/// click-through; it stays anchored to the configured snap edge. With `animate` on macOS the
/// window frame springs to the new shape and the native glass follows it (dot ↔ pill morph).
#[tauri::command]
fn resize_dock(app: AppHandle, width: f64, height: f64, animate: Option<bool>) -> CmdResult<()> {
    let win = app.get_webview_window("dock").ok_or("no dock window")?;
    let (snap, anchor) = {
        let st = app.state::<AppState>();
        *st.dock_size.lock().unwrap() = (width, height);
        let s = st.settings.lock().unwrap();
        (s.snap, s.dock_anchor)
    };
    #[cfg(target_os = "macos")]
    {
        if animate.unwrap_or(false) {
            let (x, y) = dock_origin(snap, anchor, width, height, monitor_rect(&win).map_err(err)?);
            return morph::animate_to(&win, [x, y, width, height]).map_err(err);
        }
        morph::cancel();
        // AppKit setters only take effect on the main thread; commands run on the async runtime.
        // Always half of the shorter side so scaled-up pills (e.g. 1.5x → 84pt tall, radius 42pt)
        // stay true semicircles at both ends and never flatten into a box.
        let radius = width.min(height) / 2.0;
        let _ = app.run_on_main_thread(move || liquid_glass::set_corner_radius(radius));
    }
    #[cfg(not(target_os = "macos"))]
    let _ = animate;
    position_dock(&win, width, height, snap, anchor).map_err(err)
}

/// Webview-side errors and diagnostics land in the Rust log (there is no console on a hidden pill).
#[tauri::command]
fn ui_log(level: String, msg: String) {
    match level.as_str() {
        "error" => tracing::error!(target: "agentdock_lib::ui", "{msg}"),
        "warn" => tracing::warn!(target: "agentdock_lib::ui", "{msg}"),
        _ => tracing::info!(target: "agentdock_lib::ui", "{msg}"),
    }
}

/// One frame of a user drag: put the window so the point grabbed at pointer-down (`offset`, logical px
/// inside the window) stays under the cursor. The OS cursor position is used instead of webview
/// screenX/Y, which lag behind while the window itself is moving (double-counted on WebKitGTK).
#[tauri::command]
fn drag_dock(app: AppHandle, offset_x: f64, offset_y: f64) -> CmdResult<()> {
    let win = app.get_webview_window("dock").ok_or("no dock window")?;
    #[cfg(target_os = "macos")]
    morph::cancel();
    let sc = win.scale_factor().map_err(err)?;
    let cursor = app.cursor_position().map_err(err)?;
    let (x, y) = (cursor.x / sc - offset_x, cursor.y / sc - offset_y);
    // The webview keeps calling `resize_dock` while the pointer moves (hover/status re-layout), and
    // that re-applies the *stored* anchor, yanking the window back mid-drag — whichever call landed
    // last won, so some drags silently ended where they started. Track the live centre in memory
    // (not on disk) so any concurrent `position_dock` resolves to the point under the cursor.
    let (w, h) = {
        let st = app.state::<AppState>();
        let (w, h) = *st.dock_size.lock().unwrap();
        let mut s = st.settings.lock().unwrap();
        s.snap = settings::Snap::Free;
        s.dock_anchor = Some(settings::DockAnchor { x: x + w / 2.0, y: y + h / 2.0 });
        (w, h)
    };
    let size = win.outer_size().map_err(err)?;
    if ((size.width as f64 / sc) - w).abs() > 1.0 || ((size.height as f64 / sc) - h).abs() > 1.0 {
        let _ = win.set_size(tauri::LogicalSize::new(w, h));
    }
    win.set_position(tauri::LogicalPosition::new(x, y)).map_err(err)
}

/// Is the OS cursor inside the dock window? Used by the auto-collapse countdown (webview hover
/// state goes stale when the window grows under the pointer).
#[tauri::command]
fn cursor_over_dock(app: AppHandle) -> bool {
    cursor_in_dock(&app)
}

/// Compare in *logical* (point) coordinates: tao reports the cursor in physical pixels of the
/// primary monitor but the window in physical pixels of *its own* monitor, so on a mixed-DPI
/// multi-display setup the raw numbers disagree by the scale ratio. Both logical values share the
/// same top-left-of-main-display origin, so this is exact on every screen.
fn cursor_in_dock(app: &AppHandle) -> bool {
    cursor_local(app).is_some_and(|(_, _, inside)| inside)
}

/// The OS cursor relative to the dock window's top-left (logical px = CSS px), and whether it is inside.
fn cursor_local(app: &AppHandle) -> Option<(f64, f64, bool)> {
    let win = app.get_webview_window("dock")?;
    let (c, p, s, ws) = (app.cursor_position().ok()?, win.outer_position().ok()?, win.outer_size().ok()?, win.scale_factor().ok()?);
    let cs = app.primary_monitor().ok().flatten().map(|m| m.scale_factor()).unwrap_or(ws);
    let (cx, cy) = (c.x / cs, c.y / cs);
    let (x, y) = (p.x as f64 / ws, p.y as f64 / ws);
    let (w, h) = (s.width as f64 / ws, s.height as f64 / ws);
    Some((cx - x, cy - y, cx >= x && cy >= y && cx < x + w && cy < y + h))
}

/// True on the first sample after entering the dock, or once the cursor has moved > 0.25 logical px.
fn pointer_moved(last: Option<(f64, f64)>, x: f64, y: f64) -> bool {
    last.map_or(true, |(lx, ly)| (x - lx).abs() > 0.25 || (y - ly).abs() > 0.25)
}

/// WKWebView only receives mouse-moved/entered/exited while its window is key, and the Dock is
/// never key by design (`focus: false`): parked next to another app it gets no `:hover`, no
/// pointerenter/leave. Poll the OS cursor and tell the dock when it enters or leaves the window
/// rect; while it is inside, stream the local position at ~60 Hz so the liquid lens can follow it.
fn spawn_hover_watcher(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut over = false;
        let mut last: Option<(f64, f64)> = None;
        loop {
            tokio::time::sleep(Duration::from_millis(if over { 14 } else { 40 })).await;
            let Some((x, y, inside)) = cursor_local(&app) else { continue };
            if inside != over {
                over = inside;
                if !over {
                    last = None;
                }
                let _ = app.emit("dock:hover", over);
                // AppKit may rebuild the glass layers behind our back (appearance, key state);
                // re-apply the tuning whenever the user is about to look at / touch the Dock.
                #[cfg(target_os = "macos")]
                if over {
                    let _ = app.run_on_main_thread(liquid_glass::tune_all);
                }
            }
            if over && pointer_moved(last, x, y) {
                last = Some((x, y));
                let _ = app.emit("dock:pointer", (x, y));
            }
        }
    });
}

/// Called by the dock once a drag ends: the pill's centre becomes the anchor and snapping is off.
#[tauri::command]
fn remember_dock_position(app: AppHandle) -> CmdResult<()> {
    let win = app.get_webview_window("dock").ok_or("no dock window")?;
    let sc = win.scale_factor().map_err(err)?;
    let pos = win.outer_position().map_err(err)?;
    let size = win.outer_size().map_err(err)?;
    let anchor = settings::DockAnchor {
        x: (pos.x as f64 + size.width as f64 / 2.0) / sc,
        y: (pos.y as f64 + size.height as f64 / 2.0) / sc,
    };
    let st = app.state::<AppState>();
    let settings = {
        let mut s = st.settings.lock().unwrap();
        s.snap = settings::Snap::Free;
        s.dock_anchor = Some(anchor);
        s.clone()
    };
    tracing::info!("dock anchored at ({:.0}, {:.0})", anchor.x, anchor.y);
    settings.save(&settings::settings_path(&st.config_dir)).map_err(err)?;
    let _ = app.emit("settings:changed", &settings);
    Ok(())
}

/// The dock's monitor as (x, y, w, h) in logical px.
fn monitor_rect<R: Runtime>(win: &WebviewWindow<R>) -> tauri::Result<(f64, f64, f64, f64)> {
    Ok(match win.current_monitor()?.or(win.primary_monitor()?) {
        Some(m) => {
            let (s, p, sc) = (m.size(), m.position(), m.scale_factor());
            (p.x as f64 / sc, p.y as f64 / sc, s.width as f64 / sc, s.height as f64 / sc)
        }
        None => (0.0, 0.0, 1440.0, 900.0),
    })
}

fn position_dock<R: Runtime>(win: &WebviewWindow<R>, width: f64, height: f64, snap: settings::Snap, anchor: Option<settings::DockAnchor>) -> tauri::Result<()> {
    let (x, y) = dock_origin(snap, anchor, width, height, monitor_rect(win)?);
    win.set_size(tauri::LogicalSize::new(width, height))?;
    win.set_position(tauri::LogicalPosition::new(x, y))?;
    // AppKit caches the shadow of a transparent window; after a pill↔dot resize the stale
    // rectangular shadow shows as a dark square behind the circle until it is recomputed.
    #[cfg(target_os = "macos")]
    if let Ok(p) = win.ns_window() {
        let p = p as usize;
        let _ = win.run_on_main_thread(move || unsafe { (&*(p as *mut objc2_app_kit::NSWindow)).invalidateShadow() });
    }
    Ok(())
}

/// Pure placement: top-left origin of a `width`×`height` pill on monitor `rect` = (x, y, w, h), logical px.
/// `Free` keeps the pill centred on the user's anchor but never lets it leave the screen.
fn dock_origin(snap: settings::Snap, anchor: Option<settings::DockAnchor>, width: f64, height: f64, rect: (f64, f64, f64, f64)) -> (f64, f64) {
    use settings::Snap::*;
    let (mx, my, mw, mh) = rect;
    let margin = 8.0;
    let clamp = |v: f64, lo: f64, hi: f64| v.max(lo).min(hi.max(lo));
    match (snap, anchor) {
        (Free, Some(a)) => (
            clamp(a.x - width / 2.0, mx + margin, mx + mw - width - margin),
            clamp(a.y - height / 2.0, my + margin, my + mh - height - margin),
        ),
        _ => {
            let x = match snap {
                TopLeft | BottomLeft => mx + margin,
                TopRight | BottomRight => mx + mw - width - margin,
                _ => mx + (mw - width) / 2.0,
            };
            let y = match snap {
                BottomCenter | BottomLeft | BottomRight => my + mh - height - margin - 70.0,
                _ => my + margin,
            };
            (x, y)
        }
    }
}

#[cfg(test)]
mod placement_tests {
    use super::{dock_origin, pointer_moved};
    use crate::settings::{DockAnchor, Snap};
    const MON: (f64, f64, f64, f64) = (0.0, 0.0, 1440.0, 900.0);

    #[test]
    fn pointer_moved_fires_on_first_sample_and_after_threshold() {
        assert!(pointer_moved(None, 24.0, 28.0));
        assert!(!pointer_moved(Some((24.0, 28.0)), 24.1, 28.2));
        assert!(pointer_moved(Some((24.0, 28.0)), 24.4, 28.0));
        assert!(pointer_moved(Some((24.0, 28.0)), 24.0, 28.5));
    }

    #[test]
    fn free_anchor_centres_and_clamps_to_screen() {
        assert_eq!(dock_origin(Snap::Free, Some(DockAnchor { x: 720.0, y: 450.0 }), 100.0, 56.0, MON), (670.0, 422.0));
        assert_eq!(dock_origin(Snap::Free, Some(DockAnchor { x: 5.0, y: 5000.0 }), 100.0, 56.0, MON), (8.0, 900.0 - 56.0 - 8.0));
    }

    #[test]
    fn free_without_anchor_behaves_like_top_centre() {
        assert_eq!(dock_origin(Snap::Free, None, 100.0, 56.0, MON), dock_origin(Snap::TopCenter, None, 100.0, 56.0, MON));
        assert_eq!(dock_origin(Snap::BottomRight, None, 100.0, 56.0, MON), (1440.0 - 108.0, 900.0 - 56.0 - 78.0));
    }
}

// ───────────────────────── commands: seats ─────────────────────────

#[tauri::command]
fn get_seats(st: tauri::State<AppState>) -> Vec<SeatView> {
    seat_views(&st)
}

/// App seats need no login: installed + Accessibility allowed is all "connected" means.
/// Seats on a placeholder driver stay "pending" whatever their kind.
fn refresh_app_status(seats: &mut [Seat], registry: &Registry) {
    for s in seats {
        let driver = match &s.driver {
            DriverKind::Web { driver, .. } | DriverKind::App { driver, .. } => driver,
        };
        if registry.get(driver).is_ok_and(|d| d.pending) {
            s.status = SeatStatus::Pending;
            continue;
        }
        if let DriverKind::App { bundle_id, .. } = &s.driver {
            s.status = if !AppDriver::is_installed(bundle_id) {
                SeatStatus::Unavailable
            } else if AppDriver::is_ax_trusted() {
                SeatStatus::Connected
            } else {
                SeatStatus::NeedsAuth
            };
        }
    }
}

#[tauri::command]
fn save_seats(app: AppHandle, mut seats: Vec<Seat>) -> CmdResult<()> {
    let st = app.state::<AppState>();
    refresh_app_status(&mut seats, &st.registry);
    {
        let mut cfg = st.seats.lock().unwrap();
        cfg.seats = seats;
    }
    persist_seats(&st);
    emit_seats(&app, &st);
    register_hotkeys(&app).map_err(err)?;
    Ok(())
}

#[tauri::command]
fn reorder_seats(app: AppHandle, ids: Vec<String>) -> CmdResult<()> {
    let st = app.state::<AppState>();
    st.seats.lock().unwrap().reorder(&ids);
    persist_seats(&st);
    emit_seats(&app, &st);
    register_hotkeys(&app).map_err(err)
}

#[tauri::command]
fn list_drivers(st: tauri::State<AppState>) -> Vec<DriverInfo> {
    st.registry
        .ids()
        .into_iter()
        .filter_map(|id| st.registry.get(id).ok())
        .map(|d| DriverInfo {
            id: d.id.clone(),
            display_name: d.display_name.clone(),
            version: d.version,
            verified: d.verified,
            kind: if d.url.is_some() { "web".into() } else { "app".into() },
            url: d.url.clone(),
            bundle_id: d.bundle_id.clone(),
            installed: d.url.is_some() || d.bundle_id.as_deref().is_some_and(AppDriver::is_installed),
            discover: !d.targets.is_empty(),
            picks: drivers::uses_target(&d.call),
            target_noun: d.target_noun.clone(),
            pending: d.pending,
        })
        .collect()
}

/// Web seat: in Chrome, open its tab in front so the user signs in there once (usually they
/// already are); otherwise show the hidden webview as a login window that hides itself once logged
/// in. App seat: open the OS accessibility settings so the user can grant permission.
/// Returns where the login happens: "chrome", "webview" or "app".
#[tauri::command]
async fn connect_account(app: AppHandle, seat_id: String) -> CmdResult<String> {
    tauri::async_runtime::spawn_blocking(move || connect_account_blocking(app, seat_id)).await.map_err(err)?
}

fn connect_account_blocking(app: AppHandle, seat_id: String) -> CmdResult<String> {
    let st = app.state::<AppState>();
    let seat = st.seats.lock().unwrap().get(&seat_id).cloned().ok_or("unknown seat")?;
    match &seat.driver {
        DriverKind::Web { driver, url } => {
            let spec = st.registry.get(driver).map_err(err)?;
            let url = spec.url.as_deref().unwrap_or(url);
            st.logging_in.lock().unwrap().insert(seat_id.clone());
            if use_chrome(&st) {
                st.chrome.show(&seat_id, url).map_err(err)?;
                return Ok("chrome".into());
            }
            let win = web::ensure_webview(&app, &seat_id, spec).map_err(err)?;
            web::show(&win).map_err(err)?;
            // a long-hidden webview can come back blank (its web process was reclaimed): reload it
            if let Ok(u) = tauri::Url::parse(url) {
                let _ = win.navigate(u);
            }
            Ok("webview".into())
        }
        DriverKind::App { bundle_id, .. } => {
            if !AppDriver::is_installed(bundle_id) {
                set_seat_status(&app, &seat_id, SeatStatus::Unavailable);
                return Err(i18n::t("app.not_installed"));
            }
            if AppDriver::prompt_ax_trust() {
                set_seat_status(&app, &seat_id, SeatStatus::Connected);
                return Ok("app".into());
            }
            set_seat_status(&app, &seat_id, SeatStatus::NeedsAuth);
            #[cfg(target_os = "macos")]
            {
                use tauri_plugin_opener::OpenerExt;
                app.opener().open_url(drivers::app::MACOS_ACCESSIBILITY_SETTINGS, None::<&str>).map_err(err)?;
            }
            Ok("app".into())
        }
    }
}

/// Clears the built-in webview's session and closes the seat's Chrome tab; Chrome's own logins stay.
#[tauri::command]
fn disconnect_seat(app: AppHandle, seat_id: String) -> CmdResult<()> {
    let st = app.state::<AppState>();
    st.chrome.close(&seat_id);
    web::disconnect(&app, &seat_id).map_err(err)?;
    st.probes.lock().unwrap().remove(&seat_id);
    {
        let mut seats = st.seats.lock().unwrap();
        if let Some(s) = seats.get_mut(&seat_id) {
            s.status = SeatStatus::NeedsAuth;
            s.account = None;
        }
    }
    persist_seats(&st);
    emit_seats(&app, &st);
    Ok(())
}

/// Bring a seat's hidden page forward (captcha / "needs a human"), or hide it again. App seats
/// have no page of ours: their own window is brought to the front instead; Chrome seats, their tab.
#[tauri::command]
async fn show_seat(app: AppHandle, seat_id: String, visible: bool) -> CmdResult<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let st = app.state::<AppState>();
        let seat = st.seats.lock().unwrap().get(&seat_id).cloned().ok_or("unknown seat")?;
        match &seat.driver {
            DriverKind::App { bundle_id, .. } => return if visible { AppDriver::activate(bundle_id).map_err(err) } else { Ok(()) },
            DriverKind::Web { driver, url } if visible && use_chrome(&st) => {
                let spec = st.registry.get(driver).map_err(err)?;
                return st.chrome.show(&seat_id, spec.url.as_deref().unwrap_or(url)).map_err(err);
            }
            DriverKind::Web { .. } => {}
        }
        let win = app.get_webview_window(&web::window_label(&seat_id)).ok_or("seat has no webview yet")?;
        if visible { web::show(&win) } else { web::hide(&win) }.map_err(err)
    })
    .await
    .map_err(err)?
}

/// Connect-flow self-check: call, wait for the page to report a call, hang up.
#[tauri::command]
async fn test_call(app: AppHandle, seat_id: String) -> CmdResult<bool> {
    let driver = |op: DriverOp| {
        let (app, seat_id) = (app.clone(), seat_id.clone());
        tauri::async_runtime::spawn_blocking(move || run_driver(&app, &seat_id, op))
    };
    driver(DriverOp::Call).await.map_err(err)?.map_err(err)?;
    let mut connected = false;
    for _ in 0..30 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let _ = driver(DriverOp::Probe).await;
        if app.state::<AppState>().probes.lock().unwrap().get(&seat_id).map(|p| p.in_call).unwrap_or(false) {
            connected = true;
            break;
        }
    }
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let _ = driver(DriverOp::Hangup).await;
    Ok(connected)
}

#[tauri::command]
fn probe_seat(app: AppHandle, seat_id: String) -> CmdResult<()> {
    run_driver(&app, &seat_id, DriverOp::Probe).map_err(err)
}

/// Called from inside a hidden seat webview by the injected probe script.
#[tauri::command]
fn driver_report(app: AppHandle, window: tauri::Window, seat_id: String, result: ProbeResult) {
    if window.label() != web::window_label(&seat_id) {
        tracing::warn!("probe report from unexpected window {}", window.label());
        return;
    }
    ingest_probe(&app, &seat_id, result);
}

#[tauri::command]
fn driver_error(window: tauri::Window, message: String) {
    tracing::warn!("driver error in {}: {message}", window.label());
}

/// Caption lines scraped from a seat page. Ignored unless that page's window reports them and
/// memory is on for an open call on this seat.
#[tauri::command]
fn memory_report(app: AppHandle, window: tauri::Window, seat_id: String, lines: Vec<memory::TranscriptLine>) {
    if window.label() != web::window_label(&seat_id) {
        tracing::warn!("memory report from unexpected window {}", window.label());
        return;
    }
    memory_lines(&app, &seat_id, &lines);
}

/// New caption lines from a seat page (webview IPC or a Chrome tab's script value).
fn memory_lines(app: &AppHandle, seat_id: &str, lines: &[memory::TranscriptLine]) {
    let st = app.state::<AppState>();
    let client = st.everos.lock().unwrap().clone();
    if !client.is_active() {
        return;
    }
    let batch = {
        let mut book = st.memory.lock().unwrap();
        book.on_lines(seat_id, lines, memory::now_ms())
    };
    if let Some(batch) = batch {
        tauri::async_runtime::spawn(async move { write_batch(client, batch).await });
    }
}

/// What an app driver can call, read from the app (e.g. the Bots in Grok Bot's sidebar).
#[tauri::command]
async fn app_targets(app: AppHandle, driver: String) -> CmdResult<Vec<String>> {
    tauri::async_runtime::spawn_blocking(move || {
        let st = app.state::<AppState>();
        let spec = st.registry.get(&driver).map_err(err)?;
        AppDriver::targets(spec).map_err(|e| match e {
            DriverError::NeedsAuth => i18n::t("app.needs_ax"),
            other => other.to_string(),
        })
    })
    .await
    .map_err(err)?
}

/// Whether web seats can use the user's Chrome right now (Settings › Agent).
#[tauri::command]
fn chrome_status(st: tauri::State<AppState>) -> serde_json::Value {
    serde_json::json!({ "available": drivers::chrome::available(), "connected": st.chrome.is_connected() })
}

#[tauri::command]
fn memory_status(st: tauri::State<AppState>) -> everos::Status {
    st.everos.lock().unwrap().status().clone()
}

// ───────────────────────── commands: icons ─────────────────────────

fn icons_dir(st: &AppState) -> PathBuf {
    st.config_dir.join("icons")
}

/// Store a user-picked image (png/svg/webp/gif/apng) for a seat. `data_base64` is raw base64.
#[tauri::command]
fn set_seat_icon(app: AppHandle, seat_id: String, file_name: String, data_base64: String) -> CmdResult<String> {
    use base64::Engine;
    let st = app.state::<AppState>();
    let ext = std::path::Path::new(&file_name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_else(|| "png".into());
    if !["png", "svg", "webp", "gif", "apng", "jpg", "jpeg"].contains(&ext.as_str()) {
        return Err(i18n::t("icon.bad_format"));
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(data_base64.trim()).map_err(err)?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(i18n::t("icon.too_large"));
    }
    let dir = icons_dir(&st);
    std::fs::create_dir_all(&dir).map_err(err)?;
    let path = dir.join(format!("{seat_id}.{ext}"));
    std::fs::write(&path, bytes).map_err(err)?;
    let rel = format!("icons/{seat_id}.{ext}");
    {
        let mut seats = st.seats.lock().unwrap();
        if let Some(s) = seats.get_mut(&seat_id) {
            s.icon = SeatIcon::Custom { path: rel.clone() };
        }
    }
    persist_seats(&st);
    emit_seats(&app, &st);
    Ok(rel)
}

#[tauri::command]
fn reset_seat_icon(app: AppHandle, seat_id: String) -> CmdResult<()> {
    let st = app.state::<AppState>();
    {
        let mut seats = st.seats.lock().unwrap();
        if let Some(s) = seats.get_mut(&seat_id) {
            s.icon = SeatIcon::Builtin { key: seat_id.clone() };
        }
    }
    persist_seats(&st);
    emit_seats(&app, &st);
    Ok(())
}

/// Custom icon as a data URL (so the UI needs no asset-protocol scope).
#[tauri::command]
fn seat_icon_data(st: tauri::State<AppState>, seat_id: String) -> CmdResult<Option<String>> {
    use base64::Engine;
    let seats = st.seats.lock().unwrap();
    let Some(seat) = seats.get(&seat_id) else { return Ok(None) };
    let SeatIcon::Custom { path } = &seat.icon else { return Ok(None) };
    let full = st.config_dir.join(path);
    let bytes = std::fs::read(&full).map_err(err)?;
    let mime = match full.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "svg" => "image/svg+xml",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "apng" => "image/apng",
        "jpg" | "jpeg" => "image/jpeg",
        _ => "image/png",
    };
    Ok(Some(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes))))
}

// ───────────────────────── commands: settings ─────────────────────────

#[tauri::command]
fn get_settings(st: tauri::State<AppState>) -> AppSettings {
    st.settings.lock().unwrap().clone()
}

#[tauri::command]
fn save_settings(app: AppHandle, settings: AppSettings) -> CmdResult<()> {
    persist_settings(&app, settings)
}

/// Shortcut / menu toggles: 横向 ↔ 竖向.
#[tauri::command]
fn toggle_orientation(app: AppHandle) -> CmdResult<()> {
    let mut s = app.state::<AppState>().settings.lock().unwrap().clone();
    s.orientation = s.orientation.toggled();
    persist_settings(&app, s)
}

/// Shortcut / menu toggles: 小圆点 ↔ 宠物 as the collapsed form.
#[tauri::command]
fn toggle_collapsed_form(app: AppHandle) -> CmdResult<()> {
    let mut s = app.state::<AppState>().settings.lock().unwrap().clone();
    s.default_form = s.default_form.toggled_collapsed();
    persist_settings(&app, s)
}

/// Right-click on the pill: the few things you'd otherwise open 设置 for.
#[tauri::command]
fn dock_menu(app: AppHandle) -> CmdResult<()> {
    use tauri::menu::{ContextMenu, Menu, MenuItem, PredefinedMenuItem};
    let win = app.get_webview_window("dock").ok_or("no dock window")?;
    let (orientation, form, expanded) = {
        let st = app.state::<AppState>();
        let expanded = st.machine.lock().unwrap().state.expanded;
        let s = st.settings.lock().unwrap();
        let out = (s.orientation, s.default_form, expanded);
        out
    };
    let hk = app.state::<AppState>().settings.lock().unwrap().hotkeys.clone();
    let tip = |k: &str| k.replace("Alt", "⌥").replace("Control", "⌃").replace("Super", "⌘").replace("Shift", "⇧").replace('+', "");
    let orient_label = match orientation {
        settings::Orientation::Horizontal => format!("{}    {}", i18n::t("menu.to_vertical"), tip(&hk.orientation)),
        settings::Orientation::Vertical => format!("{}    {}", i18n::t("menu.to_horizontal"), tip(&hk.orientation)),
    };
    let form_label = match form {
        settings::DefaultForm::Pet => format!("{}    {}", i18n::t("menu.form_dot"), tip(&hk.form)),
        _ => format!("{}    {}", i18n::t("menu.form_pet"), tip(&hk.form)),
    };
    let toggle_label = format!("{}    {}", i18n::t(if expanded { "menu.collapse" } else { "menu.expand" }), tip(&hk.toggle));
    let cur_pct = (app.state::<AppState>().settings.lock().unwrap().dock_scale.clamp(0.7, 1.6) * 100.0).round() as i32;
    let items: Vec<Box<dyn tauri::menu::IsMenuItem<tauri::Wry>>> = vec![
        Box::new(MenuItem::with_id(&app, "dock-toggle", toggle_label, true, None::<&str>).map_err(err)?),
        Box::new(MenuItem::with_id(&app, "dock-orientation", orient_label, true, None::<&str>).map_err(err)?),
        Box::new(MenuItem::with_id(&app, "dock-form", form_label, true, None::<&str>).map_err(err)?),
        Box::new(PredefinedMenuItem::separator(&app).map_err(err)?),
        Box::new(MenuItem::with_id(&app, "dock-zoom-in", i18n::t("menu.zoom_in"), cur_pct < 160, None::<&str>).map_err(err)?),
        Box::new(MenuItem::with_id(&app, "dock-zoom-out", i18n::t("menu.zoom_out"), cur_pct > 70, None::<&str>).map_err(err)?),
        Box::new(MenuItem::with_id(&app, "dock-zoom-reset", i18n::tf("menu.zoom_reset", &[("pct", &cur_pct.to_string())]), cur_pct != 100, None::<&str>).map_err(err)?),
        Box::new(PredefinedMenuItem::separator(&app).map_err(err)?),
        Box::new(language_submenu(&app).map_err(err)?),
        Box::new(MenuItem::with_id(&app, "settings", i18n::t("menu.settings"), true, None::<&str>).map_err(err)?),
        Box::new(MenuItem::with_id(&app, "quit", i18n::t("menu.quit"), true, None::<&str>).map_err(err)?),
    ];
    let refs: Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>> = items.iter().map(|b| b.as_ref()).collect();
    let menu = Menu::with_items(&app, &refs).map_err(err)?;
    menu.popup(win.as_ref().window()).map_err(err)
}

fn step_dock_scale(app: &AppHandle, delta: Option<f32>) {
    let mut s = app.state::<AppState>().settings.lock().unwrap().clone();
    s.dock_scale = match delta {
        Some(d) => ((s.dock_scale + d).clamp(0.7, 1.6) * 20.0).round() / 20.0,
        None => 1.0,
    };
    let _ = persist_settings(app, s);
}

/// 语言 · Language ▸ follow the system, or one of the four; the current choice is ticked.
fn language_submenu(app: &AppHandle) -> tauri::Result<tauri::menu::Submenu<tauri::Wry>> {
    use tauri::menu::{CheckMenuItem, IsMenuItem, Submenu};
    let current = app.state::<AppState>().settings.lock().unwrap().language;
    let items = i18n::language_choices()
        .into_iter()
        .map(|(id, lang, label)| CheckMenuItem::with_id(app, id, label, true, lang == current, None::<&str>))
        .collect::<tauri::Result<Vec<_>>>()?;
    let refs: Vec<&dyn IsMenuItem<tauri::Wry>> = items.iter().map(|i| i as &dyn IsMenuItem<tauri::Wry>).collect();
    Submenu::with_items(app, i18n::language_menu_title(), true, &refs)
}

/// A pick from a language menu (pill or menu bar). `false` when `id` is not one.
fn on_language_menu(app: &AppHandle, id: &str) -> bool {
    let Some((_, lang, _)) = i18n::language_choices().into_iter().find(|(choice, _, _)| *choice == id) else { return false };
    let mut s = app.state::<AppState>().settings.lock().unwrap().clone();
    if s.language != lang {
        s.language = lang;
        let _ = persist_settings(app, s);
    }
    true
}

/// Window-menu (pill context menu) events; the tray has its own handler.
fn on_app_menu(app: &AppHandle, id: &str) {
    if on_language_menu(app, id) {
        return;
    }
    match id {
        "dock-toggle" => dispatch(app, Event::ToggleExpanded),
        "dock-orientation" => { let _ = toggle_orientation(app.clone()); }
        "dock-form" => { let _ = toggle_collapsed_form(app.clone()); }
        "dock-zoom-in" => step_dock_scale(app, Some(0.1)),
        "dock-zoom-out" => step_dock_scale(app, Some(-0.1)),
        "dock-zoom-reset" => step_dock_scale(app, None),
        "settings" => { let _ = open_settings(app.clone()); }
        "quit" => app.exit(0),
        _ => {}
    }
}

/// macOS: a Dock icon means a regular app (Dock + ⌘Tab); menu bar only means an accessory app.
/// The bundle starts as an accessory (`LSUIElement`), so "menu bar only" never flashes an icon.
fn apply_app_icon(app: &AppHandle, mode: settings::AppIcon) {
    #[cfg(target_os = "macos")]
    {
        use settings::AppIcon;
        let policy = if mode == AppIcon::MenuBar { tauri::ActivationPolicy::Accessory } else { tauri::ActivationPolicy::Regular };
        if let Err(e) = app.set_activation_policy(policy) {
            tracing::warn!("activation policy: {e}");
        }
        if let Some(tray) = app.tray_by_id("main") {
            let _ = tray.set_visible(mode != AppIcon::Dock);
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, mode);
}

fn persist_settings(app: &AppHandle, mut settings: AppSettings) -> CmdResult<()> {
    let env = everos::MemoryEnv::from_env();
    let _ = everos::assign_user_id(&mut settings.memory, &env);
    let client = everos::Client::resolve(&settings.memory, &env);
    let st = app.state::<AppState>();
    let (hotkeys_changed, placement_changed, icon_changed, language_changed) = {
        let mut cur = st.settings.lock().unwrap();
        let out = (
            cur.hotkeys != settings.hotkeys,
            cur.snap != settings.snap || cur.dock_anchor != settings.dock_anchor,
            cur.app_icon != settings.app_icon,
            cur.language != settings.language,
        );
        *cur = settings.clone();
        out
    };
    if language_changed {
        apply_language(app, settings.language);
    }
    *st.everos.lock().unwrap() = client;
    settings.save(&settings::settings_path(&st.config_dir)).map_err(err)?;
    let _ = app.emit("settings:changed", &settings);
    apply_theme(app, settings.theme);
    if icon_changed {
        apply_app_icon(app, settings.app_icon);
    }
    #[cfg(target_os = "macos")]
    {
        let strength = settings.material_strength as f64;
        let tint = settings.accent.as_deref().and_then(settings::parse_hex_color);
        let _ = app.run_on_main_thread(move || {
            liquid_glass::set_material_strength(strength);
            liquid_glass::set_tint(tint);
        });
    }
    // Re-registering from inside a shortcut callback (⌥O / ⌥P toggles) deadlocks the global-shortcut
    // plugin on the main thread, so only touch the registrations when the bindings really changed.
    if hotkeys_changed {
        register_hotkeys(app).map_err(err)?;
    }
    // Only move the window when the user changed where it lives. Anything that changes the pill's
    // size (横向 ↔ 竖向, forms) is re-fitted by the webview itself via `resize_dock` after
    // `settings:changed`; repositioning here with the stale size would race that and win.
    if placement_changed {
        if let Some(win) = app.get_webview_window("dock") {
            let _ = reposition_dock(&win, settings.snap, settings.dock_anchor);
        }
    }
    Ok(())
}

fn reposition_dock<R: Runtime>(win: &WebviewWindow<R>, snap: settings::Snap, anchor: Option<settings::DockAnchor>) -> tauri::Result<()> {
    #[cfg(target_os = "macos")]
    morph::cancel();
    let sc = win.scale_factor()?;
    let size = win.inner_size()?;
    let (x, y) = dock_origin(snap, anchor, size.width as f64 / sc, size.height as f64 / sc, monitor_rect(win)?);
    win.set_position(tauri::LogicalPosition::new(x, y))
}

#[tauri::command]
fn hotkey_conflicts(st: tauri::State<AppState>, settings: AppSettings) -> Vec<String> {
    let n = st.seats.lock().unwrap().enabled().len();
    hotkeys::conflicts(&settings, n)
}

#[tauri::command]
fn open_settings(app: AppHandle) -> CmdResult<()> {
    let win = app.get_webview_window("settings").ok_or("no settings window")?;
    win.show().and_then(|_| win.set_focus()).map_err(err)?;
    #[cfg(target_os = "macos")]
    {
        let w2 = win.clone();
        let _ = win.run_on_main_thread(move || {
            let _ = liquid_glass::clear_webview_backdrop(&w2);
            // The CSS slab must follow the real window corner (26pt with the unified toolbar).
            let radius = liquid_glass::window_corner_radius(&w2);
            liquid_glass::set_settings_corner_radius(radius);
            let _ = w2.emit("settings:radius", radius);
            liquid_glass::tune_all();
        });
    }
    Ok(())
}

#[tauri::command]
fn quit(app: AppHandle) {
    app.exit(0);
}

/// Whether the dock glass currently wants light ("dark") or dark ("light") glyphs. Sync commands run
/// on the main thread, where AppKit may be queried; later changes arrive as `dock:glass` events.
#[tauri::command]
fn glass_scheme() -> Option<String> {
    #[cfg(target_os = "macos")]
    return liquid_glass::glass_scheme().map(String::from);
    #[cfg(not(target_os = "macos"))]
    None
}

/// Follow the glass's backdrop-adaptive appearance and tell the dock when it flips.
#[cfg(target_os = "macos")]
fn spawn_glass_scheme_watcher(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(400)).await;
            let handle = app.clone();
            let _ = app.run_on_main_thread(move || {
                if let Some(scheme) = liquid_glass::glass_scheme_changed() {
                    tracing::info!("dock glass content scheme → {scheme}");
                    let _ = handle.emit("dock:glass", scheme);
                }
            });
        }
    });
}

#[tauri::command]
fn host_info(app: AppHandle) -> serde_json::Value {
    serde_json::json!({
        "os": std::env::consts::OS,
        "vibrancy": cfg!(any(target_os = "macos", target_os = "windows")),
        "material": MATERIAL.lock().unwrap().clone(),
        // the app version (tauri.conf.json reads package.json), not the crate's
        "version": app.package_info().version.to_string(),
        "window_radius": *SETTINGS_RADIUS.lock().unwrap(),
        // what "follow the system" resolves from, on both sides
        "locale": i18n::system_locale(),
    })
}

/// Which system material actually got applied to the dock window (reported in 设置 › 外观).
static MATERIAL: Mutex<String> = Mutex::new(String::new());
/// Corner radius of the native settings window, so the CSS slab stays concentric with it.
static SETTINGS_RADIUS: Mutex<f64> = Mutex::new(0.0);

/// One semi-implicit Euler step of a slightly under-damped spring (≈3 % overshoot) over a window
/// frame in centre space `[cx, cy, w, h]`. Returns whether any component is still moving.
/// Sizes may bounce past a larger target but never sag below a smaller one: a collapsing pill that
/// dipped under the dot's 36pt width read as a vertical capsule for a few frames.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn spring_step(cur: &mut [f64; 4], vel: &mut [f64; 4], target: &[f64; 4], dt: f64) -> bool {
    const K: f64 = 520.0;
    const ZETA: f64 = 0.74;
    let c = 2.0 * ZETA * K.sqrt();
    let mut moving = false;
    for i in 0..4 {
        vel[i] += (-K * (cur[i] - target[i]) - c * vel[i]) * dt;
        cur[i] += vel[i] * dt;
        if i >= 2 && cur[i] < target[i] && vel[i] < 0.0 {
            cur[i] = target[i];
            vel[i] = 0.0;
        }
        moving |= (cur[i] - target[i]).abs() > 0.25 || vel[i].abs() > 4.0;
    }
    moving
}

/// Morph state lives in centre space, so clamping a size never drags the centre sideways.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn frame_to_centre(f: [f64; 4]) -> [f64; 4] {
    [f[0] + f[2] / 2.0, f[1] + f[3] / 2.0, f[2], f[3]]
}
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn centre_to_frame(c: [f64; 4]) -> [f64; 4] {
    [c[0] - c[2] / 2.0, c[1] - c[3] / 2.0, c[2], c[3]]
}

#[cfg(test)]
mod spring_tests {
    use super::spring_step;

    #[test]
    fn dock_morph_settles_quickly_with_a_small_overshoot() {
        let (mut cur, mut vel, target) = ([500.0, 300.0, 36.0, 36.0], [0.0; 4], [314.0, 290.0, 408.0, 56.0]);
        let (dt, mut t, mut peak) = (1.0 / 240.0, 0.0, 0.0f64);
        while spring_step(&mut cur, &mut vel, &target, dt) {
            peak = peak.max(cur[2]);
            t += dt;
            assert!(t < 1.0, "spring never settled");
        }
        assert!(t < 0.6, "settles in {t:.2}s");
        assert!((cur[2] - 408.0).abs() < 0.5 && (cur[0] - 314.0).abs() < 0.5);
        assert!(peak > 408.0 && peak < 408.0 + 0.06 * 372.0, "overshoot {peak:.1} should be a small bounce");
    }

    #[test]
    fn collapsing_never_sags_below_the_dot_and_keeps_its_centre() {
        let start = super::frame_to_centre([314.0, 290.0, 408.0, 56.0]);
        let target = super::frame_to_centre([500.0, 300.0, 36.0, 36.0]);
        let (mut cur, mut vel) = (start, [0.0; 4]);
        let mut min_side = f64::MAX;
        while spring_step(&mut cur, &mut vel, &target, 1.0 / 240.0) {
            min_side = min_side.min(cur[2]).min(cur[3]);
        }
        assert!(min_side >= 36.0, "dipped to {min_side:.1}");
        assert_eq!(super::centre_to_frame(cur).map(|v| v.round()), [500.0, 300.0, 36.0, 36.0]);
    }
}

/// Spring-driven morph of the dock window frame. The NSGlassEffectView autoresizes with the window,
/// so animating the frame animates the glass itself: the dot swells into the pill like a droplet.
/// A new target mid-flight keeps the current position and velocity, so quick toggles stay fluid.
#[cfg(target_os = "macos")]
mod morph {
    use super::{centre_to_frame, frame_to_centre, liquid_glass, spring_step};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    use tauri::{Runtime, WebviewWindow};

    struct State {
        cur: [f64; 4],
        vel: [f64; 4],
        target: [f64; 4],
        running: bool,
        gen: u64,
    }
    static STATE: Mutex<State> = Mutex::new(State { cur: [0.0; 4], vel: [0.0; 4], target: [0.0; 4], running: false, gen: 0 });

    /// Stop any running morph (drags and direct resizes own the frame from here on).
    pub fn cancel() {
        let mut s = STATE.lock().unwrap();
        s.running = false;
        s.gen += 1;
    }

    pub fn animate_to<R: Runtime>(win: &WebviewWindow<R>, target: [f64; 4]) -> tauri::Result<()> {
        let gen = {
            let mut s = STATE.lock().unwrap();
            s.target = frame_to_centre(target);
            if s.running {
                return Ok(());
            }
            let (sc, p, z) = (win.scale_factor()?, win.outer_position()?, win.outer_size()?);
            s.cur = frame_to_centre([p.x as f64 / sc, p.y as f64 / sc, z.width as f64 / sc, z.height as f64 / sc]);
            s.vel = [0.0; 4];
            s.running = true;
            s.gen += 1;
            s.gen
        };
        let win = win.clone();
        tauri::async_runtime::spawn(async move {
            let mut last = Instant::now();
            let mut frame_no = 0u32;
            loop {
                tokio::time::sleep(Duration::from_millis(8)).await;
                let now = Instant::now();
                let dt = now.duration_since(last).as_secs_f64().min(1.0 / 30.0);
                last = now;
                let (frame, done) = {
                    let mut guard = STATE.lock().unwrap();
                    let s = &mut *guard;
                    if s.gen != gen || !s.running {
                        return;
                    }
                    let steps = (dt * 240.0).ceil().max(1.0) as usize;
                    let mut moving = false;
                    for _ in 0..steps {
                        moving = spring_step(&mut s.cur, &mut s.vel, &s.target, dt / steps as f64);
                    }
                    if !moving {
                        s.cur = s.target;
                        s.vel = [0.0; 4];
                        s.running = false;
                    }
                    (centre_to_frame(s.cur), !moving)
                };
                frame_no += 1;
                apply(&win, frame, done || frame_no % 4 == 0);
                if done {
                    break;
                }
            }
        });
        Ok(())
    }

    /// Set the frame in one AppKit call (size and origin together, so the centre never jitters).
    fn apply<R: Runtime>(win: &WebviewWindow<R>, f: [f64; 4], refresh_shadow: bool) {
        let Ok(p) = win.ns_window() else { return };
        let p = p as usize;
        let _ = win.run_on_main_thread(move || unsafe {
            use objc2::runtime::AnyObject;
            use objc2::{class, msg_send};
            use objc2_foundation::{NSPoint, NSRect, NSSize};
            let window = &*(p as *mut objc2_app_kit::NSWindow);
            let screens: *mut AnyObject = msg_send![class!(NSScreen), screens];
            let main: *mut AnyObject = msg_send![screens, firstObject];
            if main.is_null() {
                return;
            }
            // Logical coordinates are top-left based on the main display; AppKit is bottom-left.
            let screen: NSRect = msg_send![main, frame];
            let rect = NSRect::new(NSPoint::new(f[0], screen.size.height - f[1] - f[3]), NSSize::new(f[2], f[3]));
            window.setFrame_display(rect, true);
            liquid_glass::set_corner_radius(f[2].min(f[3]) / 2.0);
            if refresh_shadow {
                window.invalidateShadow();
            }
        });
    }
}

// ───────────────────────── hotkeys ─────────────────────────

fn register_hotkeys<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let st = app.state::<AppState>();
    let settings = st.settings.lock().unwrap().clone();
    let enabled = st.seats.lock().unwrap().enabled().len();
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    let mut map = Vec::new();
    for (key, action) in hotkeys::bindings(&settings, enabled) {
        match key.parse::<Shortcut>() {
            Ok(sc) => {
                if let Err(e) = gs.register(sc) {
                    tracing::warn!("hotkey {key} unavailable: {e}");
                    continue;
                }
                map.push((sc, action));
            }
            Err(e) => tracing::warn!("bad hotkey {key}: {e}"),
        }
    }
    *st.hotkey_map.lock().unwrap() = map;
    Ok(())
}

fn on_hotkey(app: &AppHandle, shortcut: &Shortcut, pressed: bool) {
    let st = app.state::<AppState>();
    let action = st.hotkey_map.lock().unwrap().iter().find(|(s, _)| s == shortcut).map(|(_, a)| a.clone());
    let push_to_talk = st.settings.lock().unwrap().hotkeys.push_to_talk;
    let Some(action) = action else { return };
    tracing::info!("hotkey {shortcut:?} → {action:?} pressed={pressed}");
    match (action, pressed) {
        (HotkeyAction::Toggle, true) if push_to_talk => dispatch(app, Event::SetMuted(false)),
        (HotkeyAction::Toggle, false) if push_to_talk => dispatch(app, Event::SetMuted(true)),
        (HotkeyAction::Toggle, true) => dispatch(app, Event::ToggleExpanded),
        (HotkeyAction::Mute, true) => dispatch(app, Event::ToggleMute),
        (HotkeyAction::Hangup, true) => dispatch(app, Event::Hangup),
        (HotkeyAction::Orientation, true) => { let _ = toggle_orientation(app.clone()); }
        (HotkeyAction::Form, true) => { let _ = toggle_collapsed_form(app.clone()); }
        (HotkeyAction::Seat { index }, true) => {
            let id = st.seats.lock().unwrap().enabled().get(index - 1).map(|s| s.id.clone());
            if let Some(id) = id {
                dispatch(app, Event::Call(id));
            }
        }
        _ => {}
    }
}

// ───────────────────────── glass ─────────────────────────

/// 设置 › 外观 › 主题 must reach the native layer too: the system glass (NSGlassEffectView /
/// NSVisualEffectView / Mica) renders in the *window's* appearance, so CSS alone would paint light
/// text on a light pane. `None` follows the OS.
fn apply_theme(app: &AppHandle, theme: settings::Theme) {
    app.set_theme(match theme {
        settings::Theme::System => None,
        settings::Theme::Light => Some(tauri::Theme::Light),
        settings::Theme::Dark => Some(tauri::Theme::Dark),
    });
    // WKWebView re-derives `underPageBackgroundColor` from the new effective appearance (white in
    // light, near-black in dark) and paints it as a plain square under the pill; clear it again once
    // the appearance change has propagated.
    #[cfg(target_os = "macos")]
    for label in ["dock", "settings"] {
        if let Some(win) = app.get_webview_window(label) {
            for delay_ms in [0u64, 250, 800] {
                let win = win.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                    let w2 = win.clone();
                    let _ = win.run_on_main_thread(move || {
                        if let Err(e) = liquid_glass::clear_webview_backdrop(&w2) {
                            tracing::warn!("{label} webview backdrop (theme): {e}");
                        }
                        liquid_glass::tune_all();
                    });
                });
            }
        }
    }
}

fn apply_glass<R: Runtime>(dock: &WebviewWindow<R>, settings_win: Option<&WebviewWindow<R>>, strength: f64) {
    #[cfg(target_os = "macos")]
    {
        use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial, NSVisualEffectState};
        if let Err(e) = liquid_glass::clear_webview_backdrop(dock) {
            tracing::warn!("dock webview backdrop: {e}");
        }
        // macOS 26+: the real thing — NSGlassEffectView (Liquid Glass). Older systems: vibrancy.
        let glass_ok = liquid_glass::apply(dock, 28.0, true, strength).unwrap_or_else(|e| {
            tracing::info!("NSGlassEffectView unavailable ({e}); falling back to NSVisualEffectView");
            false
        });
        if glass_ok {
            tracing::info!("dock material: NSGlassEffectView (Liquid Glass)");
            *MATERIAL.lock().unwrap() = "NSGlassEffectView".into();
        } else {
            match apply_vibrancy(dock, NSVisualEffectMaterial::HudWindow, Some(NSVisualEffectState::Active), Some(28.0)) {
                Ok(()) => {
                    tracing::info!("dock material: NSVisualEffectView (HudWindow)");
                    *MATERIAL.lock().unwrap() = "NSVisualEffectView".into();
                }
                Err(e) => tracing::warn!("dock vibrancy: {e}"),
            }
        }
        if let Some(w) = settings_win {
            let _ = liquid_glass::clear_webview_backdrop(w);
            // A unified toolbar gives the window the macOS 26 large corner (26pt); 26 − 8pt inset =
            // the 18pt content sheet, so the window, the glass and the sheet are all concentric.
            liquid_glass::adopt_unified_toolbar(w);
            let radius = liquid_glass::window_corner_radius(w);
            *SETTINGS_RADIUS.lock().unwrap() = radius;
            if !liquid_glass::apply(w, radius, false, strength).unwrap_or(false) {
                if let Err(e) = apply_vibrancy(w, NSVisualEffectMaterial::Sidebar, Some(NSVisualEffectState::Active), Some(radius)) {
                    tracing::warn!("settings vibrancy: {e}");
                }
            }
        }
    }
    #[cfg(target_os = "windows")]
    {
        let _ = strength;
        use window_vibrancy::{apply_acrylic, apply_mica};
        if apply_acrylic(dock, Some((18, 18, 18, 125))).is_ok() {
            *MATERIAL.lock().unwrap() = "Acrylic".into();
        } else if apply_mica(dock, None).is_ok() {
            *MATERIAL.lock().unwrap() = "Mica".into();
        }
        if let Some(w) = settings_win {
            let _ = apply_mica(w, None);
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (dock, settings_win, strength);
    }
}

/// Native Liquid Glass on macOS 26 (Tahoe) and later via `NSGlassEffectView`, looked up at runtime
/// so the binary still runs on older systems. The glass view is inserted *behind* the WKWebView,
/// sized to the window, with the same continuous corner radius the frontend uses.
#[cfg(target_os = "macos")]
mod liquid_glass {
    use objc2::rc::{Allocated, Retained};
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2::{msg_send, ClassType};
    use objc2_foundation::{NSObjectProtocol, NSString};
    use objc2_app_kit::{NSAutoresizingMaskOptions, NSColor, NSView, NSWindow, NSWindowOrderingMode};
    use tauri::{Runtime, WebviewWindow};

    /// The glass views inserted under the dock and settings webviews (0 = none / fallback path).
    static DOCK_GLASS: std::sync::Mutex<usize> = std::sync::Mutex::new(0);
    static SETTINGS_GLASS: std::sync::Mutex<usize> = std::sync::Mutex::new(0);
    static STRENGTH: std::sync::Mutex<f64> = std::sync::Mutex::new(0.5);
    /// Empty view installed as the dock glass's `contentView`. The glass adapts its content's
    /// appearance to what is behind it (DarkAqua over dark wallpaper/windows, Aqua over light), the
    /// same signal native controls use to flip their glyphs; the webview mirrors it.
    static DOCK_PROBE: std::sync::Mutex<usize> = std::sync::Mutex::new(0);
    static LAST_SCHEME: std::sync::Mutex<u8> = std::sync::Mutex::new(0);

    fn probe_is_dark() -> Option<bool> {
        let p = *DOCK_PROBE.lock().unwrap();
        if p == 0 {
            return None;
        }
        unsafe {
            let appearance: *mut AnyObject = msg_send![&*(p as *const AnyObject), effectiveAppearance];
            if appearance.is_null() {
                return None;
            }
            let name: *mut AnyObject = msg_send![appearance, name];
            (!name.is_null()).then(|| (&*(name as *const NSString)).to_string().contains("Dark"))
        }
    }

    /// The glass content scheme right now ("dark" = light glyphs needed).
    pub fn glass_scheme() -> Option<&'static str> {
        let dark = probe_is_dark()?;
        *LAST_SCHEME.lock().unwrap() = if dark { 2 } else { 1 };
        Some(if dark { "dark" } else { "light" })
    }

    /// `Some(scheme)` only when it differs from the last one reported.
    pub fn glass_scheme_changed() -> Option<&'static str> {
        let dark = probe_is_dark()?;
        let code = if dark { 2 } else { 1 };
        let mut last = LAST_SCHEME.lock().unwrap();
        if *last == code {
            return None;
        }
        *last = code;
        Some(if dark { "dark" } else { "light" })
    }

    /// Theme colour → native tinted glass on the dock (`None` clears it).
    pub fn set_tint(rgb: Option<(f64, f64, f64)>) {
        let ptr = *DOCK_GLASS.lock().unwrap();
        if ptr == 0 {
            return;
        }
        unsafe {
            let glass = &*(ptr as *const AnyObject);
            match rgb {
                Some((r, g, b)) => {
                    let color = NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, 0.34);
                    let _: () = msg_send![glass, setTintColor: &*color];
                }
                None => {
                    let _: () = msg_send![glass, setTintColor: std::ptr::null::<AnyObject>()];
                }
            }
        }
        tune_all();
    }

    /// Keep the native glass shape congruent with the CSS pill: at 36×36 (dot form) a 28pt radius
    /// renders as a rounded square peeking out from behind the circle, so clamp to half the side.
    pub fn set_corner_radius(radius: f64) {
        let ptr = *DOCK_GLASS.lock().unwrap();
        if ptr == 0 {
            return;
        }
        let strength = *STRENGTH.lock().unwrap();
        unsafe {
            let glass = &*(ptr as *const AnyObject);
            let _: () = msg_send![glass, setCornerRadius: radius];
            tune_glass_ptr(ptr, strength, true);
        }
    }

    pub fn set_material_strength(strength: f64) {
        let s = strength.clamp(0.0, 1.0);
        *STRENGTH.lock().unwrap() = s;
        tune_all();
    }

    pub fn set_settings_corner_radius(radius: f64) {
        *super::SETTINGS_RADIUS.lock().unwrap() = radius;
        let ptr = *SETTINGS_GLASS.lock().unwrap();
        if ptr != 0 {
            unsafe {
                let _: () = msg_send![&*(ptr as *const AnyObject), setCornerRadius: radius];
            }
        }
    }

    /// Give the settings window an empty unified toolbar. On macOS 26 that switches the window to
    /// the large toolbar corner radius; the toolbar itself draws nothing.
    pub fn adopt_unified_toolbar<R: Runtime>(win: &WebviewWindow<R>) {
        let (Ok(p), Some(cls)) = (win.ns_window(), AnyClass::get(c"NSToolbar")) else { return };
        unsafe {
            let window = &*(p as *mut NSWindow);
            let alloc: Allocated<AnyObject> = msg_send![cls, alloc];
            let ident = NSString::from_str("ai.we1jia.agentdock.settings");
            let toolbar: Retained<AnyObject> = msg_send![alloc, initWithIdentifier: &*ident];
            let _: () = msg_send![window, setToolbar: &*toolbar];
            if window.respondsToSelector(objc2::sel!(setToolbarStyle:)) {
                let _: () = msg_send![window, setToolbarStyle: 3isize]; // NSWindowToolbarStyleUnified
            }
            if window.respondsToSelector(objc2::sel!(setTitlebarSeparatorStyle:)) {
                let _: () = msg_send![window, setTitlebarSeparatorStyle: 1isize]; // none
            }
        }
    }

    /// The window's real corner radius (private `_cornerRadius`; 16pt is the pre-toolbar default).
    pub fn window_corner_radius<R: Runtime>(win: &WebviewWindow<R>) -> f64 {
        let Ok(p) = win.ns_window() else { return 16.0 };
        unsafe {
            let window = &*(p as *mut NSWindow);
            if window.respondsToSelector(objc2::sel!(_cornerRadius)) {
                let r: f64 = msg_send![window, _cornerRadius];
                if r > 0.0 {
                    return r;
                }
            }
        }
        16.0
    }

    pub fn tune_all() {
        let s = *STRENGTH.lock().unwrap();
        let dock_ptr = *DOCK_GLASS.lock().unwrap();
        let settings_ptr = *SETTINGS_GLASS.lock().unwrap();
        unsafe {
            if dock_ptr != 0 {
                tune_glass_ptr(dock_ptr, s, true);
            }
            if settings_ptr != 0 {
                tune_glass_ptr(settings_ptr, s, false);
            }
        }
    }

    unsafe fn tune_glass_ptr(ptr: usize, strength: f64, is_dock: bool) {
        if ptr == 0 {
            return;
        }
        let glass_view = &*(ptr as *const NSView);
        let _: () = msg_send![glass_view, layoutSubtreeIfNeeded];
        let size = glass_view.frame().size;
        let side = size.width.min(size.height);
        for sub in glass_view.subviews().iter() {
            let layer: *mut AnyObject = msg_send![&*sub, layer];
            tune_layer(layer, strength, is_dock, side);
        }
    }

    /// Tune the internal `SDFLayer` tree of `NSGlassEffectView`:
    /// - `CASDFGlassDisplacementEffect`: edge refraction band, scaled to the shape so a 36pt dot is
    ///   not refracted edge-to-edge (that read as several overlapping rims).
    /// - `CASDFKeyFillHighlightEffect`: one crisp specular rim close to the system default thickness
    ///   (macOS zeroes its opacity on non-key utility windows like the Dock).
    /// - Reduce the white platter backing opacity so the wallpaper refracts clearly through the glass.
    unsafe fn tune_layer(layer: *mut AnyObject, strength: f64, is_dock: bool, side: f64) {
        if layer.is_null() {
            return;
        }
        let cls: *const AnyClass = msg_send![layer, class];
        if !cls.is_null() {
            let cname = (*cls).name().to_string_lossy();
            if cname == "CASDFLayer" {
                let eff_key = NSString::from_str("effect");
                let eff: *mut AnyObject = msg_send![layer, valueForKey: &*eff_key];
                if !eff.is_null() {
                    let ecls: *const AnyClass = msg_send![eff, class];
                    if !ecls.is_null() {
                        let ename = (*ecls).name().to_string_lossy();
                        if ename == "CASDFGlassDisplacementEffect" {
                            // System default is 11.2pt; never more than ~a quarter of the short side.
                            let h: f64 = (0.55 + 0.6 * strength) * 11.2f64.min(side * 0.24);
                            let has_h: bool = msg_send![eff, respondsToSelector: objc2::sel!(setHeight:)];
                            if has_h {
                                let _: () = msg_send![eff, setHeight: h];
                            }
                            let has_c: bool = msg_send![eff, respondsToSelector: objc2::sel!(setCurvature:)];
                            if has_c {
                                let _: () = msg_send![eff, setCurvature: 1.0f64];
                            }
                        } else if ename == "CASDFKeyFillHighlightEffect" {
                            let op: f32 = (0.85 + strength * 0.15) as f32;
                            let _: () = msg_send![layer, setOpacity: op];
                            let has_kh: bool = msg_send![eff, respondsToSelector: objc2::sel!(setKeyHeight:)];
                            if has_kh {
                                let _: () = msg_send![eff, setKeyHeight: 1.0 + strength * 0.4];
                            }
                            let has_fh: bool = msg_send![eff, respondsToSelector: objc2::sel!(setFillHeight:)];
                            if has_fh {
                                let _: () = msg_send![eff, setFillHeight: 1.0 + strength * 0.3];
                            }
                            let has_c: bool = msg_send![eff, respondsToSelector: objc2::sel!(setCurvature:)];
                            if has_c {
                                let _: () = msg_send![eff, setCurvature: 0.8f64];
                            }
                            let has_kc: bool = msg_send![eff, respondsToSelector: objc2::sel!(setKeyColor:)];
                            if has_kc {
                                let key_ns = NSColor::colorWithWhite_alpha(1.0, 0.55 + strength * 0.35);
                                let key_cg: *mut AnyObject = msg_send![&*key_ns, CGColor];
                                let _: () = msg_send![eff, setKeyColor: key_cg];
                            }
                            let has_fc: bool = msg_send![eff, respondsToSelector: objc2::sel!(setFillColor:)];
                            if has_fc {
                                let fill_ns = NSColor::colorWithWhite_alpha(1.0, 0.25 + strength * 0.25);
                                let fill_cg: *mut AnyObject = msg_send![&*fill_ns, CGColor];
                                let _: () = msg_send![eff, setFillColor: fill_cg];
                            }
                        }
                    }
                }
            } else if cname == "CALayer" {
                let bg: *mut AnyObject = msg_send![layer, backgroundColor];
                let filters: *mut AnyObject = msg_send![layer, filters];
                if !bg.is_null() && !filters.is_null() {
                    let base = if is_dock { 0.08 + strength * 0.42 } else { 0.55 + strength * 0.35 };
                    let _: () = msg_send![layer, setOpacity: base as f32];
                }
            }
        }
        let sublayers: *mut AnyObject = msg_send![layer, sublayers];
        if !sublayers.is_null() {
            let count: usize = msg_send![sublayers, count];
            for i in 0..count {
                let sub: *mut AnyObject = msg_send![sublayers, objectAtIndex: i];
                tune_layer(sub, strength, is_dock, side);
            }
        }
    }

    /// WKWebView paints `underPageBackgroundColor` (the system window background — near-black in
    /// dark appearance) behind a page whose own background is transparent. It is a plain square, so
    /// in dark mode it showed as a dark box behind the dot/pill. Clear it on every webview subview.
    pub fn clear_webview_backdrop<R: Runtime>(win: &WebviewWindow<R>) -> Result<(), String> {
        let ns_window = win.ns_window().map_err(|e| e.to_string())? as *mut NSWindow;
        if ns_window.is_null() {
            return Err("no NSWindow".into());
        }
        unsafe {
            let window: &NSWindow = &*ns_window;
            let Some(content) = window.contentView() else { return Ok(()) };
            clear_recursive(&content, 0);
        }
        Ok(())
    }

    unsafe fn clear_recursive(view: &NSView, depth: usize) {
        for sub in view.subviews().iter() {
            let name = sub.class().name().to_string_lossy().into_owned();
            tracing::debug!("{}{name} {:?}", "  ".repeat(depth), sub.frame());
            if sub.respondsToSelector(objc2::sel!(setUnderPageBackgroundColor:)) {
                let prev: *mut AnyObject = msg_send![&*sub, underPageBackgroundColor];
                tracing::debug!("{name}: underPageBackgroundColor was {}", describe(prev));
                let _: () = msg_send![&*sub, setUnderPageBackgroundColor: &*NSColor::clearColor()];
            }
            if sub.respondsToSelector(objc2::sel!(setDrawsBackground:)) {
                let _: () = msg_send![&*sub, setDrawsBackground: false];
            }
            if sub.respondsToSelector(objc2::sel!(_setDrawsBackground:)) {
                let _: () = msg_send![&*sub, _setDrawsBackground: false];
            }
            clear_recursive(&sub, depth + 1);
        }
    }

    /// Diagnostic: log the window + whole contentView tree (class, frame, opacity, layer colour).
    pub fn dump_tree<R: Runtime>(win: &WebviewWindow<R>, tag: &str) {
        let Ok(p) = win.ns_window() else { return };
        unsafe {
            let window: &NSWindow = &*(p as *mut NSWindow);
            let bg = window.backgroundColor();
            tracing::debug!("[{tag}] window opaque={} hasShadow={} bg={bg:?} frame={:?}", window.isOpaque(), window.hasShadow(), window.frame());
            if let Some(c) = window.contentView() {
                dump_view(&c, 0, tag);
            }
        }
    }

    unsafe fn describe(obj: *mut AnyObject) -> String {
        if obj.is_null() {
            return "nil".into();
        }
        let d: *mut AnyObject = msg_send![obj, description];
        if d.is_null() {
            return "?".into();
        }
        (&*(d as *const objc2_foundation::NSString)).to_string()
    }

    unsafe fn dump_view(v: &NSView, depth: usize, tag: &str) {
        let name = v.class().name().to_string_lossy().into_owned();
        let layer: *mut AnyObject = msg_send![v, layer];
        let layer_desc = if layer.is_null() {
            "no-layer".to_string()
        } else {
            let bg: *mut AnyObject = msg_send![layer, backgroundColor];
            let r: f64 = msg_send![layer, cornerRadius];
            let m: bool = msg_send![layer, masksToBounds];
            let op: bool = msg_send![layer, isOpaque];
            format!("layer(bg={} r={r} mask={m} opaque={op})", describe(bg))
        };
        tracing::debug!("[{tag}] {}{name} frame={:?} opaque={} alpha={} wantsLayer={} {layer_desc}", "  ".repeat(depth), v.frame(), v.isOpaque(), v.alphaValue(), v.wantsLayer());
        for sub in v.subviews().iter() {
            dump_view(&sub, depth + 1, tag);
        }
    }

    pub fn apply<R: Runtime>(win: &WebviewWindow<R>, corner_radius: f64, is_dock: bool, strength: f64) -> Result<bool, String> {
        let Some(cls) = objc2::runtime::AnyClass::get(c"NSGlassEffectView") else { return Ok(false) };
        let ns_window = win.ns_window().map_err(|e| e.to_string())? as *mut NSWindow;
        if ns_window.is_null() {
            return Err("no NSWindow".into());
        }
        let s = strength.clamp(0.0, 1.0);
        *STRENGTH.lock().unwrap() = s;
        // SAFETY: Tauri hands us a live NSWindow pointer; we only call documented AppKit API on the
        // main thread (Tauri's setup hook runs there).
        unsafe {
            let window: &NSWindow = &*ns_window;
            if is_dock {
                window.setStyleMask(window.styleMask() & !objc2_app_kit::NSWindowStyleMask::Resizable);
            }
            window.setOpaque(false);
            window.setBackgroundColor(Some(&NSColor::clearColor()));
            window.setHasShadow(true);
            let Some(content) = window.contentView() else { return Err("no contentView".into()) };
            let bounds = content.bounds();
            let alloc: Allocated<AnyObject> = msg_send![cls, alloc];
            let glass: Retained<AnyObject> = msg_send![alloc, initWithFrame: bounds];
            let glass_view: &NSView = &*(Retained::as_ptr(&glass) as *const NSView);
            glass_view.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable);
            // Variant 19 activates the full Liquid Glass pipeline: `glassBackground` + `displacementMap`
            // (`CASDFGlassDisplacementEffect` optical edge lensing) + `glassForeground` + `CASDFKeyFillHighlightEffect`.
            if glass_view.respondsToSelector(objc2::sel!(set_variant:)) {
                let _: () = msg_send![&*glass, set_variant: 19isize];
            } else if glass_view.respondsToSelector(objc2::sel!(setStyle:)) {
                let _: () = msg_send![&*glass, setStyle: 1isize];
            }
            let _: () = msg_send![&*glass, setCornerRadius: corner_radius];
            let _: () = msg_send![&*glass, setTintColor: std::ptr::null::<AnyObject>()];
            if is_dock {
                if let Some(vcls) = AnyClass::get(c"NSView") {
                    let valloc: Allocated<AnyObject> = msg_send![vcls, alloc];
                    let probe: Retained<AnyObject> = msg_send![valloc, initWithFrame: bounds];
                    let probe_view: &NSView = &*(Retained::as_ptr(&probe) as *const NSView);
                    probe_view.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable);
                    let _: () = msg_send![&*glass, setContentView: &*probe];
                    *DOCK_PROBE.lock().unwrap() = Retained::into_raw(probe) as usize;
                }
            }
            content.addSubview_positioned_relativeTo(glass_view, NSWindowOrderingMode::Below, None);
            let ptr = Retained::into_raw(glass) as usize;
            if is_dock {
                *DOCK_GLASS.lock().unwrap() = ptr;
            } else {
                *SETTINGS_GLASS.lock().unwrap() = ptr;
            }
            // Keep the webview itself transparent so the glass shows through.
            for sub in content.subviews().iter() {
                if !std::ptr::eq(&*sub as *const NSView, glass_view) {
                    let _: () = msg_send![&*sub, setValue: &*NSColor::clearColor(), forKey: &*NSString::from_str("backgroundColor")];
                    if sub.respondsToSelector(objc2::sel!(setDrawsBackground:)) {
                        let _: () = msg_send![&*sub, setDrawsBackground: false];
                    }
                }
            }
            tune_glass_ptr(ptr, s, is_dock);
        }
        let _ = NSView::class();
        Ok(true)
    }
}

// ───────────────────────── tray ─────────────────────────

fn tray_menu(app: &AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
    let toggle = MenuItem::with_id(app, "toggle", i18n::t("menu.toggle_dock"), true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", i18n::t("menu.settings"), true, Some("CmdOrCtrl+,"))?;
    let quit = MenuItem::with_id(app, "quit", i18n::t("menu.quit"), true, Some("CmdOrCtrl+Q"))?;
    let language = language_submenu(app)?;
    Menu::with_items(app, &[&toggle, &language, &settings, &PredefinedMenuItem::separator(app)?, &quit])
}

/// After a language change: the tray menu and the settings window title follow.
fn apply_language(app: &AppHandle, language: settings::Language) {
    i18n::set(language);
    if let (Some(tray), Ok(menu)) = (app.tray_by_id("main"), tray_menu(app)) {
        let _ = tray.set_menu(Some(menu));
    }
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.set_title(&i18n::t("window.settings"));
    }
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    use tauri::tray::TrayIconBuilder;
    let menu = tray_menu(app)?;
    let mut builder = TrayIconBuilder::with_id("main").menu(&menu).tooltip("AgentDock").show_menu_on_left_click(true);
    // macOS templates use alpha only: the glass app tile would become a solid square.
    #[cfg(target_os = "macos")]
    {
        let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray-template.png"))?;
        builder = builder.icon(icon).icon_as_template(true);
    }
    #[cfg(not(target_os = "macos"))]
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder
        .on_menu_event(|app, ev| match ev.id().as_ref() {
            id if on_language_menu(app, id) => {}
            "toggle" => {
                if let Some(w) = app.get_webview_window("dock") {
                    if w.is_visible().unwrap_or(true) { let _ = w.hide(); } else { let _ = w.show(); }
                }
            }
            "settings" => {
                let _ = open_settings(app.clone());
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(())
}

// ───────────────────────── run ─────────────────────────

pub fn run() {
    // RUST_LOG wins when set (e.g. `agentdock_lib=debug` to trace driver steps)
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("agentdock=info,agentdock_lib=info")))
        .init();

    tauri::Builder::default()
        .on_page_load(|webview, payload| {
            #[cfg(target_os = "macos")]
            if (webview.label() == "dock" || webview.label() == "settings") && matches!(payload.event(), tauri::webview::PageLoadEvent::Finished) {
                if let Some(w) = webview.window().get_webview_window(webview.label()) {
                    if let Err(e) = liquid_glass::clear_webview_backdrop(&w) {
                        tracing::warn!("{} webview backdrop (post-load): {e}", webview.label());
                    }
                    liquid_glass::tune_all();
                    if webview.label() == "dock" {
                        liquid_glass::dump_tree(&w, "page-load");
                    }
                }
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (webview, payload);
        })
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| on_hotkey(app, shortcut, event.state() == ShortcutState::Pressed))
                .build(),
        )
        .setup(|app| {
            let config_dir = app.path().app_config_dir().unwrap_or_else(|_| std::env::temp_dir().join("agentdock"));
            std::fs::create_dir_all(&config_dir).ok();
            let registry = Registry::bundled().with_overrides(&config_dir.join("drivers"));
            let mut seats = SeatsConfig::load(&seats::seats_path(&config_dir));
            refresh_app_status(&mut seats.seats, &registry);
            let _ = seats.save(&seats::seats_path(&config_dir));
            let mut settings = AppSettings::load(&settings::settings_path(&config_dir));
            i18n::set(settings.language);
            let env = everos::MemoryEnv::from_env();
            if everos::assign_user_id(&mut settings.memory, &env) {
                if let Err(e) = settings.save(&settings::settings_path(&config_dir)) {
                    tracing::warn!("saving generated everos user id failed: {e}");
                }
            }
            let everos_client = everos::Client::resolve(&settings.memory, &env);
            if everos_client.is_active() {
                tracing::info!("everos memory on ({}) user {}", everos_client.status().base_url, everos_client.status().user_id);
            } else {
                tracing::info!("everos memory off: {}", everos_client.status().reason);
            }
            let mut machine = Machine::new();
            machine.state.expanded = !matches!(settings.default_form, settings::DefaultForm::Dot | settings::DefaultForm::Pet);
            let scale = settings.dock_scale.clamp(0.7, 1.6) as f64;
            let base_size = if !machine.state.expanded {
                match settings.default_form {
                    settings::DefaultForm::Pet => (92.0, 40.0),
                    _ => (36.0, 36.0),
                }
            } else if settings.orientation == settings::Orientation::Vertical {
                (56.0, 408.0)
            } else {
                (408.0, 56.0)
            };
            let init_size = ((base_size.0 * scale).round(), (base_size.1 * scale).round());
            app.manage(AppState {
                machine: Mutex::new(machine),
                seats: Mutex::new(seats),
                settings: Mutex::new(settings),
                registry,
                mic: mic::Mic::new(),
                config_dir,
                probes: Mutex::new(HashMap::new()),
                hotkey_map: Mutex::new(Vec::new()),
                logging_in: Mutex::new(Default::default()),
                driver_lane: Mutex::new(()),
                dock_size: Mutex::new(init_size),
                memory: Mutex::new(memory::CallBook::default()),
                everos: Mutex::new(everos_client),
                chrome: Default::default(),
            });

            let dock = app.get_webview_window("dock").expect("dock window");
            let settings_win = app.get_webview_window("settings");
            let init_strength = app.state::<AppState>().settings.lock().unwrap().material_strength as f64;
            apply_glass(&dock, settings_win.as_ref(), init_strength);
            #[cfg(target_os = "macos")]
            {
                let accent = app.state::<AppState>().settings.lock().unwrap().accent.clone();
                liquid_glass::set_tint(accent.as_deref().and_then(settings::parse_hex_color));
                spawn_glass_scheme_watcher(app.handle().clone());
            }
            apply_theme(app.handle(), app.state::<AppState>().settings.lock().unwrap().theme);
            {
                let (snap, anchor) = {
                    let st = app.state::<AppState>();
                    let s = st.settings.lock().unwrap();
                    (s.snap, s.dock_anchor)
                };
                #[cfg(target_os = "macos")]
                liquid_glass::set_corner_radius(init_size.0.min(init_size.1) / 2.0);
                let _ = position_dock(&dock, init_size.0, init_size.1, snap, anchor);
            }
            if let Some(w) = &settings_win {
                let app2 = app.handle().clone();
                w.on_window_event(move |ev| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = ev {
                        api.prevent_close();
                        if let Some(w) = app2.get_webview_window("settings") { let _ = w.hide(); }
                    }
                });
            }
            build_tray(app.handle())?;
            if let Some(w) = app.get_webview_window("settings") {
                let _ = w.set_title(&i18n::t("window.settings"));
            }
            let icon_mode = app.state::<AppState>().settings.lock().unwrap().app_icon;
            apply_app_icon(app.handle(), icon_mode);
            register_hotkeys(app.handle())?;
            app.on_menu_event(|app, ev| on_app_menu(app, ev.id().as_ref()));
            spawn_hover_watcher(app.handle().clone());

            // Tick: 1 Hz state ticks; probe the active seat at 2 Hz and idle seats every 10 s.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut n: u64 = 0;
                loop {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    n += 1;
                    if n % 2 == 0 {
                        dispatch(&handle, Event::Tick);
                    }
                    let st = handle.state::<AppState>();
                    let active = st.machine.lock().unwrap().current_seat().map(str::to_string);
                    let phase_idle = matches!(st.machine.lock().unwrap().state.phase, Phase::Idle);
                    let memory_on = st.everos.lock().unwrap().is_active();
                    let seats: Vec<Seat> = st.seats.lock().unwrap().seats.clone();
                    let logging_in: Vec<String> = st.logging_in.lock().unwrap().iter().cloned().collect();
                    // Mid call / hang-up nothing is settled: in a switch between two Bots of one
                    // app, the old call is still up and would read as the new one connecting.
                    let drivers_busy = st.driver_lane.try_lock().is_err();
                    for seat in seats.iter().filter(|s| s.enabled || logging_in.contains(&s.id)) {
                        let is_active = active.as_deref() == Some(seat.id.as_str());
                        let is_login = logging_in.contains(&seat.id);
                        let has_view = handle.get_webview_window(&web::window_label(&seat.id)).is_some() || st.chrome.has_tab(&seat.id);
                        let is_app = matches!(seat.driver, DriverKind::App { .. });
                        // An app probe walks the whole accessibility tree; 1.5 s is plenty there.
                        let due = (is_active && (!is_app || n % 3 == 0)) || is_login || (n % 20 == 0 && (has_view || is_app));
                        if due && !drivers_busy && (!phase_idle || has_view || is_login || is_app) {
                            let _ = run_driver(&handle, &seat.id, DriverOp::Probe);
                        }
                        // Visible captions about every 2s while this seat is the live call.
                        if memory_on && is_active && !phase_idle && n % 4 == 0 {
                            if let DriverKind::Web { driver, .. } = &seat.driver {
                                let selectors = st.registry.get(driver).map(|spec| spec.memory.transcript.clone()).unwrap_or_default();
                                let script = drivers::transcript_script(&selectors, &seat.id);
                                if let Some(win) = handle.get_webview_window(&web::window_label(&seat.id)) {
                                    let _ = win.eval(&script);
                                } else if st.chrome.has_tab(&seat.id) {
                                    let lines = st.chrome.eval(&seat.id, "", &script, false).ok().and_then(|v| serde_json::from_value::<Vec<memory::TranscriptLine>>(v).ok());
                                    if let Some(lines) = lines.filter(|l| !l.is_empty()) {
                                        memory_lines(&handle, &seat.id, &lines);
                                    }
                                }
                            }
                        }
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_state, call_seat, hangup, toggle_mute, set_muted, toggle_expanded, set_expanded, undo_switch, resize_dock, drag_dock, remember_dock_position, ui_log, cursor_over_dock, toggle_orientation, toggle_collapsed_form, dock_menu,
            get_seats, save_seats, reorder_seats, list_drivers, connect_account, disconnect_seat, show_seat, test_call, probe_seat, chrome_status, app_targets,
            driver_report, driver_error, memory_report, memory_status, set_seat_icon, reset_seat_icon, seat_icon_data,
            get_settings, save_settings, hotkey_conflicts, open_settings, quit, host_info, glass_scheme
        ])
        .build(tauri::generate_context!())
        .expect("error while building AgentDock")
        .run(|app, event| {
            // A mute is the whole Mac's microphone: it must not outlive AgentDock.
            if matches!(event, tauri::RunEvent::Exit) {
                let st = app.state::<AppState>();
                if st.machine.lock().unwrap().state.muted && st.settings.lock().unwrap().mute_mode == settings::MuteMode::System {
                    if let Err(e) = st.mic.set_muted(false) {
                        tracing::warn!("restoring the microphone on exit failed: {e}");
                    }
                }
            }
            // Clicking the Dock icon opens the settings window (the pill is always on screen already).
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                let _ = open_settings(app.clone());
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app, event);
        });
}
