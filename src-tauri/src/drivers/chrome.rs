//! Web seats in the user's own Chrome, so its logins, cookies and passkeys come along and nothing
//! has to be signed into twice. Chrome 144+ opens a DevTools endpoint once the user turns on
//! chrome://inspect/#remote-debugging, announces it in `DevToolsActivePort`, and asks the user to
//! allow every new connection — so one connection is opened on first use and then kept. Each seat
//! gets its own background tab; the user's own tabs are never touched.

use super::DriverError;
use crate::i18n;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tungstenite::{Message, WebSocket};

/// The user has to find Chrome's dialog and click 允许 before the first reply arrives.
const APPROVAL_WAIT: Duration = Duration::from_secs(120);
/// The longest driver script (waits plus clicks) with slack.
const COMMAND_WAIT: Duration = Duration::from_secs(40);
/// Every connection attempt is a dialog in Chrome: after a refusal, don't ask again right away.
const RETRY_AFTER: Duration = Duration::from_secs(30);

/// `DevToolsActivePort` holds the port on the first line and the browser endpoint path on the second.
pub fn parse_port_file(text: &str) -> Option<(u16, String)> {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let port = lines.next()?.parse().ok()?;
    let path = lines.next()?;
    path.starts_with("/devtools/browser/").then(|| (port, path.to_string()))
}

/// `AGENTDOCK_CHROME_PORT_FILE` points at another profile's file (Chrome Beta, a test instance).
fn port_file() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("AGENTDOCK_CHROME_PORT_FILE") {
        return Some(p.into());
    }
    #[cfg(target_os = "macos")]
    return dirs::data_dir().map(|d| d.join("Google/Chrome/DevToolsActivePort"));
    #[cfg(target_os = "windows")]
    return dirs::data_local_dir().map(|d| d.join("Google\\Chrome\\User Data\\DevToolsActivePort"));
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    return dirs::config_dir().map(|d| d.join("google-chrome/DevToolsActivePort"));
}

fn endpoint() -> Option<(u16, String)> {
    parse_port_file(&std::fs::read_to_string(port_file()?).ok()?)
}

fn chrome_running() -> bool {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::NSRunningApplication;
        use objc2_foundation::NSString;
        return NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str("com.google.Chrome")).iter().next().is_some();
    }
    #[allow(unreachable_code)]
    true
}

/// Chrome is running with remote debugging on. Whether the user allows us is only known on connect;
/// this never touches the port, since every connection attempt pops a dialog in Chrome.
pub fn available() -> bool {
    chrome_running() && endpoint().is_some()
}

enum Fail {
    /// The socket is gone; the connection and every tab session with it are dropped.
    Lost,
    /// Chrome answered with a protocol error.
    Cdp(String),
    Driver(DriverError),
}

impl From<Fail> for DriverError {
    fn from(f: Fail) -> Self {
        match f {
            Fail::Lost => DriverError::Chrome(i18n::t("chrome.lost")),
            Fail::Cdp(m) => DriverError::Step(m),
            Fail::Driver(e) => e,
        }
    }
}

fn no_tab() -> Fail {
    Fail::Driver(DriverError::Chrome(i18n::t("chrome.no_tab")))
}

struct Conn {
    ws: WebSocket<TcpStream>,
    next_id: u64,
}

impl Conn {
    fn open() -> Result<Conn, DriverError> {
        let refused = || DriverError::Chrome(i18n::t("chrome.refused"));
        let (port, path) = endpoint().ok_or_else(|| DriverError::Chrome(i18n::t("chrome.not_enabled")))?;
        let stream = TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), Duration::from_secs(2)).map_err(|_| refused())?;
        let _ = stream.set_read_timeout(Some(APPROVAL_WAIT));
        let (ws, _) = tungstenite::client(format!("ws://127.0.0.1:{port}{path}"), stream).map_err(|_| refused())?;
        let mut conn = Conn { ws, next_id: 1 };
        conn.call(None, "Browser.getVersion", json!({})).map_err(|_| refused())?;
        let _ = conn.ws.get_ref().set_read_timeout(Some(COMMAND_WAIT));
        tracing::info!("connected to Chrome DevTools on port {port}");
        Ok(conn)
    }

    fn call(&mut self, session: Option<&str>, method: &str, params: Value) -> Result<Value, Fail> {
        let id = self.next_id;
        self.next_id += 1;
        let mut msg = json!({ "id": id, "method": method, "params": params });
        if let Some(s) = session {
            msg["sessionId"] = json!(s);
        }
        self.ws.send(Message::text(msg.to_string())).map_err(|_| Fail::Lost)?;
        loop {
            match self.ws.read().map_err(|_| Fail::Lost)? {
                Message::Text(text) => {
                    let Ok(v) = serde_json::from_str::<Value>(text.as_str()) else { continue };
                    if v["id"].as_u64() != Some(id) {
                        continue;
                    }
                    if let Some(e) = v.get("error") {
                        return Err(Fail::Cdp(e["message"].as_str().unwrap_or("CDP error").to_string()));
                    }
                    return Ok(v["result"].clone());
                }
                Message::Close(_) => return Err(Fail::Lost),
                _ => {}
            }
        }
    }
}

#[derive(Default)]
struct Inner {
    conn: Option<Conn>,
    /// seat id → (target id, flat session id) of the tab AgentDock opened for it.
    tabs: HashMap<String, (String, String)>,
    failed_at: Option<Instant>,
}

impl Inner {
    fn call(&mut self, open: bool, session: Option<&str>, method: &str, params: Value) -> Result<Value, Fail> {
        if self.conn.is_none() {
            if !open {
                return Err(no_tab());
            }
            if self.failed_at.is_some_and(|t| t.elapsed() < RETRY_AFTER) {
                return Err(Fail::Driver(DriverError::Chrome(i18n::t("chrome.just_refused"))));
            }
            match Conn::open() {
                Ok(c) => {
                    self.conn = Some(c);
                    self.failed_at = None;
                }
                Err(e) => {
                    self.failed_at = Some(Instant::now());
                    return Err(Fail::Driver(e));
                }
            }
        }
        let r = self.conn.as_mut().map(|c| c.call(session, method, params)).unwrap_or(Err(Fail::Lost));
        if matches!(r, Err(Fail::Lost)) {
            self.conn = None;
            self.tabs.clear();
        }
        r
    }

    fn session(&mut self, seat: &str, url: &str, open: bool) -> Result<String, Fail> {
        if let Some((_, s)) = self.tabs.get(seat) {
            return Ok(s.clone());
        }
        if !open {
            return Err(no_tab());
        }
        let created = self.call(true, None, "Target.createTarget", json!({ "url": url, "background": true }))?;
        let target = created["targetId"].as_str().ok_or_else(|| Fail::Cdp("createTarget returned no targetId".into()))?.to_string();
        let attached = self.call(true, None, "Target.attachToTarget", json!({ "targetId": target, "flatten": true }))?;
        let session = attached["sessionId"].as_str().ok_or_else(|| Fail::Cdp("attachToTarget returned no sessionId".into()))?.to_string();
        // a background tab is hidden and unfocused: keep the page acting as if it were in front
        let _ = self.call(true, Some(&session), "Emulation.setFocusEmulationEnabled", json!({ "enabled": true }));
        let _ = self.call(true, Some(&session), "Page.addScriptToEvaluateOnNewDocument", json!({ "source": super::web::BRIDGE_SCRIPT }));
        let _ = self.call(true, Some(&session), "Runtime.evaluate", json!({ "expression": super::web::BRIDGE_SCRIPT }));
        self.tabs.insert(seat.to_string(), (target, session.clone()));
        Ok(session)
    }

    /// Drop a seat's tab whose target or session went away (closed by the user, crashed).
    fn forget(&mut self, seat: &str) {
        if let Some((target, _)) = self.tabs.remove(seat) {
            let _ = self.call(false, None, "Target.closeTarget", json!({ "targetId": target }));
        }
    }
}

fn gone(m: &str) -> bool {
    m.contains("given id")
}

#[derive(Default)]
pub struct Chrome {
    inner: Mutex<Inner>,
}

impl Chrome {
    /// A busy lock means a command is in flight, which only happens while connected.
    pub fn is_connected(&self) -> bool {
        self.inner.try_lock().map(|i| i.conn.is_some()).unwrap_or(true)
    }

    /// Whether the seat has a live tab right now; `false` while another command holds the line,
    /// so periodic probes skip a beat instead of queueing behind a call.
    pub fn has_tab(&self, seat: &str) -> bool {
        self.inner.try_lock().map(|i| i.tabs.contains_key(seat)).unwrap_or(false)
    }

    /// Run `expr` in the seat's tab and return its value. With `open`, connect to Chrome and open
    /// the tab first if needed; without it, only an existing tab is used.
    pub fn eval(&self, seat: &str, url: &str, expr: &str, open: bool) -> Result<Value, DriverError> {
        let mut inner = self.inner.lock().unwrap();
        for attempt in 0..2 {
            let session = inner.session(seat, url, open)?;
            let params = json!({ "expression": expr, "awaitPromise": true, "returnByValue": true, "userGesture": true });
            match inner.call(open, Some(&session), "Runtime.evaluate", params) {
                Ok(r) => {
                    if let Some(ex) = r.get("exceptionDetails") {
                        let msg = ex["exception"]["description"].as_str().or(ex["text"].as_str()).unwrap_or("script error").to_string();
                        return Err(if msg.contains("not found:") || msg.contains("timeout:") { DriverError::NotFound(msg) } else { DriverError::Step(msg) });
                    }
                    return Ok(r["result"]["value"].clone());
                }
                Err(Fail::Cdp(m)) if gone(&m) && attempt == 0 => {
                    inner.forget(seat);
                    if !open {
                        return Err(no_tab().into());
                    }
                }
                Err(e) => return Err(e.into()),
            }
        }
        Err(no_tab().into())
    }

    /// Bring the seat's tab (opened if needed) to the front of Chrome, and Chrome to the front.
    pub fn show(&self, seat: &str, url: &str) -> Result<(), DriverError> {
        self.eval(seat, url, "1", true)?;
        let mut inner = self.inner.lock().unwrap();
        let target = inner.tabs.get(seat).map(|t| t.0.clone()).ok_or_else(|| DriverError::from(no_tab()))?;
        inner.call(false, None, "Target.activateTarget", json!({ "targetId": target }))?;
        #[cfg(target_os = "macos")]
        let _ = std::process::Command::new("open").args(["-b", "com.google.Chrome"]).status();
        Ok(())
    }

    /// Close the tab AgentDock opened for a seat. Chrome's own logins are left alone.
    pub fn close(&self, seat: &str) {
        if let Ok(mut inner) = self.inner.try_lock() {
            inner.forget(seat);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_file_gives_port_and_browser_path() {
        assert_eq!(
            parse_port_file("9222\n/devtools/browser/58ec802d-d98a-4af8-8e14-482bf27db839\n"),
            Some((9222, "/devtools/browser/58ec802d-d98a-4af8-8e14-482bf27db839".into()))
        );
        assert_eq!(parse_port_file("9222\n"), None);
        assert_eq!(parse_port_file("abc\n/devtools/browser/x"), None);
        assert_eq!(parse_port_file("9222\n/devtools/page/x"), None);
    }

    #[test]
    fn closed_tabs_are_recognised_by_their_cdp_error() {
        assert!(gone("Session with given id not found."));
        assert!(gone("No target with given id found"));
        assert!(!gone("Execution context was destroyed."));
    }

    /// Start Chrome with `--remote-debugging-port=0 --user-data-dir=<dir>` and point
    /// `AGENTDOCK_CHROME_PORT_FILE` at `<dir>/DevToolsActivePort`.
    #[test]
    #[ignore = "needs a Chrome instance with remote debugging"]
    fn drives_a_seat_tab_in_chrome() {
        use super::super::{steps_script, Step};
        let chrome = Chrome::default();
        let page = "data:text/html,<button aria-label='Start voice chat' onclick=\"document.title='called'\">call</button>";
        let click = |sel: &str| steps_script(&[Step::Click { selectors: vec![sel.to_string()], open: None }]);
        chrome.eval("t", page, &click("button[aria-label='Start voice chat']"), true).unwrap();
        assert!(chrome.has_tab("t"));
        assert_eq!(chrome.eval("t", page, "document.title", false).unwrap(), json!("called"));
        assert_eq!(chrome.eval("t", page, "document.hidden", false).unwrap(), json!(false), "bridge keeps the background tab visible to the page");
        assert!(matches!(chrome.eval("t", page, &click("button.missing"), false), Err(DriverError::NotFound(_))));
        chrome.close("t");
        assert!(!chrome.has_tab("t"));
        assert!(chrome.eval("t", page, "1", false).is_err(), "without `open` a closed tab is not reopened");
    }
}
