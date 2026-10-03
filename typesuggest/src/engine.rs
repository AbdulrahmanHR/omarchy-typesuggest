use crate::config::{BarPosition, Config, ConfigSource};
use crate::dict::{Dictionary, write_user_bigrams_file};
use crate::security::{
    get_hyprland_active_window, get_hyprland_max_monitor_extent, has_sensitive_descendant,
    is_sensitive_window,
};
use crate::shm::{BUFFER_SCALE, draw_pixmap_to_surface, hide_surface};
use crate::state::{InputMode, KeyAction, StateMachine};
use crate::theme::{OmarchyTheme, Palette, Theme};
use crate::ui::{PillRect, Renderer};
use std::os::fd::AsFd;
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_keyboard, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool,
    wl_surface,
};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols::wp::cursor_shape::v1::client::{
    wp_cursor_shape_device_v1::{self, Shape as CursorShape},
    wp_cursor_shape_manager_v1,
};
use wayland_protocols::wp::text_input::zv3::client::zwp_text_input_v3::{
    ContentHint, ContentPurpose,
};
use wayland_protocols_misc::zwp_input_method_v2::client::{
    zwp_input_method_keyboard_grab_v2, zwp_input_method_manager_v2, zwp_input_method_v2,
    zwp_input_popup_surface_v2,
};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1, zwp_virtual_keyboard_v1,
};
use xkbcommon::xkb;

/// Size Hyprland reports in `text_input_rectangle` when the focused app has not sent
/// `zwp_text_input_v3.set_cursor_rectangle` since it was enabled. The popup is then parked
/// 500px below the window's top-left corner (middle-left of the screen) instead of at the caret.
const PLACEHOLDER_CARET_SIZE: (i32, i32) = (500, 500);

/// Placeholder rectangles tolerated per activation before assuming the app never reports
/// its caret and showing the popup at the compositor's fallback position anyway.
const MAX_PLACEHOLDER_DEFERRALS: u8 = 3;

/// Logical height assumed for the tallest monitor when Hyprland cannot be asked
const FALLBACK_MONITOR_EXTENT: u32 = 2160;

/// Tallest buffer to attach: 16384 px is the smallest texture limit common GPUs guarantee
const MAX_BUFFER_HEIGHT: u32 = 16384;

/// Upper bound on key repeats replayed for one held key
const MAX_REPLAYED_REPEATS: u64 = 4096;

/// Linux input event code of the left mouse button, as wl_pointer reports it
const BTN_LEFT: u32 = 0x110;

/// How long after the bar appears or changes its words a click on it is ignored. The second
/// click of a double click, or a click aimed at the text just as the bar pops up under the
/// pointer, would otherwise take a word nobody chose.
const CLICK_GUARD: std::time::Duration = std::time::Duration::from_millis(300);

/// The cursor over the popup: a hand over a suggestion, which a click takes, else an arrow
fn cursor_shape_at(pill: Option<usize>) -> CursorShape {
    if pill.is_some() {
        CursorShape::Pointer
    } else {
        CursorShape::Default
    }
}

/// Whether a click at `now` comes too soon after the bar's words changed at `changed_at`
fn click_too_soon(changed_at: Option<std::time::Instant>, now: std::time::Instant) -> bool {
    changed_at.is_some_and(|at| now.saturating_duration_since(at) < CLICK_GUARD)
}

/// A rectangle on the popup surface, in the surface-local coordinates wl_pointer reports
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl SurfaceRect {
    fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}

/// Where the bar and its pills were drawn on the popup surface
#[derive(Debug, Clone, PartialEq)]
pub struct BarHitAreas {
    pub bar: SurfaceRect,
    /// One per candidate, in candidate order
    pub pills: Vec<SurfaceRect>,
}

impl BarHitAreas {
    /// The bar's pixmap (`width` x `height` pixels, with `pills` from `Renderer::pill_rects`)
    /// drawn `bar_top` rows down the buffer: 0 below the caret, or near the bottom of the tall
    /// transparent buffer above it. The buffer is attached at `BUFFER_SCALE`, so a surface
    /// unit is that many pixels.
    pub fn new(width: u32, height: u32, bar_top: u32, pills: &[PillRect]) -> Self {
        let scale = f64::from(BUFFER_SCALE);
        let top = f64::from(bar_top);
        let to_surface = |x: f32, y: f32, w: f32, h: f32| SurfaceRect {
            x: f64::from(x) / scale,
            y: (top + f64::from(y)) / scale,
            width: f64::from(w) / scale,
            height: f64::from(h) / scale,
        };
        Self {
            bar: to_surface(0.0, 0.0, width as f32, height as f32),
            pills: pills
                .iter()
                .map(|p| to_surface(p.x, p.y, p.width, p.height))
                .collect(),
        }
    }

    /// The candidate whose pill is under the point, if any. The padding and gaps around the
    /// pills belong to none of them.
    pub fn pill_at(&self, x: f64, y: f64) -> Option<usize> {
        self.pills.iter().position(|pill| pill.contains(x, y))
    }

    /// Whether the point is on the bar itself rather than the transparent rest of the popup
    pub fn on_bar(&self, x: f64, y: f64) -> bool {
        self.bar.contains(x, y)
    }
}

/// A forwarded key that may still be held down, so the app may be repeating it
#[derive(Debug, Clone, Copy)]
pub struct HeldKey {
    key: u32,
    /// Compositor timestamp (ms) of the press
    pressed_at: u32,
    keysym: u32,
    ch: Option<char>,
    ctrl: bool,
}

/// Repeats an app generates for a key held `held_ms` with Wayland key repeat at `rate` keys
/// per second after `delay` ms: the first repeat at `delay`, then one every 1000/rate ms
pub fn repeats_while_held(rate: i32, delay: i32, held_ms: u32) -> u64 {
    let (Ok(rate), Ok(delay)) = (u64::try_from(rate), u64::try_from(delay)) else {
        return 0;
    };
    let held_ms = u64::from(held_ms);
    if rate == 0 || held_ms < delay {
        return 0;
    }
    1 + (held_ms - delay) * rate / 1000
}

/// Modifier keys, which apps never repeat
fn is_modifier_keysym(keysym: u32) -> bool {
    (0xffe1..=0xffee).contains(&keysym) || (0xfe01..=0xfe0f).contains(&keysym) || keysym == 0xff7e
}

pub struct Engine {
    pub shm: Option<wl_shm::WlShm>,
    pub compositor: Option<wl_compositor::WlCompositor>,
    pub seat: Option<wl_seat::WlSeat>,
    pub im_manager: Option<zwp_input_method_manager_v2::ZwpInputMethodManagerV2>,
    pub vk_manager: Option<zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1>,

    pub im: Option<zwp_input_method_v2::ZwpInputMethodV2>,
    pub vk: Option<zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1>,
    pub grab: Option<zwp_input_method_keyboard_grab_v2::ZwpInputMethodKeyboardGrabV2>,
    pub popup_surface: Option<wl_surface::WlSurface>,
    pub popup: Option<zwp_input_popup_surface_v2::ZwpInputPopupSurfaceV2>,

    pub xkb_ctx: xkb::Context,
    pub xkb_state: Option<xkb::State>,

    pub dict: Dictionary,
    pub state_machine: StateMachine,
    pub renderer: Renderer,
    pub user_bigrams_path: Option<std::path::PathBuf>,
    /// The learned phrases file has been read into the dictionary (done once learning is on)
    pub user_bigrams_loaded: bool,

    /// Settings currently in effect
    pub config: Config,
    /// config.toml with the command-line overrides, checked for edits on each activation
    pub config_source: Option<ConfigSource>,
    pub omarchy_theme: OmarchyTheme,
    /// The focused window's class is in `disabled_apps`: keys pass straight through
    pub app_disabled: bool,

    pub active: bool,
    pub active_serial: u32,
    pub keymap_sent_to_vk: bool,
    pub swallowed_keys: std::collections::HashSet<u32>,
    pub is_sensitive: bool,
    pub is_sensitive_wayland: bool,
    pub terminal_sudo_pending: bool,
    pub terminal_sudo_started: Option<std::time::Instant>,
    pub active_window_pid: Option<u32>,
    pub last_window_check: std::time::Instant,
    pub is_popup_visible: bool,
    pub cursor_rect: Option<(i32, i32, i32, i32)>,
    /// Popup was hidden because the compositor has no caret rectangle yet; redraw on next done
    pub awaiting_caret_rect: bool,
    pub placeholder_deferrals: u8,
    /// Candidates and selection currently on screen
    pub last_drawn: Option<(Vec<String>, Option<usize>)>,
    /// Last surrounding text (and byte cursor) the app reported; None for apps that never do
    pub surrounding: Option<(String, usize)>,
    /// Content type and surrounding text of the update in progress; applied together on `done`,
    /// content type first, so a password field's text is never looked at
    pub pending_secret: Option<bool>,
    pub pending_surrounding: Option<(String, u32, u32)>,
    /// Another input method holds the seat; the main loop exits
    pub unavailable: bool,
    /// Logical height of the tallest monitor, for `bar_position = "above"`
    pub monitor_extent: u32,
    /// Seat pointer, used to click the pills and to move the bar out of the way when it is
    /// shown above the caret
    pub pointer: Option<wl_pointer::WlPointer>,
    /// Where the bar and its pills are on the popup surface, from the last draw
    pub bar_areas: Option<BarHitAreas>,
    /// Pointer position on the popup surface, from the last enter or motion. Cleared when the
    /// pointer leaves and whenever the popup may have moved since, so a click only counts
    /// where the pointer was seen over the bar as it is now.
    pub pointer_pos: Option<(f64, f64)>,
    /// The suggestion under the pointer, drawn highlighted like a keyboard selection while
    /// `hover_highlight` is on
    pub hover_index: Option<usize>,
    /// Sets the cursor over the popup: an arrow, and a hand over a suggestion. The popup is
    /// our surface, so without this Hyprland shows no cursor at all over it.
    pub cursor_shape_manager: Option<wp_cursor_shape_manager_v1::WpCursorShapeManagerV1>,
    pub cursor_shape_device: Option<wp_cursor_shape_device_v1::WpCursorShapeDeviceV1>,
    /// Serial of the pointer's last enter on the popup, which a cursor change must name, and
    /// the shape set since that enter
    pointer_enter_serial: Option<u32>,
    cursor_shape: Option<CursorShape>,
    /// When the bar last appeared or changed its words (see `CLICK_GUARD`)
    pub bar_changed_at: Option<std::time::Instant>,
    /// Key repeat the compositor tells apps to apply (keys per second, ms before repeating)
    pub repeat_rate: i32,
    pub repeat_delay: i32,
    /// The last forwarded key while it is held down
    pub held_key: Option<HeldKey>,
    /// When the held key starts repeating in the app; the main loop calls on_repeat_deadline
    pub repeat_deadline: Option<std::time::Instant>,
    /// Background writer for the learned phrases, so the disk sync never delays typing
    learned_saver: Option<std::sync::mpsc::Sender<(std::path::PathBuf, String)>>,
}

impl Engine {
    pub fn new(dict: Dictionary) -> Result<Self, Box<dyn std::error::Error>> {
        let xkb_ctx = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let renderer = Renderer::new().map_err(|e| format!("Renderer init failed: {}", e))?;

        Ok(Self {
            shm: None,
            compositor: None,
            seat: None,
            im_manager: None,
            vk_manager: None,
            im: None,
            vk: None,
            grab: None,
            popup_surface: None,
            popup: None,
            xkb_ctx,
            xkb_state: None,
            dict,
            state_machine: StateMachine::new(3),
            renderer,
            user_bigrams_path: None,
            user_bigrams_loaded: false,
            config: Config::default(),
            config_source: None,
            omarchy_theme: OmarchyTheme::new(),
            app_disabled: false,
            active: false,
            active_serial: 0,
            keymap_sent_to_vk: false,
            swallowed_keys: std::collections::HashSet::new(),
            is_sensitive: false,
            is_sensitive_wayland: false,
            terminal_sudo_pending: false,
            terminal_sudo_started: None,
            active_window_pid: None,
            last_window_check: std::time::Instant::now()
                .checked_sub(std::time::Duration::from_secs(10))
                .unwrap_or_else(std::time::Instant::now),
            is_popup_visible: false,
            cursor_rect: None,
            awaiting_caret_rect: false,
            placeholder_deferrals: 0,
            last_drawn: None,
            surrounding: None,
            pending_secret: None,
            pending_surrounding: None,
            unavailable: false,
            learned_saver: None,
            monitor_extent: FALLBACK_MONITOR_EXTENT,
            pointer: None,
            bar_areas: None,
            pointer_pos: None,
            hover_index: None,
            cursor_shape_manager: None,
            cursor_shape_device: None,
            pointer_enter_serial: None,
            cursor_shape: None,
            bar_changed_at: None,
            repeat_rate: 25,
            repeat_delay: 600,
            held_key: None,
            repeat_deadline: None,
        })
    }

    /// Put a (re)loaded config into effect without disturbing the current input state
    pub fn apply_config(&mut self, config: Config) {
        self.state_machine.apply_config(&config);
        self.renderer.bar_scale = config.bar_scale;
        self.dict.set_typo_correction(config.typo_correction);

        // Read the learned phrases before learning starts, or the next save would overwrite
        // the file with only what was learned since
        if config.learn
            && !self.user_bigrams_loaded
            && let Some(path) = &self.user_bigrams_path
        {
            self.dict.load_user_bigrams_file(path);
            self.user_bigrams_loaded = true;
        }
        self.dict.set_learning(config.learn);

        if config.font != self.config.font
            && let Err(e) = self.renderer.set_font(&config.font)
        {
            eprintln!("[typesuggest] Keeping the current font: {}", e);
        }

        self.config = config;
        self.refresh_palette();
        self.last_drawn = None;
    }

    /// Pick up edits to config.toml and Omarchy theme switches. Called on activation only, so
    /// the key path never touches the disk; unchanged files cost a stat each.
    pub fn reload_settings(&mut self) {
        match self
            .config_source
            .as_mut()
            .and_then(ConfigSource::reload_if_changed)
        {
            Some(config) => {
                println!("[typesuggest] Configuration reloaded");
                self.apply_config(config);
            }
            None => self.refresh_palette(),
        }
    }

    /// Rebuild the bar colors from the configured theme and overrides. Omarchy's colors.toml is
    /// only re-read when it changed on disk.
    fn refresh_palette(&mut self) {
        let base = match self.config.theme {
            Theme::Omarchy => self.omarchy_theme.refresh().unwrap_or(Palette::BUILTIN),
            Theme::Default => Palette::BUILTIN,
        };
        let palette = self.config.colors.apply(base);
        if self.renderer.palette != palette {
            self.renderer.palette = palette;
            self.last_drawn = None;
        }
    }

    pub fn set_app_disabled(&mut self, disabled: bool) {
        if self.app_disabled != disabled {
            self.app_disabled = disabled;
            if disabled {
                self.hide();
                self.state_machine.reset();
                self.state_machine.invalidate_suggestions();
                self.surrounding = None;
            }
        }
    }

    pub fn refresh_sensitivity(&mut self) -> bool {
        // 1. Wayland ContentType (GUI passwords, PINs, hidden text)
        if self.is_sensitive_wayland {
            self.set_sensitive(true);
            return true;
        }

        // 2. Terminal sudo / auth pending state
        if self.terminal_sudo_pending {
            if let Some(started) = self.terminal_sudo_started
                && started.elapsed() > std::time::Duration::from_secs(60)
            {
                self.terminal_sudo_pending = false;
                self.terminal_sudo_started = None;
            } else {
                self.set_sensitive(true);
                return true;
            }
        }

        // 3. Query Hyprland active window & inspect process tree
        // Rate-limit IPC checks to avoid excessive socket traffic (every 40ms)
        let now = std::time::Instant::now();
        if now.duration_since(self.last_window_check) >= std::time::Duration::from_millis(40) {
            self.last_window_check = now;
            if let Some(win) = get_hyprland_active_window() {
                self.active_window_pid = Some(win.pid);
                // Focus can move between windows without a new activation
                self.set_app_disabled(self.config.is_disabled_app(&win.class));

                // Layer 3: Title & Class check
                if is_sensitive_window(&win.title, &win.class) {
                    self.set_sensitive(true);
                    return true;
                }

                // Layer 1: Descendant process tree check
                if has_sensitive_descendant(win.pid) {
                    self.set_sensitive(true);
                    return true;
                }
            } else if let Some(pid) = self.active_window_pid
                && has_sensitive_descendant(pid)
            {
                self.set_sensitive(true);
                return true;
            }
        } else if self.is_sensitive {
            return true;
        }

        self.set_sensitive(false);
        false
    }

    pub fn set_sensitive(&mut self, sensitive: bool) {
        if self.is_sensitive != sensitive {
            self.is_sensitive = sensitive;
            if sensitive {
                self.hide();
                self.state_machine.reset();
                self.state_machine.invalidate_suggestions();
                self.surrounding = None;
                self.swallowed_keys.clear();
            }
        }
    }

    pub fn render_and_show(
        &mut self,
        qh: &QueueHandle<Self>,
        candidates: &[String],
        selected_index: Option<usize>,
    ) {
        // GUI apps echo each keystroke back as surrounding text; the compositor moves the bar
        // with the caret on its own, so an identical redraw would only cost time.
        if self.is_popup_visible
            && self
                .last_drawn
                .as_ref()
                .is_some_and(|(c, s)| c == candidates && *s == selected_index)
        {
            return;
        }

        if let (Some(surface), Some(shm)) = (&self.popup_surface, &self.shm)
            && let Some(pixmap) = self.renderer.render_bar(candidates, selected_index)
        {
            let buffer_height = match self.config.bar_position {
                BarPosition::Below => pixmap.height(),
                // Hyprland places a popup that does not fit below the caret above it instead. A
                // transparent surface taller than any monitor triggers that every time, and the
                // bar drawn at its bottom edge then sits right above the text.
                BarPosition::Above => (self.monitor_extent * 2 + pixmap.height())
                    .next_multiple_of(2)
                    .min(MAX_BUFFER_HEIGHT),
            };
            match draw_pixmap_to_surface(surface, shm, qh, &pixmap, buffer_height) {
                Err(e) => eprintln!("Failed to draw pixmap: {}", e),
                Ok(bar_top) => {
                    // Moving the highlight keeps the words where they are; anything else puts
                    // new words under the pointer
                    let words_changed = !self.is_popup_visible
                        || self
                            .last_drawn
                            .as_ref()
                            .is_none_or(|(drawn, _)| drawn != candidates);
                    if words_changed {
                        self.bar_changed_at = Some(std::time::Instant::now());
                    }
                    self.is_popup_visible = true;
                    self.last_drawn = Some((candidates.to_vec(), selected_index));
                    let pills = self.renderer.pill_rects(candidates);
                    let areas = BarHitAreas::new(pixmap.width(), pixmap.height(), bar_top, &pills);
                    // A popup of another size may be placed elsewhere, and Hyprland tells a
                    // pointer that stays still nothing about it
                    if self.bar_areas.as_ref().map(|a| a.bar) != Some(areas.bar) {
                        self.forget_pointer();
                    }
                    self.bar_areas = Some(areas);
                }
            }
        }
    }

    pub fn hide(&mut self) {
        // The bar may show up somewhere else next time, under a pointer that never moved
        self.forget_pointer();
        if self.is_popup_visible
            && let Some(surface) = &self.popup_surface
        {
            hide_surface(surface);
            self.is_popup_visible = false;
        }
    }

    /// Re-show the suggestions the state machine currently holds, if any
    pub fn redraw_current(&mut self, qh: &QueueHandle<Self>) {
        let (candidates, selected) = match &self.state_machine.mode {
            // Before the keyboard enters the bar, only the mouse can highlight a suggestion
            InputMode::Suggesting { candidates, .. } => (
                candidates.clone(),
                self.hover_index.filter(|&i| i < candidates.len()),
            ),
            InputMode::Navigating {
                candidates,
                selected_index,
                ..
            } => (candidates.clone(), Some(*selected_index)),
            InputMode::Idle => return,
        };
        self.render_and_show(qh, &candidates, selected);
    }

    /// Act on the text around the caret that the app reported
    fn apply_surrounding_text(
        &mut self,
        qh: &QueueHandle<Self>,
        text: String,
        cursor: u32,
        anchor: u32,
    ) {
        // Never keep the contents of password / sensitive fields (or disabled apps) around
        let tracking = self.active && !self.is_sensitive && !self.app_disabled;
        if !tracking {
            self.surrounding = None;
            return;
        }

        let action = self.state_machine.handle_surrounding_text(
            &text,
            cursor as usize,
            anchor as usize,
            &self.dict,
        );
        self.surrounding = Some((text, cursor as usize));
        match action {
            KeyAction::ShowSuggestions { candidates, .. } => {
                self.render_and_show(qh, &candidates, None);
            }
            KeyAction::HideSuggestions => {
                self.hide();
            }
            KeyAction::UpdateSelection { index } => {
                if let InputMode::Navigating { candidates, .. } = &self.state_machine.mode {
                    let c = candidates.clone();
                    self.render_and_show(qh, &c, Some(index));
                }
            }
            _ => {
                // A hidden bar must not keep a selection that Enter could still commit
                self.state_machine.mode = InputMode::Idle;
                self.hide();
            }
        }
    }

    /// The held key started repeating in the app: the shown word is going stale, so hide the
    /// bar until the key is released
    pub fn on_repeat_deadline(&mut self) {
        self.repeat_deadline = None;
        if self.held_key.is_some() {
            self.state_machine.mode = InputMode::Idle;
            self.hide();
        }
    }

    /// Apps repeat a held key themselves, so typesuggest sees a single press. Replay the repeats
    /// the app produced between the press and `until` (compositor timestamps), so the
    /// remembered line matches what the app did. Apps that report surrounding text resync on
    /// their own and are left to that.
    fn apply_key_repeats(&mut self, held: HeldKey, until: u32, qh: &QueueHandle<Self>) {
        let held_ms = until.wrapping_sub(held.pressed_at);
        let repeats = repeats_while_held(self.repeat_rate, self.repeat_delay, held_ms)
            .min(MAX_REPLAYED_REPEATS);
        if repeats == 0
            || !self.active
            || self.is_sensitive
            || self.app_disabled
            || self.surrounding.is_some()
        {
            return;
        }
        for _ in 0..repeats {
            let _ =
                self.state_machine
                    .handle_key_press(held.keysym, held.ch, held.ctrl, &self.dict);
        }
        match self.state_machine.mode {
            InputMode::Idle => self.hide(),
            _ => self.redraw_current(qh),
        }
    }

    /// Type a chosen suggestion over the word at the caret, as the state machine's
    /// `KeyAction::CommitCandidate` describes, then learn the phrase. Accept keys and clicks on
    /// the bar both end here; `time` stamps the Backspace and Delete keys sent to terminals.
    fn commit_candidate(&mut self, commit: KeyAction, time: u32, conn: &Connection) {
        let KeyAction::CommitCandidate {
            deleted_before,
            deleted_after,
            replacement,
            prev_word,
            chosen_word,
        } = commit
        else {
            return;
        };

        // Committed text is typed into the app, terminals included: never
        // let a control character through (e.g. a newline would run a command)
        if replacement.chars().any(char::is_control) {
            self.hide();
            return;
        }

        // 1. Remove the partial word. Apps that report surrounding text (GTK,
        // Qt, Chromium) get delete_surrounding_text, applied atomically with
        // the commit. Simulated Backspaces race the commit there: GTK and Qt
        // queue key events but insert committed text at once, so the new word
        // landed first and lost its tail ("hel" -> "helhe"). Only use it when
        // the app's last reported text matches what we are about to delete.
        let app_text_matches = self.surrounding.as_ref().is_some_and(|(text, cursor)| {
            text.get(..*cursor)
                .is_some_and(|before| before.ends_with(&deleted_before))
                && text
                    .get(*cursor..)
                    .is_some_and(|after| after.starts_with(&deleted_after))
        });
        if let Some(im) = &self.im
            && app_text_matches
        {
            im.delete_surrounding_text(deleted_before.len() as u32, deleted_after.len() as u32);
        } else if let Some(vk) = &self.vk {
            // Terminals: Backspace (evdev 14) and Delete (evdev 111)
            for _ in deleted_before.chars() {
                vk.key(time, 14, 1);
                vk.key(time, 14, 0);
            }
            for _ in deleted_after.chars() {
                vk.key(time, 111, 1);
                vk.key(time, 111, 0);
            }
        }

        // 2. Commit replacement string via InputMethod
        if let Some(im) = &self.im {
            im.commit_string(replacement);
            im.commit(self.active_serial);
        }

        let _ = conn.flush();
        self.hide();

        // 3. Learn the phrase (dictionary words only) and save it in the
        // background, after the text is on its way
        if self.dict.is_learning_enabled()
            && let Some(pw) = &prev_word
        {
            self.dict.record_user_bigram(pw, &chosen_word);
            self.state_machine.invalidate_suggestions();
            self.save_learned_phrases();
        }
    }

    /// Forget where the pointer is, and so which suggestion it hovers. Returns whether a
    /// hover highlight was showing, which then needs redrawing away.
    fn forget_pointer(&mut self) -> bool {
        self.pointer_pos = None;
        self.hover_index.take().is_some()
    }

    /// Bind the cursor shape device once both the shape manager and the pointer exist; the
    /// compositor may announce them in either order
    fn ensure_cursor_shape_device(&mut self, qh: &QueueHandle<Self>) {
        if self.cursor_shape_device.is_none()
            && let (Some(manager), Some(pointer)) = (&self.cursor_shape_manager, &self.pointer)
        {
            self.cursor_shape_device = Some(manager.get_pointer(pointer, qh, ()));
        }
    }

    /// Show `shape` as the cursor while it is over the popup. Needs the serial of the
    /// pointer's enter, so it does nothing before the first one.
    fn set_cursor_shape(&mut self, shape: CursorShape) {
        if self.cursor_shape == Some(shape) {
            return;
        }
        if let (Some(device), Some(serial)) = (&self.cursor_shape_device, self.pointer_enter_serial)
        {
            device.set_shape(serial, shape);
            self.cursor_shape = Some(shape);
        }
    }

    /// Highlight the suggestion at `index` (or none) because the mouse is over it. While the
    /// keyboard is in the bar the selection follows the mouse too, so an accept key takes the
    /// word that is highlighted; before that the highlight is only drawn, and keys type as usual.
    fn set_hover(&mut self, index: Option<usize>, qh: &QueueHandle<Self>) {
        if index == self.hover_index {
            return;
        }
        self.hover_index = index;
        if let Some(index) = index {
            self.state_machine.select_while_navigating(index);
        }
        self.redraw_current(qh);
    }

    /// The pointer entered the popup or moved over it
    fn pointer_moved(&mut self, x: f64, y: f64, qh: &QueueHandle<Self>) {
        self.pointer_pos = Some((x, y));
        let (on_bar, pill) = match self.bar_areas.as_ref().filter(|_| self.is_popup_visible) {
            Some(areas) => (areas.on_bar(x, y), areas.pill_at(x, y)),
            None => (false, None),
        };
        self.set_cursor_shape(cursor_shape_at(pill));
        // Above the caret, the popup's transparent part covers the text above it and takes the
        // mouse there. With mouse_hides_bar the bar gets out of the way as soon as the mouse
        // moves over that part; otherwise it stays, and a click there only hides it.
        if self.config.bar_position == BarPosition::Above && self.config.mouse_hides_bar && !on_bar
        {
            self.state_machine.mode = InputMode::Idle;
            self.hide();
            return;
        }
        let hover = if self.config.hover_highlight {
            pill
        } else {
            None
        };
        self.set_hover(hover, qh);
    }

    /// A left button press on the popup. On a suggestion it takes that word; on the
    /// transparent part above the bar (bar_position = "above") it hides the bar, since the
    /// click was meant for the text under it, which can then be clicked again.
    fn press_popup(&mut self, time: u32, conn: &Connection) {
        let position = self.pointer_pos;
        let areas = self.bar_areas.as_ref().filter(|_| self.is_popup_visible);
        let on_bar = matches!((position, areas), (Some((x, y)), Some(a)) if a.on_bar(x, y));
        if on_bar {
            self.click_bar(time, conn);
        } else if self.is_popup_visible && self.config.bar_position == BarPosition::Above {
            // Unknown position (no motion since the bar moved) counts as off the bar: hiding
            // costs nothing, while keeping the bar would swallow the click again
            self.state_machine.mode = InputMode::Idle;
            self.hide();
        }
    }

    /// Ctrl, Alt or Super is held down
    fn shortcut_modifier_held(&self) -> bool {
        self.xkb_state.as_ref().is_some_and(|xkb_state| {
            [xkb::MOD_NAME_CTRL, xkb::MOD_NAME_ALT, xkb::MOD_NAME_LOGO]
                .into_iter()
                .any(|m| xkb_state.mod_name_is_active(m, xkb::STATE_MODS_EFFECTIVE))
        })
    }

    /// A left click on the popup commits the suggestion under the pointer, exactly as an accept
    /// key commits the highlighted one. Clicks beside the pills do nothing. `time` is the
    /// button event's timestamp.
    fn click_bar(&mut self, time: u32, conn: &Connection) {
        let Some((x, y)) = self.pointer_pos else {
            return;
        };
        let Some(index) = self.bar_areas.as_ref().and_then(|a| a.pill_at(x, y)) else {
            return;
        };
        if !self.active || !self.is_popup_visible || self.app_disabled || self.is_sensitive {
            return;
        }
        if click_too_soon(self.bar_changed_at, std::time::Instant::now()) {
            return;
        }
        // The bar is already hidden in password fields, sudo prompts and disabled apps, but look
        // again before typing anything: focus may have moved without a key being pressed
        if self.refresh_sensitivity() || self.app_disabled || !self.is_popup_visible {
            return;
        }
        // Ctrl, Alt or Super would turn the Backspaces sent to terminals into word deletions,
        // and a key still held down keeps repeating in the app around the committed word
        if self.shortcut_modifier_held() || self.held_key.is_some() {
            return;
        }
        // Only the word drawn on the pill may be committed
        let shown = match &self.state_machine.mode {
            InputMode::Suggesting { candidates, .. } | InputMode::Navigating { candidates, .. } => {
                candidates
            }
            InputMode::Idle => return,
        };
        if self
            .last_drawn
            .as_ref()
            .is_none_or(|(drawn, _)| drawn != shown)
        {
            return;
        }
        if let Some(commit) = self.state_machine.commit_candidate(index) {
            self.commit_candidate(commit, time, conn);
        }
    }

    /// Drop everything typed in the field that just lost focus
    fn forget_typed_text(&mut self) {
        self.state_machine.reset();
        self.state_machine.invalidate_suggestions();
        self.surrounding = None;
        self.pending_surrounding = None;
        self.last_drawn = None;
    }

    /// Write the learned phrases on a background thread; only the newest snapshot is written
    fn save_learned_phrases(&mut self) {
        let Some(path) = self.user_bigrams_path.clone() else {
            return;
        };
        let content = self.dict.user_bigrams_tsv();
        let saver = self.learned_saver.get_or_insert_with(|| {
            let (tx, rx) = std::sync::mpsc::channel::<(std::path::PathBuf, String)>();
            std::thread::spawn(move || {
                while let Ok(mut job) = rx.recv() {
                    while let Ok(newer) = rx.try_recv() {
                        job = newer;
                    }
                    write_user_bigrams_file(&job.0, &job.1);
                }
            });
            tx
        });
        let _ = saver.send((path, content));
    }
}

/// Password, PIN, hidden-text and sensitive-data fields. Hint bits this build does not know
/// (newer protocol versions) must not hide the ones it does.
fn is_secret_content(hint: WEnum<ContentHint>, purpose: WEnum<ContentPurpose>) -> bool {
    let secret_hints = (ContentHint::HiddenText | ContentHint::SensitiveData).bits();
    let hint_bits = match hint {
        WEnum::Value(h) => h.bits(),
        WEnum::Unknown(raw) => raw,
    };
    matches!(
        purpose,
        WEnum::Value(ContentPurpose::Password | ContentPurpose::Pin)
    ) || hint_bits & secret_hints != 0
}

// Registry Dispatch
impl Dispatch<wl_registry::WlRegistry, ()> for Engine {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_shm" => {
                    state.shm = Some(registry.bind(name, version.min(1), qh, ()));
                }
                "wl_compositor" => {
                    state.compositor = Some(registry.bind(name, version.min(4), qh, ()));
                }
                "wl_seat" => {
                    if state.seat.is_none() {
                        state.seat = Some(registry.bind(name, version.min(2), qh, ()));
                    }
                }
                "zwp_input_method_manager_v2" => {
                    state.im_manager = Some(registry.bind(name, version.min(1), qh, ()));
                }
                "zwp_virtual_keyboard_manager_v1" => {
                    state.vk_manager = Some(registry.bind(name, version.min(1), qh, ()));
                }
                "wp_cursor_shape_manager_v1" => {
                    state.cursor_shape_manager = Some(registry.bind(name, version.min(1), qh, ()));
                    state.ensure_cursor_shape_device(qh);
                }
                _ => {}
            }
        }
    }
}

// Compositor & Surface Dispatches
impl Dispatch<wl_compositor::WlCompositor, ()> for Engine {
    fn event(
        _: &mut Self,
        _: &wl_compositor::WlCompositor,
        _: wl_compositor::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_surface::WlSurface, ()> for Engine {
    fn event(
        _: &mut Self,
        _: &wl_surface::WlSurface,
        _: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

// Seat Dispatch
impl Dispatch<wl_seat::WlSeat, ()> for Engine {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
            && caps.contains(wl_seat::Capability::Pointer)
            && state.pointer.is_none()
        {
            state.pointer = Some(seat.get_pointer(qh, ()));
            state.ensure_cursor_shape_device(qh);
        }
    }
}

// Pointer Dispatch: events only arrive while the pointer is over our own popup
impl Dispatch<wl_pointer::WlPointer, ()> for Engine {
    fn event(
        state: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter {
                serial,
                surface,
                surface_x,
                surface_y,
            } if state.popup_surface.as_ref() == Some(&surface) => {
                // Each enter needs its own cursor, named by this serial
                state.pointer_enter_serial = Some(serial);
                state.cursor_shape = None;
                state.pointer_moved(surface_x, surface_y, qh);
            }
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                state.pointer_moved(surface_x, surface_y, qh);
            }
            wl_pointer::Event::Leave { .. } => {
                state.pointer_enter_serial = None;
                state.cursor_shape = None;
                if state.forget_pointer() {
                    state.redraw_current(qh);
                }
            }
            // Take the word on the press. Hyprland keeps the pointer on the popup while the
            // button is held, so the release comes here too, and is ignored.
            wl_pointer::Event::Button {
                time,
                button: BTN_LEFT,
                state: WEnum::Value(wl_pointer::ButtonState::Pressed),
                ..
            } => {
                state.press_popup(time, conn);
            }
            _ => {}
        }
    }
}

// Cursor shape: requests only, the compositor sends no events
impl Dispatch<wp_cursor_shape_manager_v1::WpCursorShapeManagerV1, ()> for Engine {
    fn event(
        _: &mut Self,
        _: &wp_cursor_shape_manager_v1::WpCursorShapeManagerV1,
        _: wp_cursor_shape_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wp_cursor_shape_device_v1::WpCursorShapeDeviceV1, ()> for Engine {
    fn event(
        _: &mut Self,
        _: &wp_cursor_shape_device_v1::WpCursorShapeDeviceV1,
        _: wp_cursor_shape_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

// SHM Dispatches
impl Dispatch<wl_shm::WlShm, ()> for Engine {
    fn event(
        _: &mut Self,
        _: &wl_shm::WlShm,
        _: wl_shm::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_shm_pool::WlShmPool, ()> for Engine {
    fn event(
        _: &mut Self,
        _: &wl_shm_pool::WlShmPool,
        _: wl_shm_pool::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_buffer::WlBuffer, ()> for Engine {
    fn event(
        _: &mut Self,
        buffer: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            buffer.destroy();
        }
    }
}

// Virtual Keyboard Dispatches
impl Dispatch<zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1, ()> for Engine {
    fn event(
        _: &mut Self,
        _: &zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
        _: zwp_virtual_keyboard_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1, ()> for Engine {
    fn event(
        _: &mut Self,
        _: &zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
        _: zwp_virtual_keyboard_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

// Input Method Manager & Popup Surface Dispatches
impl Dispatch<zwp_input_method_manager_v2::ZwpInputMethodManagerV2, ()> for Engine {
    fn event(
        _: &mut Self,
        _: &zwp_input_method_manager_v2::ZwpInputMethodManagerV2,
        _: zwp_input_method_manager_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwp_input_popup_surface_v2::ZwpInputPopupSurfaceV2, ()> for Engine {
    fn event(
        state: &mut Self,
        _: &zwp_input_popup_surface_v2::ZwpInputPopupSurfaceV2,
        event: zwp_input_popup_surface_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_input_popup_surface_v2::Event::TextInputRectangle {
            x,
            y,
            width,
            height,
        } = event
        {
            state.cursor_rect = Some((x, y, width, height));

            // Hyprland's no-caret placeholder: hide instead of showing the bar in the middle of
            // the window, and redraw once the app commits its caret rectangle (next done).
            if (width, height) == PLACEHOLDER_CARET_SIZE
                && state.placeholder_deferrals < MAX_PLACEHOLDER_DEFERRALS
            {
                state.placeholder_deferrals += 1;
                state.awaiting_caret_rect = true;
                state.hide();
            } else {
                state.awaiting_caret_rect = false;
            }
        }
    }
}

// Input Method V2 Dispatch
impl Dispatch<zwp_input_method_v2::ZwpInputMethodV2, ()> for Engine {
    fn event(
        state: &mut Self,
        _: &zwp_input_method_v2::ZwpInputMethodV2,
        event: zwp_input_method_v2::Event,
        _: &(),
        conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            zwp_input_method_v2::Event::ContentType { hint, purpose } => {
                // Applied on done, before the surrounding text of the same update
                state.pending_secret = Some(is_secret_content(hint, purpose));
            }
            zwp_input_method_v2::Event::Activate => {
                state.active = true;
                state.cursor_rect = None;
                state.surrounding = None;
                state.pending_secret = None;
                state.pending_surrounding = None;
                state.held_key = None;
                state.repeat_deadline = None;
                state.awaiting_caret_rect = false;
                state.placeholder_deferrals = 0;
                state.state_machine.reset();
                state.swallowed_keys.clear();
                state.hide();
                // Config edits and theme switches apply from the next focused text field
                state.reload_settings();
                if state.config.bar_position == BarPosition::Above {
                    state.monitor_extent =
                        get_hyprland_max_monitor_extent().unwrap_or(FALLBACK_MONITOR_EXTENT);
                }
                let mut disabled = false;
                if let Some(win) = get_hyprland_active_window() {
                    state.active_window_pid = Some(win.pid);
                    disabled = state.config.is_disabled_app(&win.class);
                }
                state.set_app_disabled(disabled);
                state.refresh_sensitivity();
            }
            zwp_input_method_v2::Event::Deactivate => {
                state.active = false;
                state.held_key = None;
                state.repeat_deadline = None;
                state.cursor_rect = None;
                state.pending_secret = None;
                state.awaiting_caret_rect = false;
                state.is_sensitive_wayland = false;
                state.terminal_sudo_pending = false;
                state.terminal_sudo_started = None;
                state.set_sensitive(false);
                state.forget_typed_text();
                state.swallowed_keys.clear();
                state.hide();
            }

            zwp_input_method_v2::Event::SurroundingText {
                text,
                cursor,
                anchor,
            } => {
                // Applied on done, after any content type of the same update
                state.pending_surrounding = Some((text, cursor, anchor));
            }
            zwp_input_method_v2::Event::Done => {
                state.active_serial = state.active_serial.wrapping_add(1);

                // Hyprland places the popup again at the app's caret before every done, and
                // sends no pointer event when that slides it under a pointer that stays still.
                // A click then needs a fresh position, or it could take the wrong pill.
                if state.forget_pointer() {
                    state.redraw_current(qh);
                }

                // The update is complete: whether the field is secret first, then its text
                if let Some(secret) = state.pending_secret.take() {
                    state.is_sensitive_wayland = secret;
                    state.refresh_sensitivity();
                }
                if let Some((text, cursor, anchor)) = state.pending_surrounding.take() {
                    state.apply_surrounding_text(qh, text, cursor, anchor);
                }

                // Disabled apps are left alone entirely, as if no input method were running
                if state.active && !state.app_disabled {
                    // Acknowledge every done. Hyprland only sends zwp_text_input_v3.done to the app
                    // when the IME commits, and Chromium/Electron hold back set_cursor_rectangle and
                    // set_surrounding_text until a done arrives whose serial matches their commit
                    // count. Without this ack their caret updates lag a keystroke behind, and the
                    // popup gets placed against a stale or missing caret rectangle.
                    if let Some(im) = &state.im {
                        im.commit(state.active_serial);
                    }
                    let _ = conn.flush();

                    if state.awaiting_caret_rect && !state.is_popup_visible && !state.is_sensitive {
                        state.redraw_current(qh);
                    }
                }
            }
            zwp_input_method_v2::Event::Unavailable => {
                eprintln!(
                    "[typesuggest] Error: Input method unavailable. Ensure another IME (such as Fcitx5) is not active on this seat."
                );
                state.unavailable = true;
            }
            _ => {}
        }
    }
}

// Keyboard Grab Dispatch
impl Dispatch<zwp_input_method_keyboard_grab_v2::ZwpInputMethodKeyboardGrabV2, ()> for Engine {
    fn event(
        state: &mut Self,
        _: &zwp_input_method_keyboard_grab_v2::ZwpInputMethodKeyboardGrabV2,
        event: zwp_input_method_keyboard_grab_v2::Event,
        _: &(),
        conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            zwp_input_method_keyboard_grab_v2::Event::Keymap { format, fd, size } => {
                let vkb_fd = if state.vk.is_some() {
                    fd.as_fd().try_clone_to_owned().ok()
                } else {
                    None
                };

                if matches!(format, WEnum::Value(wl_keyboard::KeymapFormat::XkbV1)) {
                    match unsafe {
                        xkb::Keymap::new_from_fd(
                            &state.xkb_ctx,
                            fd,
                            size as usize,
                            xkb::KEYMAP_FORMAT_TEXT_V1,
                            xkb::KEYMAP_COMPILE_NO_FLAGS,
                        )
                    } {
                        Ok(Some(keymap)) => {
                            state.xkb_state = Some(xkb::State::new(&keymap));
                        }
                        _ => {
                            eprintln!("[typesuggest] Failed to parse compositor XKB keymap");
                        }
                    }
                }

                if let (Some(vk), Some(vkb_fd)) = (&state.vk, vkb_fd) {
                    vk.keymap(1, vkb_fd.as_fd(), size);
                    state.keymap_sent_to_vk = true;
                }
            }

            zwp_input_method_keyboard_grab_v2::Event::Modifiers {
                serial: _,
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
            } => {
                if let Some(xkb_state) = &mut state.xkb_state {
                    xkb_state.update_mask(mods_depressed, mods_latched, mods_locked, 0, 0, group);
                }

                if let Some(vk) = &state.vk
                    && state.keymap_sent_to_vk
                {
                    vk.modifiers(mods_depressed, mods_latched, mods_locked, group);
                }
            }

            zwp_input_method_keyboard_grab_v2::Event::Key {
                serial: _,
                time,
                key,
                state: key_state,
            } => {
                let is_pressed = matches!(key_state, WEnum::Value(wl_keyboard::KeyState::Pressed));

                if is_pressed {
                    // Forward the keystroke exactly once, from whichever step decides it
                    let mut forwarded = false;
                    macro_rules! forward {
                        () => {
                            if !forwarded {
                                // Not read again on paths that return right after forwarding
                                #[allow(unused_assignments)]
                                {
                                    forwarded = true;
                                }
                                if let Some(vk) = &state.vk {
                                    vk.key(time, key, 1);
                                }
                            }
                        };
                    }

                    // A new key ends the previous key's repeat in the app; catch up on what it did
                    if let Some(held) = state.held_key.take() {
                        state.repeat_deadline = None;
                        state.apply_key_repeats(held, time, qh);
                    }

                    // The grab receives every key, even when the focused app has no enabled
                    // text input (XWayland, games, Qt with QT_IM_MODULE=fcitx). Our commits are
                    // dropped there, so only forward: acting on suggestions would swallow
                    // Up/Enter/Space/Tab and inject Backspaces into the app.
                    if !state.active {
                        forward!();
                        return;
                    }

                    let mut keysym_raw = 0u32;
                    let mut char_opt = None;
                    let mut ctrl_active = false;
                    let mut alt_or_super = false;

                    if let Some(xkb_state) = &state.xkb_state {
                        let keycode = xkb::Keycode::new(key + 8);
                        keysym_raw = xkb_state.key_get_one_sym(keycode).raw();
                        let utf8 = xkb_state.key_get_utf8(keycode);
                        if !utf8.is_empty() {
                            char_opt = utf8.chars().next();
                        }
                        let active = |m| xkb_state.mod_name_is_active(m, xkb::STATE_MODS_EFFECTIVE);
                        ctrl_active = active(xkb::MOD_NAME_CTRL);
                        alt_or_super = active(xkb::MOD_NAME_ALT) || active(xkb::MOD_NAME_LOGO);
                    }

                    // While the bar is held back waiting for a caret rectangle the user cannot see
                    // it, so Up/Enter must not navigate or commit a hidden candidate.
                    if state.awaiting_caret_rect && !state.is_popup_visible {
                        state.state_machine.mode = InputMode::Idle;
                    }

                    // Keys that cannot be swallowed go to the app before any slower work (window
                    // and process checks, dictionary lookups, drawing), so typing never waits on it
                    if !state.state_machine.may_swallow(keysym_raw, ctrl_active) {
                        forward!();
                        let _ = conn.flush();
                    }

                    // 1. Refresh sensitivity from Wayland ContentType, Hyprland window, and /proc
                    // (the rate-limited window query also notices focus moving to a disabled app)
                    state.refresh_sensitivity();

                    // Apps in disabled_apps get every key untouched, just like the inactive path
                    if state.app_disabled {
                        forward!();
                        return;
                    }

                    // 2. If sensitive (password/PIN/sudo prompt), forward raw key and do not touch buffer or suggestions
                    if state.is_sensitive {
                        state.hide();
                        if keysym_raw == 0xff0d || keysym_raw == 0xff8d {
                            // User pressed Enter (submitting password). Clear pending flag and trigger re-check
                            state.terminal_sudo_pending = false;
                            state.terminal_sudo_started = None;
                            state.last_window_check = std::time::Instant::now()
                                .checked_sub(std::time::Duration::from_secs(1))
                                .unwrap_or_else(std::time::Instant::now);
                        } else if (ctrl_active
                            && (keysym_raw == 0x0063
                                || keysym_raw == 0x0043
                                || keysym_raw == 0x0064
                                || keysym_raw == 0x0044))
                            || keysym_raw == 0xff1b
                        {
                            // Ctrl+C, Ctrl+D, or Escape (user cancelled prompt). A password field
                            // stays sensitive; only the terminal prompt tracking ends.
                            state.terminal_sudo_pending = false;
                            state.terminal_sudo_started = None;
                            state.set_sensitive(state.is_sensitive_wayland);
                        }

                        forward!();
                        return;
                    }

                    // Alt / Super shortcuts (word jumps, window manager keys) move the caret in
                    // ways the buffer cannot follow: forget the word and let the key through
                    if alt_or_super {
                        state.state_machine.reset();
                        state.hide();
                        forward!();
                        return;
                    }

                    let action = state.state_machine.handle_key_press(
                        keysym_raw,
                        char_opt,
                        ctrl_active,
                        &state.dict,
                    );

                    // If Enter was pressed and not committing a candidate, unconditionally dismiss suggestions
                    if (keysym_raw == 0xff0d || keysym_raw == 0xff8d)
                        && !matches!(action, KeyAction::CommitCandidate { .. })
                    {
                        state.hide();
                    }

                    match action {
                        KeyAction::PassThrough => {
                            forward!();
                        }

                        // Typed-command detection is for terminals; apps that report surrounding
                        // text are GUI fields, where "su..." or "pass..." is ordinary text
                        KeyAction::SensitiveCommandSubmitted if state.surrounding.is_none() => {
                            state.hide();
                            state.terminal_sudo_pending = true;
                            state.terminal_sudo_started = Some(std::time::Instant::now());
                            state.set_sensitive(true);
                            forward!();
                        }

                        KeyAction::SensitiveCommandSubmitted | KeyAction::HideSuggestions => {
                            forward!();
                            state.hide();
                        }

                        KeyAction::ShowSuggestions { candidates, .. } => {
                            forward!();
                            // Deliver the keystroke before spending time drawing the bar
                            let _ = conn.flush();
                            state.render_and_show(qh, &candidates, None);
                        }

                        KeyAction::CancelNavigation => {
                            // SWALLOW the other arrow / Escape key when exiting navigation!
                            state.swallowed_keys.insert(key);
                            state.hide();
                        }

                        KeyAction::UpdateSelection { index } => {
                            // SWALLOW key (the select key, Left / Right)
                            state.swallowed_keys.insert(key);
                            if let InputMode::Navigating { candidates, .. } =
                                &state.state_machine.mode
                            {
                                let c = candidates.clone();
                                state.render_and_show(qh, &c, Some(index));
                            }
                        }

                        commit @ KeyAction::CommitCandidate { .. } => {
                            // SWALLOW Enter / Space / Tab key (zero accidental chat sends or extra spaces!)
                            state.swallowed_keys.insert(key);
                            state.commit_candidate(commit, time, conn);
                        }

                        KeyAction::Consume => {
                            state.swallowed_keys.insert(key);
                        }
                    }

                    // The app repeats a key that stays down; remember it to keep up with that
                    if forwarded && !is_modifier_keysym(keysym_raw) {
                        state.held_key = Some(HeldKey {
                            key,
                            pressed_at: time,
                            keysym: keysym_raw,
                            ch: char_opt,
                            ctrl: ctrl_active,
                        });
                        if state.repeat_rate > 0 {
                            state.repeat_deadline = Some(
                                std::time::Instant::now()
                                    + std::time::Duration::from_millis(
                                        state.repeat_delay.max(0) as u64
                                    ),
                            );
                        }
                    }
                } else {
                    // Key released
                    if state.swallowed_keys.remove(&key) {
                        // Swallow release of consumed keys
                    } else if let Some(vk) = &state.vk {
                        vk.key(time, key, 0);
                    }
                    if let Some(held) = state.held_key.filter(|h| h.key == key) {
                        state.held_key = None;
                        state.repeat_deadline = None;
                        state.apply_key_repeats(held, time, qh);
                    }
                }
            }

            zwp_input_method_keyboard_grab_v2::Event::RepeatInfo { rate, delay } => {
                state.repeat_rate = rate;
                state.repeat_delay = delay;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cursor_is_a_hand_over_a_suggestion() {
        assert_eq!(cursor_shape_at(Some(0)), CursorShape::Pointer);
        assert_eq!(cursor_shape_at(Some(4)), CursorShape::Pointer);
        assert_eq!(cursor_shape_at(None), CursorShape::Default);
    }

    #[test]
    fn test_click_right_after_the_bar_changes_is_ignored() {
        let now = std::time::Instant::now();
        let ago = |ms| {
            now.checked_sub(std::time::Duration::from_millis(ms))
                .unwrap()
        };
        assert!(!click_too_soon(None, now));
        // The second click of a double click lands within a few hundred milliseconds
        assert!(click_too_soon(Some(ago(0)), now));
        assert!(click_too_soon(Some(ago(250)), now));
        assert!(!click_too_soon(Some(ago(300)), now));
        assert!(!click_too_soon(Some(ago(2000)), now));
        // A change stamped after the click (clocks read out of order) never blocks forever
        assert!(click_too_soon(
            Some(now + std::time::Duration::from_millis(5)),
            now
        ));
    }

    #[test]
    fn test_repeats_while_held() {
        // Hyprland's defaults: 25 keys per second after 600 ms
        assert_eq!(repeats_while_held(25, 600, 599), 0);
        assert_eq!(repeats_while_held(25, 600, 600), 1);
        assert_eq!(repeats_while_held(25, 600, 639), 1);
        assert_eq!(repeats_while_held(25, 600, 640), 2);
        assert_eq!(repeats_while_held(25, 600, 2500), 48);
        // Repeat turned off, or nonsense values from the compositor
        assert_eq!(repeats_while_held(0, 600, 5000), 0);
        assert_eq!(repeats_while_held(-1, 600, 5000), 0);
        assert_eq!(repeats_while_held(25, -5, 5000), 0);
    }

    fn pill(x: f32, width: f32) -> PillRect {
        PillRect {
            x,
            y: 9.0,
            width,
            height: 46.0,
        }
    }

    #[test]
    fn test_pill_hit_test_below_the_caret() {
        // A 200x64 pixmap at the top of its own buffer, attached at scale 2: 100x32 on screen
        let areas = BarHitAreas::new(200, 64, 0, &[pill(12.0, 104.0), pill(126.0, 60.0)]);
        assert_eq!(
            areas.bar,
            SurfaceRect {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 32.0
            }
        );
        assert_eq!(
            areas.pills[0],
            SurfaceRect {
                x: 6.0,
                y: 4.5,
                width: 52.0,
                height: 23.0
            }
        );

        assert_eq!(areas.pill_at(6.0, 4.5), Some(0));
        assert_eq!(areas.pill_at(30.0, 16.0), Some(0));
        assert_eq!(areas.pill_at(57.9, 27.4), Some(0));
        assert_eq!(areas.pill_at(63.0, 16.0), Some(1));
        assert_eq!(areas.pill_at(92.9, 16.0), Some(1));

        // The gap between the pills (58 to 63) and the padding around them take no click
        for (x, y) in [
            (58.0, 16.0),
            (60.5, 16.0),
            (62.9, 16.0),
            (3.0, 16.0),
            (30.0, 2.0),
            (30.0, 27.5),
            (95.0, 16.0),
        ] {
            assert_eq!(areas.pill_at(x, y), None, "({x}, {y})");
            assert!(areas.on_bar(x, y), "({x}, {y})");
        }
        assert!(!areas.on_bar(30.0, 32.0));
        assert!(!areas.on_bar(100.0, 16.0));
    }

    #[test]
    fn test_pill_hit_test_above_the_caret() {
        // Above the caret the bar is drawn at the bottom of a buffer taller than the monitor,
        // here 2 * 1440 + 64 px, so it starts 1440 units down the surface
        let bar_top = 2 * 1440;
        let areas = BarHitAreas::new(200, 64, bar_top, &[pill(12.0, 104.0), pill(126.0, 60.0)]);
        let top = 1440.0;
        assert_eq!(areas.bar.y, top);
        assert_eq!(areas.pills[1].y, top + 4.5);

        // Where the pills would be without the offset is transparent popup, not bar
        assert_eq!(areas.pill_at(30.0, 16.0), None);
        assert!(!areas.on_bar(30.0, 16.0));
        assert!(!areas.on_bar(30.0, top - 0.5));

        assert_eq!(areas.pill_at(30.0, top + 16.0), Some(0));
        assert_eq!(areas.pill_at(80.0, top + 16.0), Some(1));
        assert_eq!(areas.pill_at(60.5, top + 16.0), None);
        assert!(areas.on_bar(60.5, top + 16.0));
        assert_eq!(areas.pill_at(30.0, top + 2.0), None);
    }

    #[test]
    fn test_drawn_pill_centers_hit_their_own_candidate() {
        let renderer = Renderer::new().expect("Failed to initialize renderer");
        let candidates = vec![
            "I".to_string(),
            "program".to_string(),
            "progressively".to_string(),
        ];
        let pixmap = renderer.render_bar(&candidates, None).unwrap();
        let pills = renderer.pill_rects(&candidates);
        for bar_top in [0, 2 * 2160] {
            let areas = BarHitAreas::new(pixmap.width(), pixmap.height(), bar_top, &pills);
            assert_eq!(areas.pills.len(), candidates.len());
            for (i, p) in pills.iter().enumerate() {
                let x = f64::from(p.x + p.width / 2.0) / 2.0;
                let y = (f64::from(bar_top) + f64::from(p.y + p.height / 2.0)) / 2.0;
                assert_eq!(areas.pill_at(x, y), Some(i), "pill {i}, bar at {bar_top}");
            }
        }
    }

    #[test]
    fn test_modifiers_are_not_repeated() {
        assert!(is_modifier_keysym(0xffe1)); // Shift_L
        assert!(is_modifier_keysym(0xffe3)); // Control_L
        assert!(is_modifier_keysym(0xfe03)); // ISO_Level3_Shift (AltGr)
        assert!(!is_modifier_keysym(0xff08)); // BackSpace
        assert!(!is_modifier_keysym(0x0061)); // a
    }
}
