//! Mouse clicks anywhere on the screen, from Hyprland.
//!
//! An input method only hears about the pointer while it is over its own popup, and apps such as
//! terminals never report that a click moved their caret. So that a click elsewhere closes the
//! bar, the daemon adds mouse binds to Hyprland at run time that take nothing from the app under
//! the pointer (`non_consuming`) and only post a custom event on Hyprland's event socket. Nothing
//! is written to the user's config, and no process is started per click.

use crate::security::{
    extract_json_str, extract_json_u32, get_hyprland_event_socket_path, hyprland_request_within,
};
use std::io::Read;
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::net::UnixStream;
use std::time::Duration;

/// The binds post `custom>>typesuggest-click <button>`
const CLICK_EVENT: &str = "typesuggest-click";

/// Linux event codes of the left, right and middle buttons, as Hyprland names them (`mouse:272`)
const BUTTONS: [u32; 3] = [272, 273, 274];

/// Adds the binds, and keeps their handles in a Lua global so that only these binds are ever
/// switched off and on again: Hyprland's `unbind` and `remove` would also delete the user's own
/// binds on the same buttons. A later run reuses the handles, so registering twice adds nothing;
/// a config reload drops binds and globals together. `dont_inhibit` keeps the binds working
/// while an app inhibits shortcuts, `submap_universal` inside submaps. No description, so
/// keybinding cheat sheets leave them out.
const REGISTER_LUA: &str = "if __typesuggest_clicks then \
    for _, bind in ipairs(__typesuggest_clicks) do bind:set_enabled(true) end \
    else __typesuggest_clicks = {} \
    for _, b in ipairs({272, 273, 274}) do __typesuggest_clicks[#__typesuggest_clicks + 1] = \
    hl.bind(\"mouse:\" .. b, hl.dsp.event(\"typesuggest-click \" .. b), \
    { non_consuming = true, dont_inhibit = true, submap_universal = true }) end end";

/// Switches them off again. Every other bind on the same buttons (SUPER + drag, the user's own)
/// is left alone.
const UNREGISTER_LUA: &str = "if __typesuggest_clicks then \
    for _, bind in ipairs(__typesuggest_clicks) do bind:set_enabled(false) end end";

/// Long enough for Hyprland to run a short snippet or list its binds
const IPC_TIMEOUT: Duration = Duration::from_millis(500);

/// Hyprland's bind list runs to about 100 KB with Omarchy's binds, and ours come last
const BINDS_REPLY_MAX: usize = 4 << 20;

/// Run Lua in Hyprland (its `eval` request, which needs a Lua config: Hyprland 0.56 or later)
fn eval(lua: &str) -> Result<(), String> {
    let reply = hyprland_request_within(format!("/eval {lua}").as_bytes(), IPC_TIMEOUT, 4096)
        .ok_or("Hyprland did not answer")?;
    match reply.trim() {
        "ok" => Ok(()),
        error => Err(error.to_string()),
    }
}

/// Whether Hyprland holds errors from loading its config. Every `eval` empties that list, which
/// `hyprctl configerrors` shows, so nothing is run while it has something to report: the user
/// would see a broken config as a clean one. None when Hyprland does not answer.
fn config_has_errors() -> Option<bool> {
    hyprland_request_within(b"configerrors", IPC_TIMEOUT, 65536)
        .map(|reply| !reply.trim().is_empty())
}

/// How many binds without modifiers each button has in Hyprland's `j/binds` reply, as
/// (consuming, non-consuming) pairs in `BUTTONS` order. None for a reply that is not a bind list.
fn plain_button_binds(json: &str) -> Option<[(usize, usize); 3]> {
    if !json.trim_start().starts_with('[') {
        return None;
    }
    let mut counts = [(0, 0); 3];
    // An array of flat objects, each closing on a line of its own (strings escape newlines)
    for object in json.split("\n}") {
        if extract_json_u32(object, "modmask") != Some(0) {
            continue;
        }
        let Some(key) = extract_json_str(object, "key") else {
            continue;
        };
        if let Some(i) = BUTTONS.iter().position(|b| key == format!("mouse:{b}")) {
            let compact: String = object.split_whitespace().collect();
            if compact.contains("\"non_consuming\":true") {
                counts[i].1 += 1;
            } else {
                counts[i].0 += 1;
            }
        }
    }
    Some(counts)
}

/// Whether registering left every button with a non-consuming bind and added no consuming one.
/// A bind that consumed clicks would take every plain click from the apps.
fn registered_safely(before: [(usize, usize); 3], after: [(usize, usize); 3]) -> bool {
    before.iter().zip(after.iter()).all(
        |(&(consuming_before, _), &(consuming_after, passing_after))| {
            consuming_after == consuming_before && passing_after >= 1
        },
    )
}

/// Add the click binds, then check Hyprland's bind list and switch them off again unless they
/// pass every click through
pub fn register() -> Result<(), String> {
    match config_has_errors() {
        Some(false) => {}
        Some(true) => {
            return Err(
                "Hyprland's config has errors; waiting for a clean reload, so as not to clear the list `hyprctl configerrors` shows".to_string(),
            );
        }
        None => return Err("Hyprland did not answer".to_string()),
    }
    let binds = || {
        hyprland_request_within(b"j/binds", IPC_TIMEOUT, BINDS_REPLY_MAX)
            .as_deref()
            .and_then(plain_button_binds)
    };
    let before = binds().ok_or("cannot read Hyprland's bind list")?;
    eval(REGISTER_LUA)?;
    if binds().is_some_and(|after| registered_safely(before, after)) {
        Ok(())
    } else {
        unregister();
        Err("the click binds did not show up as non-consuming; switched them off".to_string())
    }
}

/// Switch the click binds off. Skipped while the config has errors, whose list it would clear;
/// the binds then keep posting events nobody reads until the next reload removes them.
pub fn unregister() {
    if config_has_errors() != Some(false) {
        return;
    }
    if let Err(e) = eval(UNREGISTER_LUA) {
        eprintln!("[typesuggest] Could not switch off the click binds: {}", e);
    }
}

impl AsFd for EventSocket {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.stream.as_fd()
    }
}

/// What the daemon cares about on Hyprland's event socket
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HyprEvent {
    /// A mouse button went down
    Click,
    /// The config was reloaded, which drops binds added at run time
    ConfigReloaded,
}

/// One line of Hyprland's event socket (`EVENT>>DATA`, without the newline)
pub fn parse_event(line: &str) -> Option<HyprEvent> {
    if let Some(data) = line.strip_prefix("custom>>") {
        (data.split(' ').next() == Some(CLICK_EVENT)).then_some(HyprEvent::Click)
    } else if line.starts_with("configreloaded>>") {
        Some(HyprEvent::ConfigReloaded)
    } else {
        None
    }
}

/// Hyprland's event socket. It carries every event Hyprland posts, and Hyprland drops a client
/// that falls behind, so it is read whenever it has data.
pub struct EventSocket {
    stream: UnixStream,
    /// The start of a line whose end has not arrived yet
    partial: Vec<u8>,
}

impl EventSocket {
    pub fn connect() -> Option<Self> {
        let stream = UnixStream::connect(get_hyprland_event_socket_path()?).ok()?;
        stream.set_nonblocking(true).ok()?;
        Some(Self {
            stream,
            partial: Vec::new(),
        })
    }

    /// The events among the lines waiting on the socket. Err when Hyprland closed it.
    pub fn read_events(&mut self) -> std::io::Result<Vec<HyprEvent>> {
        let mut buf = [0u8; 4096];
        loop {
            match self.stream.read(&mut buf) {
                Ok(0) => return Err(std::io::ErrorKind::UnexpectedEof.into()),
                Ok(n) => self.partial.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        let Some(end) = self.partial.iter().rposition(|&b| b == b'\n') else {
            return Ok(Vec::new());
        };
        let complete: Vec<u8> = self.partial.drain(..=end).collect();
        Ok(String::from_utf8_lossy(&complete)
            .lines()
            .filter_map(parse_event)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_event() {
        assert_eq!(
            parse_event("custom>>typesuggest-click 272"),
            Some(HyprEvent::Click)
        );
        assert_eq!(
            parse_event("custom>>typesuggest-click 274"),
            Some(HyprEvent::Click)
        );
        assert_eq!(
            parse_event("configreloaded>>"),
            Some(HyprEvent::ConfigReloaded)
        );
        // Other programs' custom events, and everything else on the socket
        assert_eq!(parse_event("custom>>typesuggest-clicked"), None);
        assert_eq!(parse_event("custom>>other typesuggest-click"), None);
        assert_eq!(parse_event("activewindow>>foot,~"), None);
        assert_eq!(parse_event(""), None);
    }

    /// Trimmed from a real `j/binds` reply of Hyprland 0.56.2
    fn bind(key: &str, modmask: u32, non_consuming: bool) -> String {
        format!(
            "{{\n    \"locked\": false,\n    \"mouse\": false,\n    \"non_consuming\": {non_consuming},\n    \
             \"modmask\": {modmask},\n    \"submap\": \"\",\n    \"key\": \"{key}\",\n    \
             \"keycode\": 0,\n    \"description\": \"a {{brace}} in text\",\n    \
             \"dispatcher\": \"__lua\",\n    \"arg\": \"7\"\n}}"
        )
    }

    fn list(binds: &[String]) -> String {
        format!("[\n{}\n]\n", binds.join(",\n"))
    }

    fn ours(non_consuming: bool) -> Vec<String> {
        BUTTONS
            .iter()
            .map(|b| bind(&format!("mouse:{b}"), 0, non_consuming))
            .collect()
    }

    #[test]
    fn test_plain_button_binds_are_counted() {
        // SUPER + drag on the same buttons, and other keys, are not plain button binds
        let mut binds = vec![
            bind("mouse:272", 64, false),
            bind("mouse:273", 64, false),
            bind("XF86AudioRaiseVolume", 0, false),
            bind("mouse:274", 0, false),
        ];
        assert_eq!(
            plain_button_binds(&list(&binds)),
            Some([(0, 0), (0, 0), (1, 0)])
        );
        binds.extend(ours(true));
        assert_eq!(
            plain_button_binds(&list(&binds)),
            Some([(0, 1), (0, 1), (1, 1)])
        );
        assert_eq!(plain_button_binds("[]"), Some([(0, 0); 3]));
        assert_eq!(plain_button_binds("error: unknown request"), None);
        assert_eq!(plain_button_binds(""), None);
    }

    #[test]
    fn test_click_binds_must_pass_clicks_through() {
        let count = |binds: &[String]| plain_button_binds(&list(binds)).unwrap();
        // The user's own consuming bind on a button stays, and ours joins it
        let user = vec![bind("mouse:274", 0, false), bind("mouse:272", 64, false)];
        let mut after = user.clone();
        after.extend(ours(true));
        assert!(registered_safely(count(&user), count(&after)));
        // Registering again reuses the same binds
        assert!(registered_safely(count(&after), count(&after)));

        // Ours came out consuming: every plain click would be swallowed
        let mut swallowing = user.clone();
        swallowing.extend(ours(false));
        assert!(!registered_safely(count(&user), count(&swallowing)));
        // A button left without a bind
        let mut missing = user.clone();
        missing.extend(ours(true).into_iter().take(2));
        assert!(!registered_safely(count(&user), count(&missing)));
    }

    #[test]
    fn test_register_lua_keeps_its_own_handles() {
        // Only the stored handles are ever switched; nothing deletes binds by key
        for lua in [REGISTER_LUA, UNREGISTER_LUA] {
            assert!(!lua.contains("unbind") && !lua.contains("remove"), "{lua}");
            assert!(lua.contains("__typesuggest_clicks"), "{lua}");
        }
        assert!(REGISTER_LUA.contains("non_consuming = true"));
        assert!(UNREGISTER_LUA.contains("set_enabled(false)"));
    }
}
