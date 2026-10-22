//! App seats (Grok Bot, native Devin): drive the desktop client through the OS accessibility tree.
//! macOS talks to the AX API directly and presses buttons with `AXPress`, so the app stays in the
//! background; Windows will use UI Automation; other hosts report `Unsupported`.
//!
//! Selectors in `drivers/*.json` for app seats use the mini-grammar `AXRole.domclass[Title]`, e.g.
//! `AXButton[Start voice chat]`; the `.domclass` part is optional (Chromium exposes DOM classes).
//! The title is compared with the element's title, description, help text and string value
//! (Chromium puts `aria-label` in the description, text in the value): `[text]` is an exact match,
//! `[~part]` a label containing `part`, `[@text]` an element whose first text inside is `text`.

use super::{DriverError, DriverSpec, ProbeResult, Step};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Title {
    Any,
    Exact(String),
    Contains(String),
    /// A list row whose name is followed by a preview, all of which lands in its label.
    FirstText(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxSelector {
    pub role: String,
    pub class: Option<String>,
    pub title: Title,
}

impl AxSelector {
    /// Role and DOM class: cheap, checked on every node.
    pub fn kind_matches(&self, role: &str, classes: &[String]) -> bool {
        self.role == role && self.class.as_ref().map_or(true, |c| classes.contains(c))
    }

    /// `first_text` is only consulted for `[@text]`.
    pub fn title_matches(&self, labels: &[&str], first_text: impl FnOnce() -> Option<String>) -> bool {
        match &self.title {
            Title::Any => true,
            Title::Exact(t) => labels.iter().any(|l| l.trim() == t),
            Title::Contains(t) => labels.iter().any(|l| l.contains(t.as_str())),
            Title::FirstText(t) => first_text().is_some_and(|f| f.trim() == t),
        }
    }
}

pub fn parse_selector(s: &str) -> Option<AxSelector> {
    let s = s.trim();
    let (head, rest) = match s.find('[') {
        Some(i) => (&s[..i], Some(&s[i + 1..s.len().checked_sub(1)?])),
        None => (s, None),
    };
    let (role, class) = match head.split_once('.') {
        Some((r, c)) => (r, Some(c.to_string())),
        None => (head, None),
    };
    if !role.starts_with("AX") {
        return None;
    }
    let title = match rest.filter(|t| !t.is_empty()) {
        None => Title::Any,
        Some(t) => match (t.strip_prefix('~'), t.strip_prefix('@')) {
            (Some(p), _) if !p.is_empty() => Title::Contains(p.to_string()),
            (_, Some(p)) if !p.is_empty() => Title::FirstText(p.to_string()),
            _ => Title::Exact(t.to_string()),
        },
    };
    Some(AxSelector { role: role.to_string(), class, title })
}

/// One entry of an app's roster: its name, its id, and why it cannot take a call, if it cannot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterEntry {
    pub name: String,
    pub id: Option<String>,
    pub blocked: Option<String>,
}

/// RFC 4648 base32 in any case, padding optional: the roster store's file names.
fn base32_decode(s: &str) -> Option<Vec<u8>> {
    let (mut acc, mut bits, mut out) = (0u32, 0u32, Vec::new());
    for c in s.bytes().filter(|c| *c != b'=') {
        let v = match c {
            b'a'..=b'z' => c - b'a',
            b'A'..=b'Z' => c - b'A',
            b'2'..=b'7' => c - b'2' + 26,
            _ => return None,
        };
        acc = (acc << 5) | v as u32;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// The newest `<base32(key)>.blob` in `dir` whose key ends with the spec's suffix, as entries.
pub fn read_roster(spec: &super::RosterSpec, dir: &std::path::Path) -> Option<Vec<RosterEntry>> {
    let newest = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let file = e.file_name();
            let key = String::from_utf8(base32_decode(file.to_str()?.strip_suffix(".blob")?)?).ok()?;
            key.ends_with(&spec.key_suffix).then(|| (e.metadata().and_then(|m| m.modified()).ok(), e.path()))
        })
        .max_by_key(|(modified, _)| *modified)?
        .1;
    let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(newest).ok()?).ok()?;
    let rows = json.pointer(&spec.rows)?.as_array()?;
    Some(
        rows.iter()
            .filter_map(|row| {
                let name = row.get(&spec.name)?.as_str()?.trim().to_string();
                let id = spec.id.as_ref().and_then(|field| row.get(field)?.as_str()).map(str::to_string);
                let blocked = spec.exclude.iter().find(|(field, _)| row.get(field.as_str()).and_then(|v| v.as_bool()) == Some(true)).map(|(_, why)| why.clone());
                (!name.is_empty()).then_some(RosterEntry { name, id, blocked })
            })
            .collect(),
    )
}

fn parse_all(selectors: &[String]) -> Result<Vec<AxSelector>, DriverError> {
    selectors.iter().map(|s| parse_selector(s).ok_or_else(|| DriverError::Step(format!("bad AX selector {s}")))).collect()
}

/// How long a click waits for its button: the web content of an Electron app is built lazily
/// after we ask for it, and a freshly launched app still has to load its page.
const CLICK_WAIT: std::time::Duration = std::time::Duration::from_secs(4);
const LAUNCH_WAIT: std::time::Duration = std::time::Duration::from_secs(15);
/// How long a click with a link to fall back on waits for its element in a running app.
const LINK_WAIT: std::time::Duration = std::time::Duration::from_secs(2);
/// How long a hang-up may take to clear the call controls.
const HANGUP_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Plain,
    Call,
    Hangup,
}

#[derive(Debug, Default)]
pub struct AppDriver;

#[cfg(target_os = "macos")]
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: *const std::ffi::c_void) -> bool;
}

impl AppDriver {
    pub fn resolve_bundle_id(bundle_id: &str) -> &str {
        if bundle_id == "com.cursor.grokbot" {
            "com.anysphere.sand"
        } else {
            bundle_id
        }
    }

    pub fn is_installed(bundle_id: &str) -> bool {
        #[cfg(target_os = "macos")]
        {
            use objc2_app_kit::NSWorkspace;
            use objc2_foundation::NSString;
            let bid = Self::resolve_bundle_id(bundle_id);
            let found = NSWorkspace::sharedWorkspace().URLForApplicationWithBundleIdentifier(&NSString::from_str(bid)).is_some();
            return found || (bid == "com.anysphere.sand" && std::path::Path::new("/Applications/Grok Bot.app").exists());
        }
        #[allow(unreachable_code)]
        {
            let _ = bundle_id;
            false
        }
    }

    pub fn is_ax_trusted() -> bool {
        #[cfg(target_os = "macos")]
        unsafe {
            return AXIsProcessTrusted();
        }
        #[allow(unreachable_code)]
        false
    }

    /// Trigger the native macOS Accessibility permission prompt so the current binary is added to
    /// System Settings › Privacy & Security › Accessibility automatically.
    pub fn prompt_ax_trust() -> bool {
        #[cfg(target_os = "macos")]
        unsafe {
            use objc2_foundation::{NSNumber, NSString};
            let key = NSString::from_str("AXTrustedCheckOptionPrompt");
            let val = NSNumber::new_bool(true);
            let dict: *mut objc2::runtime::AnyObject = objc2::msg_send![
                objc2::class!(NSDictionary),
                dictionaryWithObject: &*val,
                forKey: &*key
            ];
            return AXIsProcessTrustedWithOptions(dict as *const std::ffi::c_void);
        }
        #[allow(unreachable_code)]
        false
    }

    pub fn run_steps(spec: &DriverSpec, steps: &[Step]) -> Result<(), DriverError> {
        Self::run(spec, steps, Mode::Plain)
    }

    /// Call steps. Most apps hold one call at a time: if a control is missing while the app still
    /// shows a call (not hung up yet, or started outside AgentDock), that call is ended first.
    #[cfg(target_os = "macos")]
    pub fn place_call(spec: &DriverSpec, steps: &[Step]) -> Result<(), DriverError> {
        Self::run(spec, steps, Mode::Call)
    }

    /// Hang-up steps, then a (bounded) wait until the call controls are gone, so a call placed
    /// right after, in a switch, finds the app ready for it.
    #[cfg(target_os = "macos")]
    pub fn hang_up(spec: &DriverSpec, steps: &[Step]) -> Result<(), DriverError> {
        Self::run(spec, steps, Mode::Hangup)
    }

    #[cfg(target_os = "macos")]
    fn run(spec: &DriverSpec, steps: &[Step], mode: Mode) -> Result<(), DriverError> {
        let bundle = Self::resolve_bundle_id(spec.bundle_id.as_deref().ok_or(DriverError::Unsupported)?);
        if !Self::is_ax_trusted() {
            return Err(DriverError::NeedsAuth);
        }
        let (pid, launched) = Self::ensure_running(bundle)?;
        let app = ax::app(pid);
        let in_call = parse_all(&spec.probe.in_call)?;
        let mut wait = if launched { LAUNCH_WAIT } else { CLICK_WAIT };
        let mut ended_stray = false;
        for step in steps {
            match step {
                Step::Click { selectors, open } => {
                    let sels = parse_all(selectors)?;
                    let link = match open {
                        Some(o) => Some((o.url.as_str(), parse_all(&o.until)?)),
                        None => None,
                    };
                    // already where the click would go, e.g. the Bot's chat is the one open
                    if link.as_ref().is_some_and(|(_, until)| !until.is_empty() && ax::find(&app, until).is_some()) {
                        continue;
                    }
                    let mut found = ax::wait_for(&app, &sels, if link.is_some() && !launched { LINK_WAIT } else { wait });
                    if found.is_none() && mode == Mode::Call && !ended_stray && Self::end_call(&app, spec, &in_call) {
                        ended_stray = true;
                        found = ax::wait_for(&app, &sels, CLICK_WAIT);
                    }
                    match (found, &link) {
                        (Some(el), _) => {
                            if !ax::press(&el) {
                                return Err(DriverError::Step(format!("AXPress failed: {}", selectors.join(" | "))));
                            }
                        }
                        (None, Some((url, until))) if Self::follow_link(&app, bundle, url, until) => {}
                        (None, _) => return Err(DriverError::NotFound(selectors.join(" | "))),
                    }
                    wait = CLICK_WAIT;
                }
                Step::Clear { selectors } => {
                    if let Some(el) = ax::find(&app, &parse_all(selectors)?) {
                        ax::set_value(&el, "");
                    }
                }
                Step::Sleep { ms } => std::thread::sleep(std::time::Duration::from_millis(*ms)),
                Step::WaitFor { selectors, timeout_ms } => {
                    ax::wait_for(&app, &parse_all(selectors)?, std::time::Duration::from_millis(*timeout_ms)).ok_or_else(|| DriverError::Step("timeout".into()))?;
                }
                Step::Key { key } => {
                    run_osascript(&format!("tell application \"System Events\" to keystroke \"{}\"", key.replace('"', "\\\"")))?;
                }
                Step::Goto { .. } => {}
            }
        }
        if mode == Mode::Hangup {
            ax::wait_gone(&app, &in_call, HANGUP_WAIT);
        }
        Ok(())
    }

    /// End a call the app is still showing: press its hang-up control and wait for the call
    /// controls to go. `false` when there was no call to end.
    #[cfg(target_os = "macos")]
    fn end_call(app: &ax::Cf, spec: &DriverSpec, in_call: &[AxSelector]) -> bool {
        if in_call.is_empty() || ax::find(app, in_call).is_none() {
            return false;
        }
        tracing::info!("{}: a call is still up, ending it before the next one", spec.id);
        for step in &spec.hangup {
            if let Step::Click { selectors, .. } = step {
                if let Some(el) = parse_all(selectors).ok().and_then(|s| ax::find(app, &s)) {
                    ax::press(&el);
                }
            }
        }
        ax::wait_gone(app, in_call, HANGUP_WAIT);
        true
    }

    /// Open a deep link the app handles itself and wait until `until` shows it landed. The app
    /// brings itself forward to handle a link; whatever was in front goes back in front, so the
    /// rest of the call still happens behind it.
    #[cfg(target_os = "macos")]
    fn follow_link(app: &ax::Cf, bundle: &str, url: &str, until: &[AxSelector]) -> bool {
        tracing::info!("{bundle}: nothing to press, opening {url}");
        let front = ax::frontmost().filter(|(_, b)| b != bundle);
        let opened = std::process::Command::new("open").args(["-g", url]).status().is_ok_and(|s| s.success());
        let landed = opened && if until.is_empty() { std::thread::sleep(CLICK_WAIT); true } else { ax::wait_for(app, until, LAUNCH_WAIT).is_some() };
        if let Some((pid, _)) = front.filter(|_| ax::frontmost().is_some_and(|(_, b)| b == bundle)) {
            ax::activate(pid);
        }
        landed
    }

    #[cfg(not(target_os = "macos"))]
    fn run(_spec: &DriverSpec, _steps: &[Step], _mode: Mode) -> Result<(), DriverError> {
        Err(DriverError::Unsupported)
    }

    #[cfg(not(target_os = "macos"))]
    pub fn place_call(_spec: &DriverSpec, _steps: &[Step]) -> Result<(), DriverError> {
        Err(DriverError::Unsupported)
    }

    #[cfg(not(target_os = "macos"))]
    pub fn hang_up(_spec: &DriverSpec, _steps: &[Step]) -> Result<(), DriverError> {
        Err(DriverError::Unsupported)
    }

    #[cfg(target_os = "macos")]
    pub fn probe(spec: &DriverSpec) -> Result<ProbeResult, DriverError> {
        let bundle = Self::resolve_bundle_id(spec.bundle_id.as_deref().ok_or(DriverError::Unsupported)?);
        if !Self::is_installed(bundle) {
            return Err(DriverError::Unsupported);
        }
        if !Self::is_ax_trusted() {
            return Err(DriverError::NeedsAuth);
        }
        let Some(pid) = ax::pid_of(bundle) else {
            return Ok(ProbeResult { logged_in: true, ..Default::default() });
        };
        let groups = [parse_all(&spec.probe.in_call)?, parse_all(&spec.probe.muted)?, parse_all(&spec.probe.needs_human)?];
        let found = ax::scan(&ax::app(pid), &groups);
        Ok(ProbeResult { in_call: found[0], muted: found[1], needs_human: found[2], logged_in: true, account: None, url: None })
    }

    #[cfg(not(target_os = "macos"))]
    pub fn probe(_spec: &DriverSpec) -> Result<ProbeResult, DriverError> {
        Err(DriverError::Unsupported)
    }

    /// The app's cached roster (`spec.roster`), if it has one and it can be read.
    pub fn roster(spec: &DriverSpec) -> Option<Vec<RosterEntry>> {
        let r = spec.roster.as_ref()?;
        let dir = match r.dir.strip_prefix("~/") {
            Some(rest) => dirs::home_dir()?.join(rest),
            None => std::path::PathBuf::from(&r.dir),
        };
        read_roster(r, &dir)
    }

    /// Why `target` cannot take a call according to the app's roster, e.g. it is a group chat.
    pub fn refusal(spec: &DriverSpec, target: &str) -> Option<String> {
        let target = target.trim();
        let why = Self::roster(spec)?.into_iter().find(|e| e.name == target)?.blocked?;
        // `why` is a key into the native string table ("group_chat"), or plain text from a user driver
        Some(crate::i18n::lookup(&format!("roster.{why}")).map_or_else(|| format!("「{target}」{why}"), |s| s.replace("{target}", target)))
    }

    /// `target`'s id in the app's roster, for the `{target_id}` of an `open` link.
    pub fn target_id(spec: &DriverSpec, target: &str) -> Option<String> {
        let target = target.trim();
        Self::roster(spec)?.into_iter().find(|e| e.name == target)?.id
    }

    /// What the app offers to call, e.g. the Bots in Grok Bot: from its roster when it has one
    /// (minus group chats and hidden entries), otherwise read off its sidebar (`spec.targets`).
    pub fn targets(spec: &DriverSpec) -> Result<Vec<String>, DriverError> {
        if let Some(entries) = Self::roster(spec) {
            let names: Vec<String> = entries.into_iter().filter(|e| e.blocked.is_none()).map(|e| e.name).collect();
            if !names.is_empty() {
                return Ok(names);
            }
        }
        Self::sidebar_targets(spec)
    }

    #[cfg(target_os = "macos")]
    fn sidebar_targets(spec: &DriverSpec) -> Result<Vec<String>, DriverError> {
        let bundle = Self::resolve_bundle_id(spec.bundle_id.as_deref().ok_or(DriverError::Unsupported)?);
        let sels = parse_all(&spec.targets)?;
        if sels.is_empty() {
            return Ok(Vec::new());
        }
        if !Self::is_ax_trusted() {
            return Err(DriverError::NeedsAuth);
        }
        let (pid, launched) = Self::ensure_running(bundle)?;
        let app = ax::app(pid);
        let wait = if launched { LAUNCH_WAIT } else { CLICK_WAIT };
        let t0 = std::time::Instant::now();
        loop {
            let names = ax::list(&app, &sels);
            if !names.is_empty() || t0.elapsed() >= wait {
                return Ok(names);
            }
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn sidebar_targets(_spec: &DriverSpec) -> Result<Vec<String>, DriverError> {
        Err(DriverError::Unsupported)
    }

    /// The app's pid, plus whether it had to be (re)opened. A missing app is launched hidden and a
    /// running one without windows gets a reopen event; neither is activated.
    #[cfg(target_os = "macos")]
    fn ensure_running(bundle: &str) -> Result<(i32, bool), DriverError> {
        if let Some(pid) = ax::pid_of(bundle) {
            if ax::has_window(&ax::app(pid)) {
                return Ok((pid, false));
            }
        }
        let opened = std::process::Command::new("open").args(["-g", "-j", "-b", bundle]).status().map_err(|e| DriverError::Step(e.to_string()))?;
        if !opened.success() {
            return Err(DriverError::Unsupported);
        }
        let t0 = std::time::Instant::now();
        while t0.elapsed() < LAUNCH_WAIT {
            if let Some(pid) = ax::pid_of(bundle) {
                return Ok((pid, true));
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        Err(DriverError::Step(format!("{bundle} did not start")))
    }

    /// Bring the app forward: only when the user asks to look at it.
    pub fn activate(bundle_id: &str) -> Result<(), DriverError> {
        let bid = Self::resolve_bundle_id(bundle_id);
        run_osascript(&format!("tell application id \"{bid}\" to activate")).map(|_| ())
    }
}

#[cfg(target_os = "macos")]
mod ax {
    use super::AxSelector;
    use objc2::rc::Retained;
    use objc2_app_kit::NSRunningApplication;
    use objc2_foundation::{NSArray, NSString};
    use std::ffi::c_void;

    type CFTypeRef = *const c_void;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXUIElementCreateApplication(pid: i32) -> CFTypeRef;
        fn AXUIElementCopyAttributeValue(el: CFTypeRef, attr: CFTypeRef, value: *mut CFTypeRef) -> i32;
        fn AXUIElementCopyMultipleAttributeValues(el: CFTypeRef, attrs: CFTypeRef, options: u32, values: *mut CFTypeRef) -> i32;
        fn AXUIElementSetAttributeValue(el: CFTypeRef, attr: CFTypeRef, value: CFTypeRef) -> i32;
        fn AXUIElementPerformAction(el: CFTypeRef, action: CFTypeRef) -> i32;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFBooleanTrue: CFTypeRef;
        fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
        fn CFRelease(cf: CFTypeRef);
        fn CFGetTypeID(cf: CFTypeRef) -> usize;
        fn CFStringGetTypeID() -> usize;
        fn CFArrayGetTypeID() -> usize;
        fn CFArrayGetCount(a: CFTypeRef) -> isize;
        fn CFArrayGetValueAtIndex(a: CFTypeRef, i: isize) -> CFTypeRef;
    }

    /// An owned (+1) Core Foundation reference.
    pub struct Cf(CFTypeRef);
    impl Drop for Cf {
        fn drop(&mut self) {
            unsafe { CFRelease(self.0) }
        }
    }

    /// Chat histories can be long; past this many nodes a search gives up.
    const NODE_BUDGET: usize = 20_000;
    /// Roles whose subtree never holds a control.
    const LEAF_ROLES: &[&str] = &["AXStaticText", "AXImage", "AXButton", "AXCheckBox", "AXLink", "AXTextArea", "AXTextField"];

    fn cf(s: &NSString) -> CFTypeRef {
        s as *const NSString as CFTypeRef
    }
    fn key(s: &str) -> Retained<NSString> {
        NSString::from_str(s)
    }

    unsafe fn as_string(v: CFTypeRef) -> Option<String> {
        (!v.is_null() && CFGetTypeID(v) == CFStringGetTypeID()).then(|| (*(v as *const NSString)).to_string())
    }
    unsafe fn as_elements(v: CFTypeRef) -> Vec<Cf> {
        if v.is_null() || CFGetTypeID(v) != CFArrayGetTypeID() {
            return Vec::new();
        }
        (0..CFArrayGetCount(v)).map(|i| Cf(CFRetain(CFArrayGetValueAtIndex(v, i)))).collect()
    }
    unsafe fn as_strings(v: CFTypeRef) -> Vec<String> {
        if v.is_null() || CFGetTypeID(v) != CFArrayGetTypeID() {
            return Vec::new();
        }
        (0..CFArrayGetCount(v)).filter_map(|i| as_string(CFArrayGetValueAtIndex(v, i))).collect()
    }
    fn attr(el: &Cf, name: &str) -> Option<Cf> {
        unsafe {
            let mut v: CFTypeRef = std::ptr::null();
            (AXUIElementCopyAttributeValue(el.0, cf(&key(name)), &mut v) == 0 && !v.is_null()).then(|| Cf(v))
        }
    }
    fn string_attr(el: &Cf, name: &str) -> Option<String> {
        attr(el, name).and_then(|v| unsafe { as_string(v.0) })
    }
    fn children(el: &Cf) -> Vec<Cf> {
        attr(el, "AXChildren").map(|v| unsafe { as_elements(v.0) }).unwrap_or_default()
    }

    /// The first non-empty static text inside `el`, in reading order (a row's name before its preview).
    fn first_text(el: &Cf) -> Option<String> {
        let mut stack = children(el);
        stack.reverse();
        for _ in 0..40 {
            let c = stack.pop()?;
            if string_attr(&c, "AXRole").as_deref() == Some("AXStaticText") {
                if let Some(t) = string_attr(&c, "AXValue").or_else(|| string_attr(&c, "AXTitle")).filter(|t| !t.trim().is_empty()) {
                    return Some(t.trim().to_string());
                }
            }
            let mut kids = children(&c);
            kids.reverse();
            stack.extend(kids);
        }
        None
    }
    fn windows(app: &Cf) -> Vec<Cf> {
        unsafe {
            let mut v: CFTypeRef = std::ptr::null();
            if AXUIElementCopyAttributeValue(app.0, cf(&key("AXWindows")), &mut v) != 0 || v.is_null() {
                return Vec::new();
            }
            let v = Cf(v);
            as_elements(v.0)
        }
    }

    /// The app the user is in: its pid and bundle id.
    pub fn frontmost() -> Option<(i32, String)> {
        let app = objc2_app_kit::NSWorkspace::sharedWorkspace().frontmostApplication()?;
        Some((app.processIdentifier(), app.bundleIdentifier()?.to_string()))
    }

    pub fn activate(pid: i32) -> bool {
        NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
            .is_some_and(|a| a.activateWithOptions(objc2_app_kit::NSApplicationActivationOptions::empty()))
    }

    pub fn pid_of(bundle: &str) -> Option<i32> {
        NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(bundle))
            .iter()
            .map(|a| a.processIdentifier())
            .find(|pid| *pid > 0)
    }

    /// The app's AX root. Electron/Chromium only builds the accessibility tree of its web content
    /// once an assistive client asks for it, the way VoiceOver does; until then no button inside
    /// the page exists for us.
    pub fn app(pid: i32) -> Cf {
        unsafe {
            let el = Cf(AXUIElementCreateApplication(pid));
            AXUIElementSetAttributeValue(el.0, cf(&key("AXManualAccessibility")), kCFBooleanTrue);
            el
        }
    }

    pub fn has_window(app: &Cf) -> bool {
        !windows(app).is_empty()
    }

    /// Walk every window depth-first. With `both_ends`, two frontiers alternate: one from the top
    /// of the document (sidebar, header, call bar) and one from the bottom (composer), so a long
    /// chat history in the middle, which can outgrow the node budget, is reached last.
    /// `visit` gets each node's matched selector group and index, and returns `true` to stop.
    fn walk(app: &Cf, groups: &[Vec<AxSelector>], both_ends: bool, mut visit: impl FnMut(&Cf, usize, usize) -> bool) {
        let attrs = NSArray::from_retained_slice(&[
            key("AXRole"), key("AXTitle"), key("AXDescription"), key("AXHelp"), key("AXValue"), key("AXDOMClassList"), key("AXChildren"),
        ]);
        let attrs = Retained::as_ptr(&attrs) as CFTypeRef;
        let mut queue: std::collections::VecDeque<Cf> = windows(app).into();
        let mut from_back = false;
        let mut seen = 0;
        while let Some(el) = if from_back { queue.pop_back() } else { queue.pop_front() } {
            seen += 1;
            if seen > NODE_BUDGET {
                tracing::warn!("ax search stopped after {NODE_BUDGET} nodes");
                return;
            }
            unsafe {
                let mut vals: CFTypeRef = std::ptr::null();
                if AXUIElementCopyMultipleAttributeValues(el.0, attrs, 0, &mut vals) == 0 && !vals.is_null() {
                    let vals = Cf(vals);
                    if CFArrayGetCount(vals.0) >= 7 {
                        let get = |i| CFArrayGetValueAtIndex(vals.0, i);
                        let role = as_string(get(0)).unwrap_or_default();
                        let labels: Vec<String> = (1..5).filter_map(|i| as_string(get(i))).collect();
                        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
                        let classes = as_strings(get(5));
                        for (g, sels) in groups.iter().enumerate() {
                            if let Some(i) = sels.iter().position(|s| s.kind_matches(&role, &classes) && s.title_matches(&labels, || first_text(&el))) {
                                if visit(&el, g, i) {
                                    return;
                                }
                            }
                        }
                        if !LEAF_ROLES.contains(&role.as_str()) {
                            let kids = as_elements(get(6));
                            if from_back {
                                queue.extend(kids);
                            } else {
                                for k in kids.into_iter().rev() {
                                    queue.push_front(k);
                                }
                            }
                        }
                    }
                }
            }
            from_back = both_ends && !from_back;
        }
    }

    /// First element matching any selector, preferring earlier selectors.
    pub fn find(app: &Cf, sels: &[AxSelector]) -> Option<Cf> {
        let mut best: Option<(usize, Cf)> = None;
        walk(app, std::slice::from_ref(&sels.to_vec()), true, |el, _, i| {
            if best.as_ref().map_or(true, |(b, _)| i < *b) {
                best = Some((i, Cf(unsafe { CFRetain(el.0) })));
            }
            i == 0
        });
        best.map(|(_, el)| el)
    }

    /// Which selector groups have a match, in one walk that ends as soon as the first group
    /// (in call) is found: that answer is what the call state needs, and it comes cheap mid-call.
    pub fn scan(app: &Cf, groups: &[Vec<AxSelector>]) -> Vec<bool> {
        let mut found = vec![false; groups.len()];
        walk(app, groups, true, |_, g, _| {
            found[g] = true;
            found[0]
        });
        found
    }

    /// The first text of every element matching `sels` (e.g. Bot rows in a sidebar), in document
    /// order, without duplicates.
    pub fn list(app: &Cf, sels: &[AxSelector]) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        walk(app, std::slice::from_ref(&sels.to_vec()), false, |el, _, _| {
            if let Some(t) = first_text(el).filter(|t| !names.contains(t)) {
                names.push(t);
            }
            false
        });
        names
    }

    /// Wait (bounded) until nothing matches `sels` any more.
    pub fn wait_gone(app: &Cf, sels: &[AxSelector], timeout: std::time::Duration) {
        let t0 = std::time::Instant::now();
        while !sels.is_empty() && t0.elapsed() < timeout && find(app, sels).is_some() {
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    }

    pub fn wait_for(app: &Cf, sels: &[AxSelector], timeout: std::time::Duration) -> Option<Cf> {
        let t0 = std::time::Instant::now();
        loop {
            if let Some(el) = find(app, sels) {
                return Some(el);
            }
            if t0.elapsed() >= timeout {
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
    }

    pub fn press(el: &Cf) -> bool {
        unsafe { AXUIElementPerformAction(el.0, cf(&key("AXPress"))) == 0 }
    }

    pub fn set_value(el: &Cf, value: &str) -> bool {
        unsafe { AXUIElementSetAttributeValue(el.0, cf(&key("AXValue")), cf(&NSString::from_str(value))) == 0 }
    }
}

#[cfg(target_os = "macos")]
fn run_osascript(script: &str) -> Result<String, DriverError> {
    let out = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .map_err(|e| DriverError::Step(e.to_string()))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        if err.contains("not allowed assistive access") || err.contains("-1719") {
            return Err(DriverError::NeedsAuth);
        }
        return Err(DriverError::Step(err.trim().to_string()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(not(target_os = "macos"))]
fn run_osascript(_script: &str) -> Result<String, DriverError> {
    Err(DriverError::Unsupported)
}

/// Deep link into System Settings › Privacy & Security › Accessibility.
pub const MACOS_ACCESSIBILITY_SETTINGS: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";

#[cfg(test)]
mod tests {
    use super::*;

    fn sel(s: &str) -> AxSelector {
        parse_selector(s).unwrap()
    }
    fn hit(s: &str, role: &str, classes: &[&str], labels: &[&str], first: Option<&str>) -> bool {
        let classes: Vec<String> = classes.iter().map(|c| c.to_string()).collect();
        let s = sel(s);
        s.kind_matches(role, &classes) && s.title_matches(labels, || first.map(str::to_string))
    }

    #[test]
    fn parses_ax_selectors() {
        assert_eq!(sel("AXButton[Start voice chat]"), AxSelector { role: "AXButton".into(), class: None, title: Title::Exact("Start voice chat".into()) });
        assert_eq!(sel("AXTextArea"), AxSelector { role: "AXTextArea".into(), class: None, title: Title::Any });
        assert_eq!(
            sel("AXButton.sand-agent-item[@Analytics]"),
            AxSelector { role: "AXButton".into(), class: Some("sand-agent-item".into()), title: Title::FirstText("Analytics".into()) }
        );
        assert_eq!(sel("AXLink[~ab]").title, Title::Contains("ab".into()));
        assert_eq!(sel("AXLink[~]").title, Title::Exact("~".into()), "a bare ~ is literal");
        assert_eq!(parse_selector("button[aria-label]"), None, "CSS selectors are not AX selectors");
    }

    #[test]
    fn selectors_match_role_class_and_label() {
        assert!(hit("AXButton[发起语音聊天]", "AXButton", &[], &["", "发起语音聊天"], None), "aria-label arrives as the description");
        assert!(!hit("AXButton[发起语音聊天]", "AXButton", &[], &["开始语音输入"], None));
        assert!(!hit("AXButton[发起语音聊天]", "AXCheckBox", &[], &["发起语音聊天"], None));
        assert!(hit("AXTextArea", "AXTextArea", &[], &[], None));
        assert!(hit("AXLink[~Analytics]", "AXLink", &[], &["Analytics 3 条未读"], None));
        assert!(!hit("AXLink[~Analytics]", "AXLink", &[], &["Code"], None));
        // a sidebar row: name + preview in the label, the name alone as its first text
        let row = "AXButton.sand-agent-item[@Analytics]";
        assert!(hit(row, "AXButton", &["sand-agent-item"], &["Analytics 10:13 这一小时很平稳"], Some("Analytics")));
        assert!(!hit(row, "AXButton", &["sand-agent-item"], &["Radar 提到了 Analytics"], Some("Radar")), "a preview mentioning the name is not the row");
        assert!(!hit(row, "AXButton", &["other"], &[], Some("Analytics")), "class must match");
    }

    #[test]
    fn grok_bot_driver_covers_the_chinese_ui() {
        let reg = super::super::Registry::bundled();
        let spec = reg.get("grok-bot").unwrap();
        let all = |steps: &[Step]| steps.iter().flat_map(|s| match s { Step::Click { selectors, .. } => selectors.clone(), _ => vec![] }).collect::<Vec<_>>();
        assert!(all(&spec.call).iter().any(|s| s == "AXButton[发起语音聊天]"));
        assert!(all(&spec.call).iter().any(|s| s == "AXButton.sand-agent-item[@{target}]"), "a Bot is picked by its sidebar row");
        // the sidebar only renders rows near its scroll position: a Bot off screen opens by link
        let Some(Step::Click { open: Some(link), .. }) = spec.call.first() else { panic!("the Bot step has a link to fall back on") };
        assert_eq!(link.url, "grokbot://app/v1/agent?id={target_id}");
        assert!(link.until.iter().all(|s| parse_selector(s).is_some()) && link.until.iter().any(|s| s.contains("{target}")));
        assert_eq!(spec.roster.as_ref().and_then(|r| r.id.as_deref()), Some("id"));
        assert!(all(&spec.hangup).iter().any(|s| s == "AXButton[挂断]"));
        assert!(spec.probe.in_call.iter().any(|s| s == "AXButton[挂断]"));
        assert!(spec.targets.iter().all(|s| parse_selector(s).is_some()) && !spec.targets.is_empty());
    }

    #[test]
    fn base32_keys_decode() {
        // a real file name from Grok Bot's store
        assert_eq!(base32_decode("onqw4zbomnwgszlooqxhg3djmnss45ljfvwgc6lpov2a").unwrap(), b"sand.client.slice.ui-layout");
        assert_eq!(base32_decode("MZXW6==="), Some(b"foo".to_vec()));
        assert_eq!(base32_decode("not base32!"), None);
    }

    #[test]
    fn roster_lists_callable_bots_and_explains_the_rest() {
        let spec = super::super::Registry::bundled().get("grok-bot").unwrap().roster.clone().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let encode = |key: &str| {
            const A: &[u8] = b"abcdefghijklmnopqrstuvwxyz234567";
            let (mut acc, mut bits, mut out) = (0u32, 0u32, String::new());
            for b in key.bytes() {
                acc = (acc << 8) | b as u32;
                bits += 8;
                while bits >= 5 {
                    bits -= 5;
                    out.push(A[((acc >> bits) & 31) as usize] as char);
                }
            }
            if bits > 0 {
                out.push(A[((acc << (5 - bits)) & 31) as usize] as char);
            }
            out
        };
        let rows = serde_json::json!({ "schemaVersion": 4, "value": { "rows": [
            { "id": "c059", "name": "Analytics", "isGroup": false, "isHiddenFromSidebar": false },
            { "name": "图片创作", "isGroup": true },
            { "name": "Old", "isGroup": false, "isHiddenFromSidebar": true },
            { "name": "可乐" }
        ] } });
        std::fs::write(dir.path().join(format!("{}.blob", encode("sand.client.slice.account.u1.roster.last-roster"))), rows.to_string()).unwrap();
        std::fs::write(dir.path().join(format!("{}.blob", encode("sand.client.slice.ui-layout"))), "{}").unwrap();
        let entries = read_roster(&spec, dir.path()).unwrap();
        let callable: Vec<&str> = entries.iter().filter(|e| e.blocked.is_none()).map(|e| e.name.as_str()).collect();
        assert_eq!(callable, ["Analytics", "可乐"], "group chats and hidden Bots are left out");
        assert_eq!(entries[0].id.as_deref(), Some("c059"), "ids come along for deep links");
        assert_eq!(entries[3].id, None);
        assert_eq!(entries.iter().find(|e| e.name == "图片创作").unwrap().blocked.as_deref(), Some("group_chat"));
        assert!(read_roster(&spec, &dir.path().join("missing")).is_none());
    }

    #[test]
    fn chatgpt_app_driver_places_dot_calls() {
        let reg = super::super::Registry::bundled();
        let spec = reg.get("chatgpt-app").unwrap();
        assert_eq!(spec.bundle_id.as_deref(), Some("com.openai.codex"));
        let all = |steps: &[Step]| steps.iter().flat_map(|s| match s { Step::Click { selectors, .. } => selectors.clone(), _ => vec![] }).collect::<Vec<_>>();
        assert!(all(&spec.call).iter().any(|s| s == "AXButton[发起通话]"));
        assert!(all(&spec.hangup).iter().any(|s| s == "AXButton[挂断]"));
        for s in all(&spec.call).iter().chain(all(&spec.hangup).iter()).chain(spec.probe.in_call.iter()) {
            assert!(parse_selector(s).is_some(), "{s}");
        }
        assert!(super::super::uses_target(&spec.call));
    }
}
