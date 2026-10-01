use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HyprlandWindow {
    pub pid: u32,
    pub class: String,
    pub title: String,
}

/// Extract a u32 value for a given key from simple JSON (e.g. `"pid": 12345`)
pub fn extract_json_u32(json: &str, key: &str) -> Option<u32> {
    let needle = format!("\"{}\":", key);
    let idx = json.find(&needle)?;
    let rest = json[idx + needle.len()..].trim_start();
    let num_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    num_str.parse().ok()
}

/// Extract a string value for a given key from simple JSON (e.g. `"class": "org.omarchy.agent"`)
pub fn extract_json_str(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{}\":", key);
    let idx = json.find(&needle)?;
    let rest = json[idx + needle.len()..].trim_start();
    if let Some(after_quote) = rest.strip_prefix('"') {
        let end_idx = after_quote.find('"')?;
        Some(after_quote[..end_idx].to_string())
    } else {
        None
    }
}

/// Find Hyprland IPC socket path
fn get_hyprland_socket_path() -> Option<PathBuf> {
    let xdg_runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| {
        let uid = rustix::process::getuid().as_raw();
        format!("/run/user/{}", uid)
    });

    if let Ok(his) = std::env::var("HYPRLAND_INSTANCE_SIGNATURE") {
        let candidate = PathBuf::from(&xdg_runtime)
            .join("hypr")
            .join(&his)
            .join(".socket.sock");
        if candidate.exists() {
            return Some(candidate);
        }
    }

    // Fallback: the newest instance in $XDG_RUNTIME_DIR/hypr/*/ that still accepts connections
    // (directories of earlier sessions are left behind)
    let hypr_dir = PathBuf::from(&xdg_runtime).join("hypr");
    let mut sockets: Vec<(std::time::SystemTime, PathBuf)> = fs::read_dir(hypr_dir)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let sock = entry.path().join(".socket.sock");
            let modified = fs::metadata(&sock).ok()?.modified().ok()?;
            Some((modified, sock))
        })
        .collect();
    sockets.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    sockets
        .into_iter()
        .map(|(_, sock)| sock)
        .find(|sock| UnixStream::connect(sock).is_ok())
}

/// Send one request to Hyprland's IPC socket and return the reply
fn hyprland_request(request: &[u8]) -> Option<String> {
    let sock_path = get_hyprland_socket_path()?;
    let mut stream = UnixStream::connect(&sock_path).ok()?;
    stream
        .set_read_timeout(Some(Duration::from_millis(15)))
        .ok()?;
    stream
        .set_write_timeout(Some(Duration::from_millis(15)))
        .ok()?;

    stream.write_all(request).ok()?;

    let mut response = Vec::with_capacity(2048);
    let mut buf = [0u8; 1024];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                response.extend_from_slice(&buf[..n]);
                // Generous cap: "pid" comes after the (possibly long) window title
                if response.len() >= 65536 {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    String::from_utf8(response).ok()
}

/// Query Hyprland for currently focused window via Unix socket IPC
pub fn get_hyprland_active_window() -> Option<HyprlandWindow> {
    let text = hyprland_request(b"j/activewindow")?;
    let pid = extract_json_u32(&text, "pid")?;
    let class = extract_json_str(&text, "class").unwrap_or_default();
    let title = extract_json_str(&text, "title").unwrap_or_default();

    Some(HyprlandWindow { pid, class, title })
}

/// The tallest logical monitor height (rotation taken into account), from Hyprland's monitor list
pub fn get_hyprland_max_monitor_extent() -> Option<u32> {
    max_monitor_extent(&hyprland_request(b"j/monitors")?)
}

/// See [`get_hyprland_max_monitor_extent`]; `text` is Hyprland's `j/monitors` reply. Each monitor
/// object holds every one of these keys once, so the n-th values belong to the n-th monitor.
pub fn max_monitor_extent(text: &str) -> Option<u32> {
    let numbers = |key: &str| -> Vec<f64> {
        let needle = format!("\"{}\":", key);
        text.match_indices(&needle)
            .filter_map(|(i, _)| {
                let rest = text[i + needle.len()..].trim_start();
                let end = rest
                    .find(|c: char| !(c.is_ascii_digit() || c == '.'))
                    .unwrap_or(rest.len());
                rest[..end].parse::<f64>().ok()
            })
            .collect()
    };
    let (widths, heights) = (numbers("width"), numbers("height"));
    let (scales, transforms) = (numbers("scale"), numbers("transform"));
    let tallest = (0..heights.len())
        .filter_map(|i| {
            // Odd transforms rotate by 90 or 270 degrees, swapping width and height
            let rotated = transforms.get(i).is_some_and(|t| *t as u32 % 2 == 1);
            let pixels = if rotated { *widths.get(i)? } else { heights[i] };
            let scale = scales.get(i).copied().filter(|s| *s > 0.0).unwrap_or(1.0);
            Some(pixels / scale)
        })
        .fold(0.0f64, f64::max);
    (tallest > 0.0).then(|| tallest.ceil() as u32)
}

/// Check if a process comm name indicates a password, credential, or authentication prompt
pub fn is_sensitive_comm(comm: &str) -> bool {
    let c = comm.trim();
    matches!(
        c,
        "sudo"
            | "su"
            | "doas"
            | "passwd"
            | "pkexec"
            | "ssh"
            | "login"
            | "cryptsetup"
            | "op"
            | "bw"
            | "vault"
            | "pass"
            | "openconnect"
            | "openvpn"
            | "keepassxc-cli"
    ) || c.starts_with("pinentry")
}

/// Upper bound on processes inspected per check, so a huge process tree stays cheap
const MAX_TREE_PROCESSES: usize = 128;

/// Terminal multiplexer client process names and the server whose panes they display. The
/// server is not a descendant of the terminal window, so its panes are inspected separately.
const MULTIPLEXERS: &[(&str, &str)] = &[
    ("tmux: client", "tmux: server"),
    ("screen", "screen"),
    ("zellij", "zellij"),
];

/// Inspect /proc to check whether `root_pid` or any of its descendants is a sensitive command,
/// or a terminal under it is reading a password (see [`tty_reads_password`]). Panes of terminal
/// multiplexers attached from this window are checked too.
pub fn has_sensitive_descendant(root_pid: u32) -> bool {
    if root_pid == 0 {
        return false;
    }

    let tree = process_tree(root_pid);
    let mut seen_ttys = HashSet::new();
    if tree_is_sensitive(&tree, &mut seen_ttys) {
        return true;
    }

    for &(client, server) in MULTIPLEXERS {
        if tree.iter().any(|&pid| comm(pid).as_deref() == Some(client)) {
            for server_pid in own_processes_named(server) {
                if !tree.contains(&server_pid)
                    && tree_is_sensitive(&process_tree(server_pid), &mut seen_ttys)
                {
                    return true;
                }
            }
        }
    }

    false
}

/// `root_pid` and its descendants (children of every thread, since terminals may start the
/// shell from a worker thread), up to MAX_TREE_PROCESSES
fn process_tree(root_pid: u32) -> Vec<u32> {
    let mut tree = Vec::new();
    let mut stack = vec![root_pid];
    let mut visited = HashSet::new();

    while let Some(pid) = stack.pop() {
        if tree.len() >= MAX_TREE_PROCESSES {
            break;
        }
        if !visited.insert(pid) {
            continue;
        }
        tree.push(pid);

        let Ok(tasks) = fs::read_dir(format!("/proc/{}/task", pid)) else {
            continue;
        };
        for task in tasks.flatten() {
            if let Ok(content) = fs::read_to_string(task.path().join("children")) {
                stack.extend(
                    content
                        .split_whitespace()
                        .filter_map(|t| t.parse::<u32>().ok()),
                );
            }
        }
    }

    tree
}

fn tree_is_sensitive(tree: &[u32], seen_ttys: &mut HashSet<PathBuf>) -> bool {
    tree.iter().any(|&pid| {
        comm(pid).is_some_and(|c| is_sensitive_comm(&c)) || tty_reads_password(pid, seen_ttys)
    })
}

fn comm(pid: u32) -> Option<String> {
    fs::read_to_string(format!("/proc/{}/comm", pid))
        .ok()
        .map(|c| c.trim().to_string())
}

/// Processes of the current user whose name is exactly `name`
fn own_processes_named(name: &str) -> Vec<u32> {
    use std::os::unix::fs::MetadataExt;
    let uid = rustix::process::getuid().as_raw();
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
        .filter(|&pid| {
            fs::metadata(format!("/proc/{}", pid)).is_ok_and(|m| m.uid() == uid)
                && comm(pid).as_deref() == Some(name)
        })
        .collect()
}

/// Whether the pseudo-terminal on `pid`'s stdin is reading a password: echo is off while input
/// is still line-buffered (canonical mode). That is what sudo, ssh, passwd, `read -s`, getpass()
/// and pinentry-tty set. Shell line editors (readline, zle, fish) also turn echo off, but they
/// leave canonical mode too, so a normal prompt does not count. Each terminal is checked once.
fn tty_reads_password(pid: u32, seen_ttys: &mut HashSet<PathBuf>) -> bool {
    use rustix::fs::{Mode, OFlags};
    use rustix::termios::LocalModes;

    let stdin = format!("/proc/{}/fd/0", pid);
    let Ok(target) = fs::read_link(&stdin) else {
        return false;
    };
    if !target.starts_with("/dev/pts/") || !seen_ttys.insert(target) {
        return false;
    }
    // Opening the terminal read-only with NOCTTY has no effect on it or on our session
    let Ok(fd) = rustix::fs::open(
        stdin.as_str(),
        OFlags::RDONLY | OFlags::NOCTTY | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) else {
        return false;
    };
    rustix::termios::tcgetattr(&fd).is_ok_and(|t| {
        !t.local_modes.contains(LocalModes::ECHO) && t.local_modes.contains(LocalModes::ICANON)
    })
}

/// Check if window title or class indicates an authentication or password dialog
pub fn is_sensitive_window(title: &str, class: &str) -> bool {
    let t = title.to_lowercase();
    let c = class.to_lowercase();

    // Check title patterns
    if t.contains("[sudo]")
        || t.contains("password:")
        || t.contains("password for")
        || t.contains("enter passphrase")
        || t.contains("authentication required")
        || t.contains("pinentry")
    {
        return true;
    }

    // Check class patterns
    if c.contains("polkit")
        || c.contains("pinentry")
        || c.contains("lxpolkit")
        || c.contains("gcr-prompter")
        || c.contains("hyprpolkitagent")
    {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_max_monitor_extent() {
        // Trimmed from a real `hyprctl -j monitors` reply
        let one = r#"[{"id": 0, "name": "eDP-1", "width": 1920, "height": 1080, "refreshRate": 60.01,
            "x": 0, "y": 0, "activeWorkspace": {"id": 1, "name": "1"}, "reserved": [0, 0, 0, 0],
            "scale": 1.25, "transform": 0, "availableModes": ["1920x1080@60.01Hz"]}]"#;
        assert_eq!(max_monitor_extent(one), Some(864));

        // A second, portrait monitor at scale 1 is the tallest
        let two = format!(
            "{},{}",
            &one[..one.len() - 1],
            r#"{"id": 1, "width": 2560, "height": 1440, "scale": 1.00, "transform": 1}]"#
        );
        assert_eq!(max_monitor_extent(&two), Some(2560));
        assert_eq!(max_monitor_extent("[]"), None);
    }

    #[test]
    fn test_extract_json_u32() {
        let json = r#"{"address": "0x123", "pid": 545948, "class": "foot"}"#;
        assert_eq!(extract_json_u32(json, "pid"), Some(545948));
        assert_eq!(extract_json_u32(json, "missing"), None);
    }

    #[test]
    fn test_extract_json_str() {
        let json = r#"{"pid": 545948, "class": "org.omarchy.agent", "title": "foot terminal"}"#;
        assert_eq!(
            extract_json_str(json, "class"),
            Some("org.omarchy.agent".to_string())
        );
        assert_eq!(
            extract_json_str(json, "title"),
            Some("foot terminal".to_string())
        );
        assert_eq!(extract_json_str(json, "missing"), None);
    }

    #[test]
    fn test_is_sensitive_comm() {
        assert!(is_sensitive_comm("sudo"));
        assert!(is_sensitive_comm("su"));
        assert!(is_sensitive_comm("doas"));
        assert!(is_sensitive_comm("passwd"));
        assert!(is_sensitive_comm("pkexec"));
        assert!(is_sensitive_comm("pinentry"));
        assert!(is_sensitive_comm("pinentry-curses"));
        assert!(is_sensitive_comm("pinentry-qt"));
        assert!(is_sensitive_comm("ssh"));
        assert!(is_sensitive_comm("cryptsetup"));
        assert!(is_sensitive_comm("op"));
        assert!(is_sensitive_comm("bw"));

        assert!(!is_sensitive_comm("bash"));
        assert!(!is_sensitive_comm("foot"));
        assert!(!is_sensitive_comm("alacritty"));
        assert!(!is_sensitive_comm("cargo"));
        assert!(!is_sensitive_comm("firefox"));
    }

    #[test]
    fn test_is_sensitive_window() {
        assert!(is_sensitive_window("[sudo] password for user:", "foot"));
        assert!(is_sensitive_window(
            "Authentication Required",
            "org.kde.polkit-kde-authentication-agent-1"
        ));
        assert!(is_sensitive_window("Terminal", "lxpolkit"));
        assert!(is_sensitive_window("Pinentry", "pinentry-gtk"));
        assert!(!is_sensitive_window(
            "Editing main.rs - Helix",
            "org.omarchy.agent"
        ));
        assert!(!is_sensitive_window("Google Chrome", "google-chrome"));
    }
}
