//! Drivers reach an agent's voice entry. A driver is *data*: a `DriverSpec` (URL, login detector,
//! CSS selectors, step scripts) loaded from `drivers/*.json`, interpreted by a small runtime.
//! Web seats run inside hidden, persistent webviews; app seats go through Accessibility / UIA.

pub mod app;
pub mod chrome;
pub mod web;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "op")]
pub enum Step {
    /// Click the first visible element matching the selector (tries each selector in order).
    Click {
        selectors: Vec<String>,
        /// App seats: a deep link to the same place, for when nothing matches.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        open: Option<OpenLink>,
        /// App seats: `until` shows the press took effect (a call's controls after its call
        /// button). When it does not show up the app is throttled in the background, where it can
        /// take a minute to handle a press, and `url` brings it forward to wake it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wake: Option<OpenLink>,
    },
    /// Wait for a selector to appear (ms budget).
    WaitFor {
        selectors: Vec<String>,
        timeout_ms: u64,
    },
    Sleep {
        ms: u64,
    },
    /// Navigate the hidden webview.
    Goto {
        url: String,
    },
    /// Press a key on the focused element (web) or the app (native).
    Key {
        key: String,
    },
    /// Clear a text field so an empty-composer-only button appears.
    Clear {
        selectors: Vec<String>,
    },
}

/// A deep link that lands where a click would have, for an element the app left out of its
/// accessibility tree: Grok Bot's sidebar only renders the Bot rows near its scroll position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenLink {
    /// `{target_id}` is the target's id in the app's roster, `{target}` its name (both URL-encoded).
    pub url: String,
    /// Present once the link has landed; already present means the click is not needed at all.
    #[serde(default)]
    pub until: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeSpec {
    /// Any of these present ⇒ in a call.
    pub in_call: Vec<String>,
    /// Any of these present ⇒ seat-level mute is on.
    #[serde(default)]
    pub muted: Vec<String>,
    /// Any of these present ⇒ user must look (captcha, unknown dialog).
    #[serde(default)]
    pub needs_human: Vec<String>,
    /// Any of these present ⇒ logged in.
    pub logged_in: Vec<String>,
    /// URL substrings that mean "this is the login flow".
    #[serde(default)]
    pub login_urls: Vec<String>,
    /// Selector whose text / src gives the account name or avatar.
    #[serde(default)]
    pub account: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriverSpec {
    pub id: String,
    pub display_name: String,
    pub version: u32,
    /// Whether the selectors were pinned against the live product on a real machine.
    #[serde(default)]
    pub verified: bool,
    /// A placeholder: the contact and its icon exist, calling is not wired up yet. Seats on it
    /// stay "pending", are never probed, and a call explains instead of failing.
    #[serde(default)]
    pub pending: bool,
    pub url: Option<String>,
    pub bundle_id: Option<String>,
    pub user_agent: Option<String>,
    pub call: Vec<Step>,
    pub hangup: Vec<Step>,
    #[serde(default)]
    pub mute: Vec<Step>,
    #[serde(default)]
    pub unmute: Vec<Step>,
    pub probe: ProbeSpec,
    /// Optional selectors for EverOS context injection and caption scraping.
    /// Empty lists fall back to generic composer / `[data-message-author-role]` selectors.
    #[serde(default)]
    pub memory: MemorySelectors,
    /// App seats: elements listing what can be called (e.g. Bot rows); their first text is a
    /// seat `target`. Lets the settings window offer those names instead of having them typed.
    #[serde(default)]
    pub targets: Vec<String>,
    /// What a target is called in the settings window, e.g. "Bot".
    #[serde(default)]
    pub target_noun: Option<String>,
    /// App seats: the app's own cached list of what can be called. Preferred over reading the
    /// sidebar: complete (collapsed sections too), works without the app open, and says which
    /// entries cannot take a call (group chats).
    #[serde(default)]
    pub roster: Option<RosterSpec>,
    /// App seats: arguments the app is started with when AgentDock starts it, and restarted with
    /// (idle, in the background) when it runs without them. Grok Bot gets Chromium's switches
    /// against background throttling: throttled, it handles a press up to a minute late.
    #[serde(default)]
    pub launch_args: Vec<String>,
}

/// A JSON roster in a key-value store directory whose file names are base32 keys
/// (`<base32(key)>.blob`). The newest file whose key ends with `key_suffix` wins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RosterSpec {
    /// `~` is the home directory.
    pub dir: String,
    pub key_suffix: String,
    /// JSON pointer to the array of entries.
    pub rows: String,
    /// Field holding an entry's name (what `{target}` gets).
    pub name: String,
    /// Field holding an entry's id (what `{target_id}` gets in a step's `open` link).
    #[serde(default)]
    pub id: Option<String>,
    /// Boolean fields that make an entry uncallable, with the reason shown to the user: a key
    /// of the native string table under `roster.` ("group_chat"), or plain text.
    #[serde(default)]
    pub exclude: std::collections::BTreeMap<String, String>,
}

/// Whether any step selector mentions `{target}`.
pub fn uses_target(steps: &[Step]) -> bool {
    steps.iter().any(|s| match s {
        Step::Click { selectors, .. } | Step::Clear { selectors } | Step::WaitFor { selectors, .. } => selectors.iter().any(|x| x.contains("{target}")),
        _ => false,
    })
}

/// Where a web seat keeps its composer and its visible transcript. Both lists are tried in order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct MemorySelectors {
    #[serde(default)]
    pub composer: Vec<String>,
    #[serde(default)]
    pub transcript: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeResult {
    pub in_call: bool,
    pub muted: bool,
    pub needs_human: bool,
    pub logged_in: bool,
    pub account: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum DriverError {
    #[error("no driver named {0}")]
    Unknown(String),
    #[error("driver step failed: {0}")]
    Step(String),
    #[error("control not found: {0}")]
    NotFound(String),
    /// Talking to the user's Chrome failed; the message is already user-facing.
    #[error("{0}")]
    Chrome(String),
    /// The target cannot take a call (e.g. a group chat); the message is user-facing.
    #[error("{0}")]
    NotCallable(String),
    #[error("seat needs login or permission")]
    NeedsAuth,
    #[error("platform not supported for this driver")]
    Unsupported,
}

/// Bundled driver specs, compiled in so the app works offline; can be overridden by files in
/// `<config>/drivers/*.json` (hot update without shipping a new binary).
pub const BUNDLED: &[(&str, &str)] = &[
    (
        "chatgpt-dot",
        include_str!("../../drivers/chatgpt-dot.json"),
    ),
    ("chatgpt-app", include_str!("../../drivers/chatgpt-app.json")),
    ("gemini-app", include_str!("../../drivers/gemini-app.json")),
    ("claude-app", include_str!("../../drivers/claude-app.json")),
    ("antigravity", include_str!("../../drivers/antigravity.json")),
    ("cursor", include_str!("../../drivers/cursor.json")),
    (
        "copilot-voice",
        include_str!("../../drivers/copilot-voice.json"),
    ),
    (
        "devin-voice",
        include_str!("../../drivers/devin-voice.json"),
    ),
    ("muse-voice", include_str!("../../drivers/muse-voice.json")),
    ("grok-bot", include_str!("../../drivers/grok-bot.json")),
];

#[derive(Debug, Default)]
pub struct Registry {
    specs: HashMap<String, DriverSpec>,
}

impl Registry {
    pub fn bundled() -> Self {
        let mut r = Registry::default();
        for (id, text) in BUNDLED {
            let spec: DriverSpec = serde_json::from_str(text)
                .unwrap_or_else(|e| panic!("bundled driver {id} invalid: {e}"));
            debug_assert_eq!(&spec.id, id);
            r.specs.insert(spec.id.clone(), spec);
        }
        r
    }

    /// Layer user-provided overrides (hot updates) on top of the bundled set.
    pub fn with_overrides(mut self, dir: &std::path::Path) -> Self {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                match std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|t| serde_json::from_str::<DriverSpec>(&t).ok())
                {
                    Some(spec) => {
                        let newer = self
                            .specs
                            .get(&spec.id)
                            .map(|cur| spec.version > cur.version)
                            .unwrap_or(true);
                        if newer {
                            self.specs.insert(spec.id.clone(), spec);
                        }
                    }
                    None => tracing::warn!("ignoring invalid driver override {}", path.display()),
                }
            }
        }
        self
    }

    pub fn get(&self, id: &str) -> Result<&DriverSpec, DriverError> {
        self.specs
            .get(id)
            .ok_or_else(|| DriverError::Unknown(id.into()))
    }

    pub fn ids(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.specs.keys().map(String::as_str).collect();
        v.sort();
        v
    }
}

/// JS that evaluates a probe inside the page and reports back through the Tauri IPC bridge.
/// Kept here (not in web.rs) so it can be unit-tested as a pure string builder.
pub fn probe_script(spec: &ProbeSpec, seat_id: &str) -> String {
    let j = |v: &Vec<String>| serde_json::to_string(v).unwrap();
    format!(
        r#"(() => {{
  const vis = (sel) => Array.from(document.querySelectorAll(sel)).some(el => {{
    const r = el.getBoundingClientRect(); const s = getComputedStyle(el);
    return r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && s.display !== 'none';
  }});
  const any = (sels) => sels.some(s => {{ try {{ return vis(s); }} catch (_) {{ return false; }} }});
  const acct = (() => {{ const sel = {account}; if (!sel) return null;
    const el = document.querySelector(sel); if (!el) return null;
    return el.getAttribute('alt') || el.getAttribute('aria-label') || el.textContent?.trim() || el.getAttribute('src') || null; }})();
  const loginUrls = {login_urls};
  const onLogin = loginUrls.some(u => location.href.includes(u));
  const result = {{
    in_call: any({in_call}), muted: any({muted}), needs_human: any({needs_human}),
    logged_in: !onLogin && any({logged_in}), account: acct, url: location.href
  }};
  window.__TAURI_INTERNALS__?.invoke('driver_report', {{ seatId: {seat}, result }});
  return result;
}})()"#,
        account = serde_json::to_string(&spec.account).unwrap(),
        login_urls = j(&spec.login_urls),
        in_call = j(&spec.in_call),
        muted = j(&spec.muted),
        needs_human = j(&spec.needs_human),
        logged_in = j(&spec.logged_in),
        seat = serde_json::to_string(seat_id).unwrap(),
    )
}

const DEFAULT_COMPOSER: &[&str] = &[
    "#prompt-textarea",
    "textarea[placeholder]",
    "textarea",
    "div[contenteditable='true']",
    "[contenteditable='true']",
];
const DEFAULT_TRANSCRIPT: &[&str] = &[
    "[data-message-author-role]",
    "[data-testid='conversation-turn']",
    "article[data-turn]",
];

/// Paste a recalled brief into the composer. Does not press Send — a mis-click would start a
/// separate text turn beside the voice call. Re-running is a no-op once the marker is present.
pub fn inject_memory_script(text: &str, composer: &[String]) -> String {
    let text = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into());
    let defaults = DEFAULT_COMPOSER
        .iter()
        .map(|s| (*s).to_string())
        .collect::<Vec<_>>();
    let sels_ref: &[String] = if composer.is_empty() {
        &defaults
    } else {
        composer
    };
    let sels = serde_json::to_string(sels_ref).unwrap();
    format!(
        r#"(async () => {{
  const text = {text};
  const marker = "[AgentDock memory]";
  if (!text || !text.includes(marker)) return "empty";
  const sels = {sels};
  const find = (list) => {{ for (const s of list) {{ try {{
    const els = Array.from(document.querySelectorAll(s));
    const el = els.find(e => {{ const r = e.getBoundingClientRect(); return r.width > 0 && r.height > 0; }});
    if (el) return el; }} catch (_) {{}} }} return null; }};
  const el = find(sels);
  if (!el) return "no-composer";
  const cur = ("value" in el ? el.value : (el.innerText || "")) || "";
  if (String(cur).includes(marker)) return "already";
  try {{
    if ("value" in el) {{
      const proto = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, "value");
      const next = text + (cur ? "\n" + cur : "");
      if (proto && proto.set) proto.set.call(el, next); else el.value = next;
      el.dispatchEvent(new Event("input", {{ bubbles: true }}));
    }} else {{
      el.focus();
      const sel = window.getSelection();
      const range = document.createRange();
      range.selectNodeContents(el);
      range.collapse(true);
      sel.removeAllRanges();
      sel.addRange(range);
      document.execCommand("insertText", false, text + "\n");
    }}
  }} catch (e) {{ return "inject-failed"; }}
  return "ok";
}})()"#
    )
}

/// Scrape visible user/assistant bubbles and report the ones this page has not reported yet.
/// Role comes from `data-message-author-role` / `data-role`, then a class or aria hint.
/// Nodes with no role are skipped so chrome text does not become memory.
/// The new lines are also the script's value, for tabs in the user's Chrome (no Tauri IPC there).
pub fn transcript_script(selectors: &[String], seat_id: &str) -> String {
    let defaults = DEFAULT_TRANSCRIPT
        .iter()
        .map(|s| (*s).to_string())
        .collect::<Vec<_>>();
    let sels_ref: &[String] = if selectors.is_empty() {
        &defaults
    } else {
        selectors
    };
    let sels = serde_json::to_string(sels_ref).unwrap();
    let seat = serde_json::to_string(seat_id).unwrap();
    format!(
        r#"(async () => {{
  const sels = {sels};
  const seen = (window.__agentdockMemorySeen = window.__agentdockMemorySeen || new Set());
  const nodes = [];
  for (const s of sels) {{ try {{ document.querySelectorAll(s).forEach(el => nodes.push(el)); }} catch (_) {{}} }}
  const lines = [];
  for (const el of nodes) {{
    let role = (el.getAttribute("data-message-author-role") || el.getAttribute("data-role") || "").toLowerCase();
    if (role !== "user" && role !== "assistant") {{
      const blob = ((el.className && String(el.className)) || "") + " " + (el.getAttribute("aria-label") || "");
      if (/\buser\b/i.test(blob)) role = "user";
      else if (/assistant|\bagent\b|\bbot\b/i.test(blob)) role = "assistant";
      else continue;
    }}
    const content = (el.innerText || "").trim().slice(0, 4000);
    if (!content) continue;
    const key = role + "\n" + content;
    if (seen.has(key)) continue;
    seen.add(key);
    lines.push({{ role, content }});
  }}
  const fresh = lines.slice(-12);
  if (!fresh.length) return [];
  window.__TAURI_INTERNALS__?.invoke("memory_report", {{ seatId: {seat}, lines: fresh }});
  return fresh;
}})()"#
    )
}

/// Fill a seat's `target` (e.g. which Bot inside Grok Bot) into `{target}` in step selectors, and
/// its roster id into `{target_id}` in `open` links (a link missing a value is left out).
/// Without a target, steps that mention `{target}` are dropped, along with a `sleep` right after.
pub fn bind_target(steps: &[Step], target: Option<&str>, target_id: Option<&str>) -> Vec<Step> {
    let target = target.map(str::trim).filter(|t| !t.is_empty());
    let mut out = Vec::with_capacity(steps.len());
    let mut dropped = false;
    for step in steps {
        let bind = |sels: &Vec<String>| -> Option<Option<Vec<String>>> {
            if !sels.iter().any(|s| s.contains("{target}")) {
                return Some(None);
            }
            target.map(|t| Some(sels.iter().map(|s| s.replace("{target}", t)).collect()))
        };
        let bound = match step {
            Step::Click { selectors, open, wake } => bind(selectors).map(|b| Step::Click {
                selectors: b.unwrap_or_else(|| selectors.clone()),
                open: open.as_ref().and_then(|o| bind_link(o, target, target_id)),
                wake: wake.as_ref().and_then(|w| bind_link(w, target, target_id)),
            }),
            Step::Clear { selectors } => bind(selectors).map(|b| b.map_or_else(|| step.clone(), |selectors| Step::Clear { selectors })),
            Step::WaitFor { selectors, timeout_ms } => bind(selectors).map(|b| b.map_or_else(|| step.clone(), |selectors| Step::WaitFor { selectors, timeout_ms: *timeout_ms })),
            Step::Sleep { .. } if dropped => None,
            _ => Some(step.clone()),
        };
        dropped = bound.is_none();
        out.extend(bound);
    }
    out
}

fn bind_link(link: &OpenLink, target: Option<&str>, target_id: Option<&str>) -> Option<OpenLink> {
    let mut url = link.url.clone();
    for (key, value) in [("{target_id}", target_id), ("{target}", target)] {
        if url.contains(key) {
            url = url.replace(key, &url_encode(value?));
        }
    }
    let until = link.until.iter().map(|s| target.map_or_else(|| s.clone(), |t| s.replace("{target}", t))).collect();
    Some(OpenLink { url, until })
}

/// Percent-encode everything but RFC 3986 unreserved characters.
fn url_encode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { char::from(b).to_string() } else { format!("%{b:02X}") }).collect()
}

/// JS that runs a step list sequentially inside the page.
pub fn steps_script(steps: &[Step]) -> String {
    let steps_json = serde_json::to_string(steps).unwrap();
    format!(
        r#"(async () => {{
  const steps = {steps_json};
  const sleep = (ms) => new Promise(r => setTimeout(r, ms));
  const find = (sels) => {{ for (const s of sels) {{ try {{
      const els = Array.from(document.querySelectorAll(s));
      const el = els.find(e => {{ const r = e.getBoundingClientRect(); return r.width > 0 && r.height > 0; }});
      if (el) return el; }} catch (_) {{}} }} return null; }};
  const waitFor = async (sels, ms) => {{ const t0 = Date.now();
    while (Date.now() - t0 < ms) {{ const el = find(sels); if (el) return el; await sleep(100); }} return null; }};
  for (const st of steps) {{
    if (st.op === 'click') {{ const el = await waitFor(st.selectors, 4000); if (!el) throw new Error('not found: ' + st.selectors.join(' | ')); el.click(); }}
    else if (st.op === 'wait_for') {{ if (!(await waitFor(st.selectors, st.timeout_ms))) throw new Error('timeout: ' + st.selectors.join(' | ')); }}
    else if (st.op === 'sleep') {{ await sleep(st.ms); }}
    else if (st.op === 'goto') {{ location.href = st.url; await sleep(1500); }}
    else if (st.op === 'key') {{ const t = document.activeElement || document.body;
      t.dispatchEvent(new KeyboardEvent('keydown', {{ key: st.key, bubbles: true }})); t.dispatchEvent(new KeyboardEvent('keyup', {{ key: st.key, bubbles: true }})); }}
    else if (st.op === 'clear') {{ const el = find(st.selectors); if (el) {{ if ('value' in el) el.value = ''; else el.textContent = ''; el.dispatchEvent(new Event('input', {{ bubbles: true }})); }} }}
  }}
  return 'ok';
}})().catch(e => {{ window.__TAURI_INTERNALS__?.invoke('driver_error', {{ message: String(e) }}); throw e; }})"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_specs_parse_and_cover_every_default_seat() {
        let r = Registry::bundled();
        assert_eq!(
            r.ids(),
            [
                "antigravity",
                "chatgpt-app",
                "chatgpt-dot",
                "claude-app",
                "copilot-voice",
                "cursor",
                "devin-voice",
                "gemini-app",
                "grok-bot",
                "muse-voice"
            ]
        );
        for id in r.ids() {
            let s = r.get(id).unwrap();
            if s.pending {
                // placeholders: an app to point at, no call wiring yet
                assert!(s.bundle_id.is_some() || s.url.is_some(), "{id} points at an app or a site");
                continue;
            }
            assert!(!s.call.is_empty(), "{id} has a call sequence");
            assert!(!s.hangup.is_empty(), "{id} has a hangup sequence");
            assert!(
                !s.probe.in_call.is_empty(),
                "{id} can detect an active call"
            );
        }
    }

    #[test]
    fn overrides_only_win_when_newer() {
        let dir = tempfile::tempdir().unwrap();
        let mut spec = Registry::bundled().get("copilot-voice").unwrap().clone();
        spec.version += 1;
        spec.display_name = "Copilot (hotfix)".into();
        std::fs::write(
            dir.path().join("copilot.json"),
            serde_json::to_string(&spec).unwrap(),
        )
        .unwrap();
        let mut stale = Registry::bundled().get("chatgpt-dot").unwrap().clone();
        stale.version = 0;
        stale.display_name = "stale".into();
        std::fs::write(
            dir.path().join("dot.json"),
            serde_json::to_string(&stale).unwrap(),
        )
        .unwrap();
        let r = Registry::bundled().with_overrides(dir.path());
        assert_eq!(
            r.get("copilot-voice").unwrap().display_name,
            "Copilot (hotfix)"
        );
        assert_ne!(r.get("chatgpt-dot").unwrap().display_name, "stale");
    }

    #[test]
    fn scripts_embed_selectors_safely() {
        let reg = Registry::bundled();
        let spec = reg.get("chatgpt-dot").unwrap();
        let js = probe_script(&spec.probe, "dot\"); alert(1); (\"");
        assert!(js.contains(r#"invoke('driver_report'"#));
        assert!(
            js.contains(r#""dot\"); alert(1); (\"""#),
            "seat id is JSON-escaped"
        );
        let steps = steps_script(&spec.call);
        assert!(steps.contains("\"op\":\"click\""));
    }

    #[test]
    fn target_fills_selectors_or_drops_its_steps() {
        let steps = vec![
            Step::Click { selectors: vec!["AXLink[{target}]".into()], open: None, wake: None },
            Step::Sleep { ms: 1200 },
            Step::Clear { selectors: vec!["AXTextArea".into()] },
            Step::Click { selectors: vec!["AXButton[发起语音聊天]".into()], open: None, wake: None },
        ];
        let bound = bind_target(&steps, Some(" Analytics "), None);
        assert_eq!(bound[0], Step::Click { selectors: vec!["AXLink[Analytics]".into()], open: None, wake: None });
        assert_eq!(bound.len(), 4);
        let plain = bind_target(&steps, None, None);
        assert_eq!(plain, steps[2..].to_vec(), "no target: the select step and its pause are skipped");
        assert_eq!(bind_target(&steps, Some("  "), None), plain);
    }

    #[test]
    fn open_links_get_the_target_and_its_id_or_are_left_out() {
        let link = |url: &str| Some(OpenLink { url: url.into(), until: vec!["AXGroup.sand-chat[{target}]".into()] });
        let step = |url: &str| vec![Step::Click { selectors: vec!["AXButton[{target}]".into()], open: link(url), wake: link("grokbot://app/v1/open") }];
        let open_of = |steps: Vec<Step>| match steps.into_iter().next() {
            Some(Step::Click { open, .. }) => open,
            other => panic!("{other:?}"),
        };
        let bound = open_of(bind_target(&step("grokbot://app/v1/agent?id={target_id}"), Some("Content"), Some("ea70-0b4c")));
        assert_eq!(bound, Some(OpenLink { url: "grokbot://app/v1/agent?id=ea70-0b4c".into(), until: vec!["AXGroup.sand-chat[Content]".into()] }));
        let named = open_of(bind_target(&step("app://bot?name={target}"), Some("图片 创作&"), None)).unwrap();
        assert_eq!(named.url, "app://bot?name=%E5%9B%BE%E7%89%87%20%E5%88%9B%E4%BD%9C%26", "names are URL-encoded");
        assert_eq!(open_of(bind_target(&step("grokbot://app/v1/agent?id={target_id}"), Some("Content"), None)), None, "no id, no link: the click alone decides");
    }

    #[test]
    fn memory_scripts_json_escape_text_and_seat_ids() {
        let js = inject_memory_script("[AgentDock memory]\n他说 \"hi\" </script>", &[]);
        assert!(js.contains(r#""[AgentDock memory]\n他说 \"hi\" </script>""#));
        assert!(js.contains("insertText"));
        assert!(
            !js.contains("</script>\"),"),
            "the closing tag stays inside a JSON string"
        );
        let scrape = transcript_script(&[], "dot\"); alert(1)");
        assert!(scrape.contains(r#""dot\"); alert(1)""#));
        assert!(scrape.contains("memory_report"));
        let custom = transcript_script(&[".msg".into()], "dot");
        assert!(custom.contains(r#"[".msg"]"#));
        assert!(!custom.contains("conversation-turn"));
    }
}
