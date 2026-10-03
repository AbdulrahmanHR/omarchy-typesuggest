use crate::theme::{ColorOverrides, Theme, parse_hex_color};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Smallest and largest accepted suggestion bar size multipliers
pub const MIN_BAR_SCALE: f32 = 0.5;
pub const MAX_BAR_SCALE: f32 = 3.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub learn: bool,
    pub min_prefix_length: usize,
    pub max_candidates: usize,
    /// Size of the suggestion bar relative to the default (1.0)
    pub bar_scale: f32,
    /// Show the bar below the caret (default) or above it
    pub bar_position: BarPosition,
    /// Bar colors: follow Omarchy's theme or always use the built-in palette
    pub theme: Theme,
    /// `color_*` overrides applied on top of the theme
    pub colors: ColorOverrides,
    /// Key that moves from the document into the suggestion bar
    pub select_key: SelectKey,
    /// Keys that commit the highlighted suggestion while navigating
    pub accept_keys: AcceptKeys,
    /// Append a space after a committed word
    pub trailing_space: bool,
    /// Highlight the suggestion under the mouse the way the keyboard selection is drawn
    pub hover_highlight: bool,
    /// With `bar_position = "above"`: hide the bar as soon as the mouse moves over the empty
    /// area above it, rather than when that area is clicked
    pub mouse_hides_bar: bool,
    /// Lowercase Hyprland window classes (a trailing `*` matches any suffix) where typesuggest
    /// stays out of the way
    pub disabled_apps: Vec<String>,
    /// Fall back to similar words when nothing starts with the typed prefix
    pub typo_correction: bool,
    /// Font file path or fontconfig family; empty for the built-in font chain
    pub font: String,
}

/// Keys that commit the highlighted suggestion while navigating (`accept_keys`)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcceptKeys {
    /// Return and keypad Enter
    pub enter: bool,
    pub space: bool,
    pub tab: bool,
}

impl Default for AcceptKeys {
    fn default() -> Self {
        Self {
            enter: true,
            space: true,
            tab: true,
        }
    }
}

impl AcceptKeys {
    pub fn names(&self) -> Vec<&'static str> {
        [
            (self.enter, "enter"),
            (self.space, "space"),
            (self.tab, "tab"),
        ]
        .into_iter()
        .filter_map(|(on, name)| on.then_some(name))
        .collect()
    }
}

/// Where the suggestion bar appears relative to the text caret
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BarPosition {
    #[default]
    Below,
    Above,
}

impl BarPosition {
    pub fn parse(val: &str) -> Option<Self> {
        match val.trim().to_lowercase().as_str() {
            "below" => Some(Self::Below),
            "above" => Some(Self::Above),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Below => "below",
            Self::Above => "above",
        }
    }
}

/// The arrow key that moves from the document into the suggestion bar (`select_key`)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectKey {
    #[default]
    Up,
    Down,
}

impl SelectKey {
    pub fn parse(val: &str) -> Option<Self> {
        match val.trim().to_lowercase().as_str() {
            "up" => Some(Self::Up),
            "down" => Some(Self::Down),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
        }
    }
}

/// Parse a suggestion bar size multiplier, clamped to the supported range
pub fn parse_bar_scale(val: &str) -> Option<f32> {
    val.trim()
        .parse::<f32>()
        .ok()
        .filter(|s| s.is_finite())
        .map(|s| s.clamp(MIN_BAR_SCALE, MAX_BAR_SCALE))
}

/// Split a list written as `a, b, c` or `["a", "b", "c"]` into trimmed, unquoted items
pub fn parse_list(val: &str) -> Vec<String> {
    let val = val.trim();
    let val = val.strip_prefix('[').unwrap_or(val);
    let val = val.strip_suffix(']').unwrap_or(val);
    val.split(',')
        .map(|item| item.trim().trim_matches(['"', '\'']).trim())
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

/// Parse `accept_keys`; None when the list names no known key (enter, space, tab)
pub fn parse_accept_keys(val: &str) -> Option<AcceptKeys> {
    let mut keys = AcceptKeys {
        enter: false,
        space: false,
        tab: false,
    };
    for name in parse_list(val) {
        match name.to_lowercase().as_str() {
            "enter" | "return" => keys.enter = true,
            "space" => keys.space = true,
            "tab" => keys.tab = true,
            _ => {}
        }
    }
    (keys.enter || keys.space || keys.tab).then_some(keys)
}

/// Settings `typesuggest --set` can change, with the other spellings the parser also reads
const SETTABLE_KEYS: &[(&str, &[&str])] = &[
    ("learn", &["learn_phrases", "dynamic_learning"]),
    ("min_prefix_length", &["min_prefix"]),
    ("max_candidates", &["candidates"]),
    ("bar_scale", &["scale", "size"]),
    ("bar_position", &["position"]),
    ("theme", &[]),
    ("select_key", &[]),
    ("accept_keys", &[]),
    ("trailing_space", &[]),
    ("hover_highlight", &[]),
    ("mouse_hides_bar", &[]),
    ("typo_correction", &[]),
    ("disabled_apps", &[]),
    ("font", &[]),
];

/// Validate `value` for `key` and return the config.toml line that stores it. Stricter than the
/// file parser (which skips bad values): out-of-range numbers and unknown names are errors.
pub fn config_line(key: &str, value: &str) -> Result<String, String> {
    let value = value.trim();
    let invalid = || format!("invalid value for {}: {:?}", key, value);
    let parse_bool = || match value.to_lowercase().as_str() {
        "true" | "on" | "yes" => Ok(true),
        "false" | "off" | "no" => Ok(false),
        _ => Err(invalid()),
    };
    let quoted = |text: &str| -> Result<String, String> {
        if text.chars().any(|c| c.is_control() || c == '"') {
            return Err(invalid());
        }
        Ok(format!("\"{}\"", text))
    };
    let quoted_list = |items: &[String]| -> Result<String, String> {
        let items: Result<Vec<String>, String> = items.iter().map(|i| quoted(i)).collect();
        Ok(format!("[{}]", items?.join(", ")))
    };

    let stored = match key {
        "learn" | "trailing_space" | "typo_correction" | "hover_highlight" | "mouse_hides_bar" => {
            parse_bool()?.to_string()
        }
        "min_prefix_length" | "max_candidates" => {
            let max = if key == "max_candidates" { 5 } else { 10 };
            let n = value.parse::<usize>().map_err(|_| invalid())?;
            if !(1..=max).contains(&n) {
                return Err(format!("{} must be between 1 and {}", key, max));
            }
            n.to_string()
        }
        "bar_scale" => {
            let scale = value.parse::<f32>().map_err(|_| invalid())?;
            if !(scale.is_finite() && (MIN_BAR_SCALE..=MAX_BAR_SCALE).contains(&scale)) {
                return Err(format!(
                    "bar_scale must be between {} and {}",
                    MIN_BAR_SCALE, MAX_BAR_SCALE
                ));
            }
            scale.to_string()
        }
        "bar_position" => quoted(BarPosition::parse(value).ok_or_else(invalid)?.name())?,
        "theme" => quoted(Theme::parse(value).ok_or_else(invalid)?.name())?,
        "select_key" => quoted(
            SelectKey::parse(value)
                .ok_or_else(|| "select_key takes up or down".to_string())?
                .name(),
        )?,
        "accept_keys" => {
            let names = parse_list(value);
            let known = |n: &String| {
                matches!(
                    n.to_lowercase().as_str(),
                    "enter" | "return" | "space" | "tab"
                )
            };
            if names.is_empty() || !names.iter().all(known) {
                return Err("accept_keys takes one or more of enter, space, tab".to_string());
            }
            let keys = parse_accept_keys(value).ok_or_else(invalid)?;
            let names: Vec<String> = keys.names().into_iter().map(str::to_string).collect();
            quoted_list(&names)?
        }
        "disabled_apps" => quoted_list(&parse_list(value))?,
        "font" => quoted(value)?,
        _ => {
            let known: Vec<&str> = SETTABLE_KEYS.iter().map(|(k, _)| *k).collect();
            return Err(format!(
                "unknown setting {:?} (known: {})",
                key,
                known.join(", ")
            ));
        }
    };
    Ok(format!("{} = {}", key, stored))
}

/// `content` with the setting `key` replaced by `line`. Every spelling of the key is replaced
/// (later lines would otherwise win), multi-line lists are removed whole, and comments and other
/// settings are kept. A missing setting is appended.
pub fn with_setting(content: &str, key: &str, line: &str) -> String {
    let aliases = SETTABLE_KEYS
        .iter()
        .find(|(k, _)| *k == key)
        .map_or(&[][..], |(_, a)| *a);
    let mut out = Vec::new();
    let mut written = false;
    let mut lines = content.lines();
    while let Some(current) = lines.next() {
        let trimmed = current.trim();
        let setting = (!trimmed.starts_with('#') && !trimmed.starts_with(';'))
            .then(|| trimmed.split_once('='))
            .flatten();
        let Some((k, raw)) = setting else {
            out.push(current.to_string());
            continue;
        };
        let k = k.trim().to_lowercase();
        if k != key && !aliases.contains(&k.as_str()) {
            out.push(current.to_string());
            continue;
        }
        // Skip the rest of a multi-line list belonging to the replaced setting
        let raw = raw.trim();
        if raw.starts_with('[') && !raw.contains(']') {
            for next in lines.by_ref() {
                if next.contains(']') {
                    break;
                }
            }
        }
        if !written {
            out.push(line.to_string());
            written = true;
        }
    }
    if !written {
        if out.last().is_some_and(|l| !l.trim().is_empty()) {
            out.push(String::new());
        }
        out.push(line.to_string());
    }
    out.join("\n") + "\n"
}

/// `text` as a JSON string literal
fn json_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Case-insensitive window class match; a trailing `*` in the pattern matches any suffix
pub fn app_pattern_matches(pattern: &str, class: &str) -> bool {
    let (pattern, class) = (pattern.to_lowercase(), class.to_lowercase());
    match pattern.strip_suffix('*') {
        Some(prefix) => class.starts_with(prefix),
        None => class == pattern,
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            learn: true,
            min_prefix_length: 1,
            max_candidates: 3,
            bar_scale: 1.0,
            bar_position: BarPosition::Below,
            theme: Theme::Omarchy,
            colors: ColorOverrides::default(),
            select_key: SelectKey::Up,
            accept_keys: AcceptKeys::default(),
            trailing_space: true,
            hover_highlight: true,
            mouse_hides_bar: false,
            disabled_apps: Vec::new(),
            typo_correction: true,
            font: String::new(),
        }
    }
}

impl Config {
    pub fn load_from_file(path: &Path) -> Self {
        let mut config = Self::default();
        if let Ok(content) = std::fs::read_to_string(path) {
            config.parse_str(&content);
        }
        config
    }

    pub fn parse_str(&mut self, text: &str) {
        let mut lines = text.lines().peekable();
        while let Some(line) = lines.next() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            if let Some((key, raw_val)) = line.split_once('=') {
                let key = key.trim().to_lowercase();
                let mut raw_val = raw_val.trim().to_string();
                // A TOML array may continue over the following lines up to its closing bracket.
                // The next `key = value` line ends it even if the bracket is missing.
                if raw_val.starts_with('[') && !raw_val.contains(']') {
                    while let Some(next) = lines.next_if(|next| !next.contains('=')) {
                        let next = next.trim();
                        if !next.starts_with('#') {
                            raw_val.push(',');
                            raw_val.push_str(next);
                        }
                        if next.contains(']') {
                            break;
                        }
                    }
                }
                let val = raw_val.trim_matches('"').trim_matches('\'').trim();
                match key.as_str() {
                    "learn" | "learn_phrases" | "dynamic_learning" => {
                        if let Ok(b) = val.parse::<bool>() {
                            self.learn = b;
                        }
                    }
                    "min_prefix_length" | "min_prefix" => {
                        if let Ok(n) = val.parse::<usize>() {
                            self.min_prefix_length = n.clamp(1, 10);
                        }
                    }
                    "max_candidates" | "candidates" => {
                        if let Ok(n) = val.parse::<usize>() {
                            self.max_candidates = n.clamp(1, 5);
                        }
                    }
                    "bar_scale" | "scale" | "size" => {
                        if let Some(scale) = parse_bar_scale(val) {
                            self.bar_scale = scale;
                        }
                    }
                    "bar_position" | "position" => {
                        if let Some(position) = BarPosition::parse(val) {
                            self.bar_position = position;
                        }
                    }
                    "theme" => {
                        if let Some(theme) = Theme::parse(val) {
                            self.theme = theme;
                        }
                    }
                    "select_key" => {
                        if let Some(key) = SelectKey::parse(val) {
                            self.select_key = key;
                        }
                    }
                    "accept_keys" => {
                        self.accept_keys = parse_accept_keys(&raw_val).unwrap_or_default();
                    }
                    "trailing_space" => {
                        if let Ok(b) = val.parse::<bool>() {
                            self.trailing_space = b;
                        }
                    }
                    "hover_highlight" => {
                        if let Ok(b) = val.parse::<bool>() {
                            self.hover_highlight = b;
                        }
                    }
                    "mouse_hides_bar" => {
                        if let Ok(b) = val.parse::<bool>() {
                            self.mouse_hides_bar = b;
                        }
                    }
                    "disabled_apps" => {
                        self.disabled_apps = parse_list(&raw_val)
                            .into_iter()
                            .map(|app| app.to_lowercase())
                            .collect();
                    }
                    "typo_correction" => {
                        if let Ok(b) = val.parse::<bool>() {
                            self.typo_correction = b;
                        }
                    }
                    "font" => {
                        self.font = val.to_string();
                    }
                    other => {
                        if let (Some(slot), Some(color)) =
                            (self.colors.slot(other), parse_hex_color(val))
                        {
                            *slot = Some(color);
                        }
                    }
                }
            }
        }
    }

    /// Whether suggestions are turned off for windows of this Hyprland class (`disabled_apps`)
    /// The settings as one JSON object, for tools such as the Omarchy plugin
    pub fn to_json(&self) -> String {
        let list = |items: &[&str]| {
            let items: Vec<String> = items.iter().map(|i| json_string(i)).collect();
            format!("[{}]", items.join(","))
        };
        let apps: Vec<&str> = self.disabled_apps.iter().map(String::as_str).collect();
        format!(
            "{{\"learn\":{},\"min_prefix_length\":{},\"max_candidates\":{},\"bar_scale\":{},\"bar_position\":{},\"theme\":{},\"select_key\":{},\"accept_keys\":{},\"trailing_space\":{},\"hover_highlight\":{},\"mouse_hides_bar\":{},\"typo_correction\":{},\"disabled_apps\":{},\"font\":{}}}",
            self.learn,
            self.min_prefix_length,
            self.max_candidates,
            self.bar_scale,
            json_string(self.bar_position.name()),
            json_string(self.theme.name()),
            json_string(self.select_key.name()),
            list(&self.accept_keys.names()),
            self.trailing_space,
            self.hover_highlight,
            self.mouse_hides_bar,
            self.typo_correction,
            list(&apps),
            json_string(&self.font),
        )
    }

    pub fn is_disabled_app(&self, class: &str) -> bool {
        self.disabled_apps
            .iter()
            .any(|pattern| app_pattern_matches(pattern, class))
    }

    pub fn default_config_file_content() -> &'static str {
        r##"# typesuggest configuration file
# Location: ~/.config/typesuggest/config.toml
# Changes apply the next time a text field is focused; no restart needed.

# Enable or disable dynamic learning of user bigrams/phrases
# When enabled, typing combinations will be learned and prioritized in future suggestions.
learn = true

# Minimum number of letters typed before suggestions popup appears (default: 1)
min_prefix_length = 1

# Maximum number of suggestion pills to display in popup bar (1 - 5, default: 3)
max_candidates = 3

# Size of the suggestion bar as a multiplier of the default size (0.5 - 3.0, default: 1.0)
# e.g. 1.25 makes the bar 25% larger, 0.8 makes it 20% smaller
bar_scale = 1.0

# Where the bar appears (default: "below"): "below" or "above" the text caret.
# Near the bottom of the screen the bar always goes above. With "above", the empty area above
# the bar takes the mouse while the bar shows: a click or scroll there hides the bar (move the
# mouse a little, then click the text again), or set mouse_hides_bar below to hide it as soon
# as the mouse moves there.
bar_position = "below"

# Bar colors (default: "omarchy"): "omarchy" follows the current Omarchy theme and falls back to
# the built-in dark teal palette when no Omarchy theme is found; "default" always uses that palette
theme = "omarchy"

# Optional color overrides that win over the theme, as "#rrggbb" or "#rrggbbaa" (default: unset)
color_background = ""
color_border = ""
color_pill = ""
color_pill_border = ""
color_text = ""
color_accent = ""
color_accent_text = ""

# Arrow key that moves into the suggestion bar while it is showing (default: "up"): "up" or
# "down". Left/Right then move between suggestions; the other arrow or Escape goes back to the text.
select_key = "up"

# Keys that commit the highlighted suggestion after pressing the select key
# (default: enter, space, tab). Any other key leaves the suggestions and reaches the app as usual.
accept_keys = enter, space, tab

# Insert a space after the committed word (default: true)
trailing_space = true

# Highlight the suggestion under the mouse, the way the keyboard selection is shown
# (default: true). A click takes the word under the mouse either way.
hover_highlight = true

# With bar_position = "above": hide the bar as soon as the mouse moves over the empty area
# above it, so a click there always reaches the text (default: false, hide only on a click)
mouse_hides_bar = false

# Hyprland window classes where typesuggest stays off, case-insensitive (default: none)
# A trailing * matches any suffix, e.g. disabled_apps = code, org.wezfurlong.wezterm, steam*
# Find a window's class with: hyprctl activewindow
disabled_apps =

# Suggest similar words for typos when nothing matches the typed prefix (default: true)
typo_correction = true

# Font for the suggestion bar (default: "" = Segoe UI, Liberation Sans or DejaVu Sans)
# Either a font family name such as "Inter" or a path to a .ttf/.otf file
font = ""
"##
    }
}

/// Settings given on the command line. They win over config.toml, also after a live reload.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CliOverrides {
    pub learn: Option<bool>,
    pub min_prefix_length: Option<usize>,
    pub max_candidates: Option<usize>,
    pub bar_scale: Option<f32>,
    pub bar_position: Option<BarPosition>,
    pub typo_correction: Option<bool>,
}

impl CliOverrides {
    pub fn apply(&self, config: &mut Config) {
        if let Some(learn) = self.learn {
            config.learn = learn;
        }
        if let Some(n) = self.min_prefix_length {
            config.min_prefix_length = n;
        }
        if let Some(n) = self.max_candidates {
            config.max_candidates = n;
        }
        if let Some(scale) = self.bar_scale {
            config.bar_scale = scale;
        }
        if let Some(position) = self.bar_position {
            config.bar_position = position;
        }
        if let Some(typo) = self.typo_correction {
            config.typo_correction = typo;
        }
    }
}

/// Identifies a file's current contents from a single stat, without reading it
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStamp {
    modified: Option<SystemTime>,
    len: u64,
    inode: u64,
}

pub fn file_stamp(path: &Path) -> Option<FileStamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some(FileStamp {
        modified: meta.modified().ok(),
        len: meta.len(),
        inode: meta.ino(),
    })
}

/// config.toml plus the command-line overrides, re-read whenever the file changes on disk
#[derive(Debug)]
pub struct ConfigSource {
    path: PathBuf,
    overrides: CliOverrides,
    stamp: Option<FileStamp>,
}

impl ConfigSource {
    pub fn new(path: PathBuf, overrides: CliOverrides) -> Self {
        Self {
            path,
            overrides,
            stamp: None,
        }
    }

    pub fn load(&mut self) -> Config {
        self.stamp = file_stamp(&self.path);
        self.read()
    }

    /// The new config if the file changed since it was last read; costs one stat otherwise.
    /// A missing file keeps the current settings (e.g. mid-save by an editor).
    pub fn reload_if_changed(&mut self) -> Option<Config> {
        let stamp = file_stamp(&self.path)?;
        if self.stamp == Some(stamp) {
            return None;
        }
        self.stamp = Some(stamp);
        Some(self.read())
    }

    fn read(&self) -> Config {
        let mut config = Config::load_from_file(&self.path);
        self.overrides.apply(&mut config);
        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Rgba;

    #[test]
    fn test_default_config() {
        let cfg = Config::default();
        assert!(cfg.learn);
        assert_eq!(cfg.min_prefix_length, 1);
        assert_eq!(cfg.max_candidates, 3);
        assert_eq!(cfg.bar_scale, 1.0);
        assert_eq!(cfg.theme, Theme::Omarchy);
        assert_eq!(cfg.colors, ColorOverrides::default());
        assert_eq!(cfg.select_key, SelectKey::Up);
        assert!(cfg.hover_highlight);
        assert!(!cfg.mouse_hides_bar);
        assert_eq!(cfg.accept_keys, AcceptKeys::default());
        assert!(cfg.trailing_space);
        assert!(cfg.disabled_apps.is_empty());
        assert!(cfg.typo_correction);
        assert_eq!(cfg.font, "");
    }

    #[test]
    fn test_default_file_parses_to_defaults() {
        let mut cfg = Config::default();
        cfg.parse_str(Config::default_config_file_content());
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn test_parse_custom_config() {
        let mut cfg = Config::default();
        let content = r##"
            # Some comment
            learn = false
            min_prefix_length = 2
            max_candidates = 5
            bar_scale = 1.5
            theme = "default"
            color_accent = "#ff8800"
            color_background = #10203040
            color_text = "not a color"
            trailing_space = false
            hover_highlight = false
            mouse_hides_bar = true
            typo_correction = false
            font = "JetBrains Mono"
        "##;
        cfg.parse_str(content);
        assert!(!cfg.learn);
        assert_eq!(cfg.min_prefix_length, 2);
        assert_eq!(cfg.max_candidates, 5);
        assert_eq!(cfg.bar_scale, 1.5);
        assert_eq!(cfg.theme, Theme::Default);
        assert_eq!(cfg.colors.accent, Some(Rgba::new(255, 136, 0, 255)));
        assert_eq!(cfg.colors.background, Some(Rgba::new(16, 32, 48, 64)));
        assert_eq!(cfg.colors.text, None);
        assert!(!cfg.trailing_space);
        assert!(!cfg.hover_highlight);
        assert!(cfg.mouse_hides_bar);
        assert!(!cfg.typo_correction);
        assert_eq!(cfg.font, "JetBrains Mono");
    }

    #[test]
    fn test_parse_clamping() {
        let mut cfg = Config::default();
        let content = "min_prefix_length = 0\nmax_candidates = 100\nbar_scale = 10\n";
        cfg.parse_str(content);
        assert_eq!(cfg.min_prefix_length, 1);
        assert_eq!(cfg.max_candidates, 5);
        assert_eq!(cfg.bar_scale, MAX_BAR_SCALE);
    }

    #[test]
    fn test_config_line_validates() {
        assert_eq!(
            config_line("bar_position", "Above").unwrap(),
            "bar_position = \"above\""
        );
        assert_eq!(
            config_line("bar_scale", "1.25").unwrap(),
            "bar_scale = 1.25"
        );
        assert_eq!(config_line("learn", "off").unwrap(), "learn = false");
        assert_eq!(
            config_line("select_key", "Down").unwrap(),
            "select_key = \"down\""
        );
        assert!(config_line("select_key", "left").is_err());
        assert_eq!(
            config_line("hover_highlight", "off").unwrap(),
            "hover_highlight = false"
        );
        assert_eq!(
            config_line("mouse_hides_bar", "yes").unwrap(),
            "mouse_hides_bar = true"
        );
        assert!(config_line("mouse_hides_bar", "sometimes").is_err());
        assert_eq!(
            config_line("accept_keys", "tab, Return").unwrap(),
            "accept_keys = [\"enter\", \"tab\"]"
        );
        assert_eq!(
            config_line("disabled_apps", "").unwrap(),
            "disabled_apps = []"
        );
        assert!(config_line("max_candidates", "9").is_err());
        assert!(config_line("bar_scale", "NaN").is_err());
        assert!(config_line("accept_keys", "tab, entr").is_err());
        assert!(config_line("font", "Inter\"\nlearn = true").is_err());
        assert!(config_line("nope", "1").is_err());
    }

    #[test]
    fn test_with_setting_replaces_every_spelling() {
        let content =
            "# Size\nscale = 1.0\nlearn = true\ndisabled_apps = [\n  \"code\",\n]\nsize = 2\n";
        let out = with_setting(content, "bar_scale", "bar_scale = 1.5");
        assert_eq!(
            out,
            "# Size\nbar_scale = 1.5\nlearn = true\ndisabled_apps = [\n  \"code\",\n]\n"
        );

        let out = with_setting(&out, "disabled_apps", "disabled_apps = []");
        assert_eq!(
            out,
            "# Size\nbar_scale = 1.5\nlearn = true\ndisabled_apps = []\n"
        );

        let out = with_setting(&out, "font", "font = \"Inter\"");
        assert!(out.ends_with("disabled_apps = []\n\nfont = \"Inter\"\n"));

        // The default file round-trips through --set and parses back to the new value
        let mut cfg = Config::default();
        cfg.parse_str(&with_setting(
            Config::default_config_file_content(),
            "max_candidates",
            "max_candidates = 5",
        ));
        assert_eq!(cfg.max_candidates, 5);
    }

    #[test]
    fn test_to_json() {
        let cfg = Config {
            disabled_apps: vec!["code".into()],
            font: "My \"Font\"".into(),
            ..Config::default()
        };
        let json = cfg.to_json();
        assert!(json.starts_with(
            "{\"learn\":true,\"min_prefix_length\":1,\"max_candidates\":3,\"bar_scale\":1,"
        ));
        assert!(
            json.contains("\"select_key\":\"up\",\"accept_keys\":[\"enter\",\"space\",\"tab\"]")
        );
        assert!(json.contains("\"disabled_apps\":[\"code\"]"));
        assert!(json.contains("\"hover_highlight\":true,\"mouse_hides_bar\":false"));
        assert!(json.ends_with("\"font\":\"My \\\"Font\\\"\"}"));
    }

    #[test]
    fn test_bar_position() {
        assert_eq!(Config::default().bar_position, BarPosition::Below);
        let mut cfg = Config::default();
        cfg.parse_str("bar_position = \"Above\"\n");
        assert_eq!(cfg.bar_position, BarPosition::Above);
        cfg.parse_str("bar_position = sideways\n");
        assert_eq!(
            cfg.bar_position,
            BarPosition::Above,
            "invalid values are ignored"
        );
        cfg.parse_str("position = below\n");
        assert_eq!(cfg.bar_position, BarPosition::Below);
    }

    #[test]
    fn test_select_key() {
        let mut cfg = Config::default();
        cfg.parse_str("select_key = \"Down\"\n");
        assert_eq!(cfg.select_key, SelectKey::Down);
        cfg.parse_str("select_key = sideways\n");
        assert_eq!(
            cfg.select_key,
            SelectKey::Down,
            "invalid values are ignored"
        );
        cfg.parse_str("select_key = up\n");
        assert_eq!(cfg.select_key, SelectKey::Up);
    }

    #[test]
    fn test_bar_scale_rejects_garbage() {
        assert_eq!(parse_bar_scale("0.1"), Some(MIN_BAR_SCALE));
        assert_eq!(parse_bar_scale(" 1.25 "), Some(1.25));
        assert_eq!(parse_bar_scale("NaN"), None);
        assert_eq!(parse_bar_scale("inf"), None);
        assert_eq!(parse_bar_scale("big"), None);

        let mut cfg = Config::default();
        cfg.parse_str("bar_scale = nan\n");
        assert_eq!(cfg.bar_scale, 1.0);
    }

    #[test]
    fn test_parse_list_syntaxes() {
        assert_eq!(
            parse_list("code, org.wezfurlong.wezterm"),
            ["code", "org.wezfurlong.wezterm"]
        );
        assert_eq!(parse_list(r#"["code", 'steam*' ,]"#), ["code", "steam*"]);
        assert_eq!(parse_list(r#""code", "kitty""#), ["code", "kitty"]);
        assert!(parse_list("").is_empty());
        assert!(parse_list("[]").is_empty());
    }

    #[test]
    fn test_parse_accept_keys() {
        let only = |enter, space, tab| AcceptKeys { enter, space, tab };
        assert_eq!(
            parse_accept_keys("enter, space, tab"),
            Some(AcceptKeys::default())
        );
        assert_eq!(
            parse_accept_keys(r#"["Enter", "TAB"]"#),
            Some(only(true, false, true))
        );
        assert_eq!(parse_accept_keys("tab"), Some(only(false, false, true)));
        assert_eq!(
            parse_accept_keys("space, bogus"),
            Some(only(false, true, false))
        );
        assert_eq!(parse_accept_keys(""), None);
        assert_eq!(parse_accept_keys("[]"), None);
        assert_eq!(parse_accept_keys("escape"), None);

        // Empty or invalid lists fall back to the default
        let mut cfg = Config::default();
        cfg.parse_str("accept_keys = [\"tab\"]\n");
        assert_eq!(cfg.accept_keys, only(false, false, true));
        cfg.parse_str("accept_keys = []\n");
        assert_eq!(cfg.accept_keys, AcceptKeys::default());
        cfg.parse_str("accept_keys = tab\naccept_keys = nonsense\n");
        assert_eq!(cfg.accept_keys, AcceptKeys::default());
    }

    #[test]
    fn test_parse_disabled_apps() {
        let mut cfg = Config::default();
        cfg.parse_str("disabled_apps = Code, org.wezfurlong.wezterm\n");
        assert_eq!(cfg.disabled_apps, ["code", "org.wezfurlong.wezterm"]);

        cfg.parse_str(r#"disabled_apps = ["Steam*", "firefox"]"#);
        assert_eq!(cfg.disabled_apps, ["steam*", "firefox"]);

        // Multi-line TOML array with a comment line
        cfg.parse_str(
            "disabled_apps = [\n  \"code\",\n  # editors\n  \"kitty\",\n]\nlearn = false\n",
        );
        assert_eq!(cfg.disabled_apps, ["code", "kitty"]);
        assert!(!cfg.learn);

        // A missing closing bracket does not swallow the settings that follow
        cfg.parse_str("disabled_apps = [\"code\",\n  \"kitty\"\ntrailing_space = false\n");
        assert_eq!(cfg.disabled_apps, ["code", "kitty"]);
        assert!(!cfg.trailing_space);

        cfg.parse_str("disabled_apps =\n");
        assert!(cfg.disabled_apps.is_empty());
    }

    #[test]
    fn test_app_pattern_glob() {
        assert!(app_pattern_matches("code", "code"));
        assert!(app_pattern_matches("code", "Code"));
        assert!(!app_pattern_matches("code", "code-oss"));
        assert!(app_pattern_matches("steam*", "steam"));
        assert!(app_pattern_matches("Steam*", "steam_app_570"));
        assert!(!app_pattern_matches("steam*", "xsteam"));
        assert!(app_pattern_matches("*", "anything"));

        let mut cfg = Config::default();
        assert!(!cfg.is_disabled_app("code"));
        cfg.parse_str("disabled_apps = code, org.wezfurlong.*\n");
        assert!(cfg.is_disabled_app("Code"));
        assert!(cfg.is_disabled_app("org.wezfurlong.wezterm"));
        assert!(!cfg.is_disabled_app("firefox"));
    }

    #[test]
    fn test_reload_keeps_cli_overrides() {
        let unique_id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_dir = std::env::temp_dir().join(format!("typesuggest_reload_{}", unique_id));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let path = temp_dir.join("config.toml");
        std::fs::write(&path, "learn = true\nmax_candidates = 4\nbar_scale = 1.5\n").unwrap();

        let overrides = CliOverrides {
            learn: Some(false),
            max_candidates: Some(2),
            ..CliOverrides::default()
        };
        let mut source = ConfigSource::new(path.clone(), overrides);
        let cfg = source.load();
        assert!(!cfg.learn);
        assert_eq!(cfg.max_candidates, 2);
        assert_eq!(cfg.bar_scale, 1.5);

        // Unchanged file: nothing to reload
        assert_eq!(source.reload_if_changed(), None);

        std::fs::write(
            &path,
            "learn = true\nmax_candidates = 5\nbar_scale = 2.0\nmin_prefix_length = 3\naccept_keys = tab\n",
        )
        .unwrap();
        // Make sure the change is visible even on filesystems with coarse timestamps
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(SystemTime::now() + std::time::Duration::from_secs(5))
            .unwrap();

        let cfg = source.reload_if_changed().expect("changed file reloads");
        assert!(!cfg.learn, "--no-learn still wins");
        assert_eq!(cfg.max_candidates, 2, "--max-candidates still wins");
        assert_eq!(cfg.bar_scale, 2.0);
        assert_eq!(cfg.min_prefix_length, 3);
        assert!(!cfg.accept_keys.enter && cfg.accept_keys.tab);
        assert_eq!(source.reload_if_changed(), None);

        // A vanished file keeps the current settings
        std::fs::remove_file(&path).unwrap();
        assert_eq!(source.reload_if_changed(), None);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
