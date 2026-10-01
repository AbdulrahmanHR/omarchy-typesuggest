use crate::config::{FileStamp, file_stamp};
use std::collections::HashMap;
use std::path::PathBuf;

/// Minimum WCAG contrast ratio for pill text derived from a theme
const MIN_TEXT_CONTRAST: f32 = 4.5;

/// An sRGB color with straight (non-premultiplied) alpha
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const BLACK: Rgba = Rgba::new(0, 0, 0, 255);
    pub const WHITE: Rgba = Rgba::new(255, 255, 255, 255);

    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub const fn with_alpha(self, a: u8) -> Self {
        Self { a, ..self }
    }

    /// WCAG relative luminance (alpha ignored)
    fn luminance(self) -> f32 {
        let channel = |c: u8| {
            let c = c as f32 / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(self.r) + 0.7152 * channel(self.g) + 0.0722 * channel(self.b)
    }

    /// Blend towards `other` by `amount` (0.0 - 1.0), like Omarchy's mix_color
    fn mix(self, other: Rgba, amount: f32) -> Rgba {
        let lerp = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * amount).round() as u8;
        Rgba::new(
            lerp(self.r, other.r),
            lerp(self.g, other.g),
            lerp(self.b, other.b),
            255,
        )
    }
}

/// WCAG contrast ratio between two colors (1.0 - 21.0)
fn contrast(a: Rgba, b: Rgba) -> f32 {
    let (la, lb) = (a.luminance(), b.luminance());
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// The first candidate that reads well on `bg`, else whichever of them, black or white contrasts most
fn readable_on(bg: Rgba, candidates: &[Rgba]) -> Rgba {
    if let Some(c) = candidates
        .iter()
        .find(|c| contrast(**c, bg) >= MIN_TEXT_CONTRAST)
    {
        return *c;
    }
    candidates
        .iter()
        .copied()
        .chain([Rgba::BLACK, Rgba::WHITE])
        .max_by(|a, b| contrast(*a, bg).total_cmp(&contrast(*b, bg)))
        .unwrap_or(Rgba::WHITE)
}

/// Parse `#rrggbb` or `#rrggbbaa` (case-insensitive); anything else is rejected
pub fn parse_hex_color(val: &str) -> Option<Rgba> {
    let hex = val.trim().strip_prefix('#')?;
    if !(hex.len() == 6 || hex.len() == 8) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    let a = if hex.len() == 8 { byte(6)? } else { 255 };
    Some(Rgba::new(byte(0)?, byte(2)?, byte(4)?, a))
}

/// Where the bar takes its colors from (`theme` in config.toml)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Theme {
    /// Omarchy's current theme palette, falling back to the built-in one
    #[default]
    Omarchy,
    /// Always the built-in dark teal palette
    Default,
}

impl Theme {
    pub fn parse(val: &str) -> Option<Self> {
        match val.trim().to_lowercase().as_str() {
            "omarchy" => Some(Theme::Omarchy),
            "default" | "builtin" | "built-in" => Some(Theme::Default),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Theme::Omarchy => "omarchy",
            Theme::Default => "default",
        }
    }
}

/// Colors of the suggestion bar
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// Bar background
    pub background: Rgba,
    /// Outer bar border
    pub border: Rgba,
    /// Unselected pill background
    pub pill: Rgba,
    /// Unselected pill border
    pub pill_border: Rgba,
    /// Unselected pill text
    pub text: Rgba,
    /// Selected pill background
    pub accent: Rgba,
    /// Selected pill text
    pub accent_text: Rgba,
}

impl Palette {
    /// The original dark teal look, used for `theme = "default"` and whenever Omarchy's palette
    /// is unavailable
    pub const BUILTIN: Palette = Palette {
        // #060f12 at 98% opacity
        background: Rgba::new(6, 15, 18, 250),
        // #1e3338
        border: Rgba::new(30, 51, 56, 255),
        // #0c1a1e
        pill: Rgba::new(12, 26, 30, 230),
        // #1a3238
        pill_border: Rgba::new(26, 50, 56, 255),
        // #d4e8e8 (high contrast, crisp)
        text: Rgba::new(212, 232, 232, 255),
        // #7fc9c4 (cyan)
        accent: Rgba::new(127, 201, 196, 255),
        // #040a0c (rich dark)
        accent_text: Rgba::new(4, 10, 12, 255),
    };

    /// Map an Omarchy colors.toml onto the bar. None when it has no usable background or
    /// foreground; other missing keys are derived from those two.
    pub fn from_omarchy_colors(text: &str) -> Option<Palette> {
        let colors = parse_colors_toml(text);
        let get = |keys: &[&str]| keys.iter().find_map(|k| colors.get(*k).copied());

        let background = get(&["background", "bg", "color0"])?.with_alpha(255);
        let foreground = get(&["foreground", "fg", "color7"])?.with_alpha(255);
        let lighter_background = get(&["lighter_background", "lighter_bg"]);
        let dark_background = get(&["dark_background", "dark_bg"]);

        let border = get(&["muted", "color8"])
            .or(lighter_background)
            .unwrap_or_else(|| background.mix(foreground, 0.25));
        let pill = lighter_background
            .or(dark_background)
            .unwrap_or_else(|| background.mix(foreground, 0.08));
        let pill_border = get(&["selection", "selection_background"])
            .unwrap_or_else(|| background.mix(foreground, 0.15));
        let accent = get(&["accent", "blue", "color4"]).unwrap_or(foreground);

        let bright_foreground = get(&["bright_foreground", "bright_fg"]).unwrap_or(foreground);
        let light_foreground = get(&["light_foreground", "light_fg"]).unwrap_or(foreground);
        let darker_background = get(&["darker_background", "darker_bg"]).unwrap_or(background);

        Some(Palette {
            background: background.with_alpha(250),
            border: border.with_alpha(255),
            pill: pill.with_alpha(230),
            pill_border: pill_border.with_alpha(255),
            text: readable_on(pill, &[foreground, bright_foreground, light_foreground])
                .with_alpha(255),
            accent: accent.with_alpha(255),
            accent_text: readable_on(
                accent,
                &[background, darker_background, foreground, bright_foreground],
            )
            .with_alpha(255),
        })
    }
}

/// `key = "#rrggbb"` lines of an Omarchy colors.toml; values that are not hex colors are skipped
fn parse_colors_toml(text: &str) -> HashMap<String, Rgba> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.starts_with('#') {
                return None;
            }
            let (key, val) = line.split_once('=')?;
            let val = val.trim();
            // Quoted values end at the closing quote, which drops any trailing comment
            let val = match val.strip_prefix(['"', '\'']) {
                Some(rest) => rest.split(['"', '\'']).next()?,
                None => val.split_whitespace().next()?,
            };
            Some((key.trim().to_lowercase(), parse_hex_color(val)?))
        })
        .collect()
}

/// Per-color overrides from config.toml (`color_*`); each one wins over the theme
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ColorOverrides {
    pub background: Option<Rgba>,
    pub border: Option<Rgba>,
    pub pill: Option<Rgba>,
    pub pill_border: Option<Rgba>,
    pub text: Option<Rgba>,
    pub accent: Option<Rgba>,
    pub accent_text: Option<Rgba>,
}

impl ColorOverrides {
    /// The override for a config key such as `color_accent`; None for any other key
    pub fn slot(&mut self, key: &str) -> Option<&mut Option<Rgba>> {
        Some(match key {
            "color_background" => &mut self.background,
            "color_border" => &mut self.border,
            "color_pill" => &mut self.pill,
            "color_pill_border" => &mut self.pill_border,
            "color_text" => &mut self.text,
            "color_accent" => &mut self.accent,
            "color_accent_text" => &mut self.accent_text,
            _ => return None,
        })
    }

    pub fn apply(&self, palette: Palette) -> Palette {
        Palette {
            background: self.background.unwrap_or(palette.background),
            border: self.border.unwrap_or(palette.border),
            pill: self.pill.unwrap_or(palette.pill),
            pill_border: self.pill_border.unwrap_or(palette.pill_border),
            text: self.text.unwrap_or(palette.text),
            accent: self.accent.unwrap_or(palette.accent),
            accent_text: self.accent_text.unwrap_or(palette.accent_text),
        }
    }
}

/// `$XDG_STATE_HOME/omarchy/current/theme/colors.toml`, with `$XDG_STATE_HOME` defaulting to
/// `~/.local/state`
pub fn omarchy_colors_path() -> Option<PathBuf> {
    let state_home = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(state_home.join("omarchy/current/theme/colors.toml"))
}

/// Omarchy's current palette, re-read only when its colors.toml changes on disk
#[derive(Debug, Default)]
pub struct OmarchyTheme {
    path: Option<PathBuf>,
    stamp: Option<FileStamp>,
    checked: bool,
    palette: Option<Palette>,
}

impl OmarchyTheme {
    pub fn new() -> Self {
        Self {
            path: omarchy_colors_path(),
            ..Self::default()
        }
    }

    /// Stat the colors file and re-read it if it changed since the last call. Returns the
    /// palette, or None when Omarchy's theme is missing or unusable.
    pub fn refresh(&mut self) -> Option<Palette> {
        let stamp = self.path.as_deref().and_then(file_stamp);
        if !self.checked || stamp != self.stamp {
            self.checked = true;
            self.stamp = stamp;
            self.palette = self
                .path
                .as_ref()
                .filter(|_| stamp.is_some())
                .and_then(|path| std::fs::read_to_string(path).ok())
                .and_then(|text| Palette::from_omarchy_colors(&text));
        }
        self.palette
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Copy of /usr/share/omarchy/themes/tokyo-night/colors.toml
    const TOKYO_NIGHT: &str = r##"mode = "dark"

accent = "#7aa2f7"
selection = "#292e42"
muted = "#414868"

background = "#1a1b26"
dark_background = "#13141c"
darker_background = "#0e0e14"
lighter_background = "#24283b"

foreground = "#a9b1d6"
dark_foreground = "#565f89"
light_foreground = "#b4bee6"
bright_foreground = "#c0caf5"

red = "#f7768e"
yellow = "#e0af68"
orange = "#eb927b"
green = "#9ece6a"
cyan = "#449dab"
blue = "#7aa2f7"
magenta = "#ad8ee6"
brown = "#75493d"

bright_red = "#ff7a93"
bright_yellow = "#ff9e64"
bright_green = "#b9f27c"
bright_cyan = "#0db9d7"
bright_blue = "#7da6ff"
bright_magenta = "#bb9af7"
"##;

    /// Copy of /usr/share/omarchy/themes/catppuccin-latte/colors.toml (palette keys)
    const CATPPUCCIN_LATTE: &str = r##"mode = "light"
accent = "#1e66f5"
selection = "#ccd0da"
muted = "#acb0be"
background = "#eff1f5"
dark_background = "#e3e4e8"
darker_background = "#d7d8dc"
lighter_background = "#dce0e8"
foreground = "#4c4f69"
dark_foreground = "#6c6f85"
light_foreground = "#5c5f77"
bright_foreground = "#4c4f69"
"##;

    fn hex(val: &str) -> Rgba {
        parse_hex_color(val).unwrap()
    }

    #[test]
    fn test_parse_hex_color() {
        assert_eq!(
            parse_hex_color("#7fc9c4"),
            Some(Rgba::new(127, 201, 196, 255))
        );
        assert_eq!(
            parse_hex_color("#7FC9C4"),
            Some(Rgba::new(127, 201, 196, 255))
        );
        assert_eq!(
            parse_hex_color(" #7fc9c480 "),
            Some(Rgba::new(127, 201, 196, 128))
        );

        for bad in [
            "",
            "#",
            "7fc9c4",
            "#7fc9c",
            "#7fc9c4c",
            "#7fc9c4c4c4",
            "#fff",
            "#gggggg",
            "#7fc9c4é",
            "rgb(1,2,3)",
        ] {
            assert_eq!(parse_hex_color(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn test_omarchy_palette_tokyo_night() {
        let p = Palette::from_omarchy_colors(TOKYO_NIGHT).unwrap();
        assert_eq!(p.background, hex("#1a1b26").with_alpha(250));
        assert_eq!(p.border, hex("#414868"));
        assert_eq!(p.pill, hex("#24283b").with_alpha(230));
        assert_eq!(p.pill_border, hex("#292e42"));
        assert_eq!(p.text, hex("#a9b1d6"));
        assert_eq!(p.accent, hex("#7aa2f7"));
        assert_eq!(p.accent_text, hex("#1a1b26"));
    }

    #[test]
    fn test_omarchy_palette_light_theme_stays_readable() {
        let p = Palette::from_omarchy_colors(CATPPUCCIN_LATTE).unwrap();
        assert_eq!(p.accent, hex("#1e66f5"));
        assert_eq!(p.text, hex("#4c4f69"));
        // The light background is slightly too faint on the blue accent, so a better choice wins
        assert!(contrast(p.accent_text, p.accent) >= contrast(hex("#eff1f5"), p.accent));
        assert!(contrast(p.text, p.pill) >= MIN_TEXT_CONTRAST);
    }

    #[test]
    fn test_omarchy_palette_missing_keys() {
        // Only the essentials: everything else is derived
        let p = Palette::from_omarchy_colors(
            "# comment\nbackground = \"#000000\" # inline\nforeground = '#ffffff'\n",
        )
        .unwrap();
        assert_eq!(p.background, Rgba::new(0, 0, 0, 250));
        assert_eq!(p.accent, Rgba::WHITE);
        assert_eq!(p.accent_text, Rgba::BLACK);
        assert_eq!(p.text, Rgba::WHITE);

        assert_eq!(
            Palette::from_omarchy_colors("foreground = \"#ffffff\""),
            None
        );
        assert_eq!(
            Palette::from_omarchy_colors("background = \"#000000\""),
            None
        );
        assert_eq!(
            Palette::from_omarchy_colors("background = \"black\"\nforeground = \"#fff\""),
            None
        );
        assert_eq!(Palette::from_omarchy_colors(""), None);
    }

    #[test]
    fn test_color_overrides_win_over_theme() {
        let mut overrides = ColorOverrides::default();
        assert_eq!(overrides.apply(Palette::BUILTIN), Palette::BUILTIN);

        *overrides.slot("color_accent").unwrap() = Some(hex("#ff0000"));
        *overrides.slot("color_background").unwrap() = Some(hex("#10203040"));
        assert!(overrides.slot("color_unknown").is_none());

        let p = overrides.apply(Palette::BUILTIN);
        assert_eq!(p.accent, Rgba::new(255, 0, 0, 255));
        assert_eq!(p.background, Rgba::new(16, 32, 48, 64));
        assert_eq!(p.text, Palette::BUILTIN.text);
    }

    #[test]
    fn test_omarchy_theme_follows_file_changes() {
        let unique_id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_dir = std::env::temp_dir().join(format!("typesuggest_theme_{}", unique_id));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let path = temp_dir.join("colors.toml");
        let mut theme = OmarchyTheme {
            path: Some(path.clone()),
            ..OmarchyTheme::default()
        };

        // No Omarchy theme yet
        assert_eq!(theme.refresh(), None);

        std::fs::write(&path, TOKYO_NIGHT).unwrap();
        let tokyo = Palette::from_omarchy_colors(TOKYO_NIGHT);
        assert_eq!(theme.refresh(), tokyo);
        assert_eq!(theme.refresh(), tokyo);

        // Switching themes replaces the file, as omarchy-theme-set does
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, CATPPUCCIN_LATTE).unwrap();
        assert_eq!(
            theme.refresh(),
            Palette::from_omarchy_colors(CATPPUCCIN_LATTE)
        );

        std::fs::remove_file(&path).unwrap();
        assert_eq!(theme.refresh(), None);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_theme_parse() {
        assert_eq!(Theme::parse("Omarchy"), Some(Theme::Omarchy));
        assert_eq!(Theme::parse("default"), Some(Theme::Default));
        assert_eq!(Theme::parse("neon"), None);
    }
}
