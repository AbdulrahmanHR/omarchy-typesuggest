use crate::theme::{Palette, Rgba};
use fontdue::{Font, FontSettings};
use std::path::{Path, PathBuf};
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};

pub struct Renderer {
    font: Font,
    /// Size of the bar relative to the default (config `bar_scale`)
    pub bar_scale: f32,
    /// Bar colors: the theme's palette with the config's `color_*` overrides applied
    pub palette: Palette,
}

impl Renderer {
    pub fn new() -> Result<Self, String> {
        Self::with_font("")
    }

    /// `font` is the config value: empty for the default chain, a font file path, or a
    /// fontconfig family name
    pub fn with_font(font: &str) -> Result<Self, String> {
        Ok(Self {
            font: load_font(font)?,
            bar_scale: 1.0,
            palette: Palette::BUILTIN,
        })
    }

    /// Switch to another configured font, keeping the current one if nothing can be loaded
    pub fn set_font(&mut self, font: &str) -> Result<(), String> {
        self.font = load_font(font)?;
        Ok(())
    }

    /// Measure text width in pixels at specified scale
    pub fn measure_text(&self, text: &str, size: f32) -> f32 {
        let mut width = 0.0;
        for ch in text.chars() {
            let metrics = self.font.metrics(ch, size);
            width += metrics.advance_width;
        }
        width
    }

    /// Render 3 suggestion pills into an ARGB32 pixmap at 2x HiDPI resolution
    /// candidates: slice of candidate strings
    /// selected_index: Some(index) if in Navigating mode, None if in Suggesting mode
    pub fn render_bar(
        &self,
        candidates: &[String],
        selected_index: Option<usize>,
    ) -> Option<Pixmap> {
        if candidates.is_empty() {
            return None;
        }

        // Render at 2x HiDPI scale for razor-sharp rendering on 1.25x / 2x Wayland displays,
        // times the user's bar size; every dimension below derives from this
        let scale = 2.0f32 * self.bar_scale;
        let font_size = 12.5 * scale;
        let pill_padding_h = 12.0 * scale;
        let pill_spacing = 5.0 * scale;
        let bar_padding_h = 6.0 * scale;
        let bar_padding_v = 4.5 * scale;
        let pill_height: f32 = 23.0 * scale;
        let bar_height: f32 = pill_height + (bar_padding_v * 2.0);

        // Measure widths for each candidate pill
        let mut pill_widths = Vec::new();
        for word in candidates {
            let text_w = self.measure_text(word, font_size);
            let pill_w = (text_w + (pill_padding_h * 2.0)).max(52.0 * scale);
            pill_widths.push(pill_w);
        }

        let total_pills_w: f32 =
            pill_widths.iter().sum::<f32>() + ((pill_widths.len() - 1) as f32 * pill_spacing);
        let bar_width_f = total_pills_w + (bar_padding_h * 2.0);
        // The buffer is attached at scale 2, so both sides must be even: compositors may reject
        // a buffer whose size is not a multiple of its scale (wlroots does)
        let even = |v: f32| (v.ceil() as u32).next_multiple_of(2);
        let bar_width = even(bar_width_f);
        let bar_height_u32 = even(bar_height);

        let mut pixmap = Pixmap::new(bar_width, bar_height_u32)?;

        let palette = &self.palette;

        // 1. Draw outer container with rounded corners
        let radius = 6.0 * scale;
        if let Some(rect) = Rect::from_xywh(1.0, 1.0, bar_width as f32 - 2.0, bar_height - 2.0) {
            let path = rounded_rect_path(rect, radius);
            let mut paint = Paint::default();
            paint.set_color(to_color(palette.background));
            pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );

            let mut stroke_paint = Paint::default();
            stroke_paint.set_color(to_color(palette.border));
            let stroke = Stroke {
                width: 1.0 * scale,
                ..Default::default()
            };
            pixmap.stroke_path(&path, &stroke_paint, &stroke, Transform::identity(), None);
        }

        // 2. Draw each candidate pill
        let mut cur_x = bar_padding_h;
        for (i, word) in candidates.iter().enumerate() {
            let pill_w = pill_widths[i];
            let is_selected = selected_index == Some(i);

            let pill_rect = Rect::from_xywh(cur_x, bar_padding_v, pill_w, pill_height)?;
            let pill_path = rounded_rect_path(pill_rect, 4.5 * scale);

            let mut pill_paint = Paint::default();
            if is_selected {
                pill_paint.set_color(to_color(palette.accent));
                pixmap.fill_path(
                    &pill_path,
                    &pill_paint,
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            } else {
                pill_paint.set_color(to_color(palette.pill));
                pixmap.fill_path(
                    &pill_path,
                    &pill_paint,
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );

                let s = Stroke {
                    width: 1.0 * scale,
                    ..Default::default()
                };
                let mut sp = Paint::default();
                sp.set_color(to_color(palette.pill_border));
                pixmap.stroke_path(&pill_path, &sp, &s, Transform::identity(), None);
            }

            // Draw text
            let text_color = if is_selected {
                palette.accent_text
            } else {
                palette.text
            };
            let text_w = self.measure_text(word, font_size);
            let text_x = cur_x + ((pill_w - text_w) / 2.0);

            // Optical vertical centering using 'H' cap-height
            let h_metrics = self.font.metrics('H', font_size);
            let text_y =
                bar_padding_v + (pill_height + h_metrics.height as f32) / 2.0 - (1.0 * scale);

            self.draw_text(&mut pixmap, word, text_x, text_y, font_size, text_color);

            cur_x += pill_w + pill_spacing;
        }

        Some(pixmap)
    }

    fn draw_text(
        &self,
        pixmap: &mut Pixmap,
        text: &str,
        start_x: f32,
        baseline_y: f32,
        size: f32,
        color: Rgba,
    ) {
        let color_alpha = color.a as f32 / 255.0;
        let mut cur_x = start_x;
        for ch in text.chars() {
            let (metrics, bitmap) = self.font.rasterize(ch, size);
            let glyph_x = (cur_x + metrics.xmin as f32).round() as i32;
            let glyph_y = (baseline_y - metrics.height as f32 - metrics.ymin as f32).round() as i32;

            for row in 0..metrics.height {
                for col in 0..metrics.width {
                    let alpha = bitmap[row * metrics.width + col];
                    if alpha > 0 {
                        let px = glyph_x + col as i32;
                        let py = glyph_y + row as i32;
                        if px >= 0
                            && px < pixmap.width() as i32
                            && py >= 0
                            && py < pixmap.height() as i32
                        {
                            let idx = (py as usize * pixmap.width() as usize + px as usize) * 4;
                            let pixels = pixmap.data_mut();

                            let a_factor = alpha as f32 / 255.0 * color_alpha;
                            let dst_r = pixels[idx] as f32;
                            let dst_g = pixels[idx + 1] as f32;
                            let dst_b = pixels[idx + 2] as f32;
                            let dst_a = pixels[idx + 3] as f32;

                            let out_r = (color.r as f32 * a_factor) + (dst_r * (1.0 - a_factor));
                            let out_g = (color.g as f32 * a_factor) + (dst_g * (1.0 - a_factor));
                            let out_b = (color.b as f32 * a_factor) + (dst_b * (1.0 - a_factor));
                            let out_a = dst_a.max(a_factor * 255.0);

                            pixels[idx] = out_r.round() as u8;
                            pixels[idx + 1] = out_g.round() as u8;
                            pixels[idx + 2] = out_b.round() as u8;
                            pixels[idx + 3] = out_a.round() as u8;
                        }
                    }
                }
            }
            cur_x += metrics.advance_width;
        }
    }
}

fn to_color(c: Rgba) -> Color {
    Color::from_rgba8(c.r, c.g, c.b, c.a)
}

/// Load the configured font. One that cannot be resolved or parsed logs a warning and falls
/// back to the default chain.
fn load_font(spec: &str) -> Result<Font, String> {
    let spec = spec.trim();
    if !spec.is_empty() {
        match resolve_font_file(spec).and_then(|path| font_from_file(&path)) {
            Ok(font) => return Ok(font),
            Err(e) => eprintln!(
                "[typesuggest] Warning: cannot use font \"{}\" ({}); using the default font",
                spec, e
            ),
        }
    }
    default_font()
}

/// Segoe UI under ~/.local/share/fonts/windows, then Liberation Sans, then DejaVu Sans
fn default_font() -> Result<Font, String> {
    // Prefer Segoe UI Bold/Semibold for rich, crisp letterforms on dark backgrounds
    let mut font_paths: Vec<PathBuf> = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        let user_fonts = Path::new(&home).join(".local/share/fonts/windows");
        font_paths.push(user_fonts.join("segoeuib.ttf"));
        font_paths.push(user_fonts.join("segoeui.ttf"));
    }
    font_paths.push("/usr/share/fonts/liberation/LiberationSans-Bold.ttf".into());
    font_paths.push("/usr/share/fonts/liberation/LiberationSans-Regular.ttf".into());
    font_paths.push("/usr/share/fonts/TTF/DejaVuSans-Bold.ttf".into());
    font_paths.push("/usr/share/fonts/TTF/DejaVuSans.ttf".into());

    let data = font_paths
        .iter()
        .find_map(|path| std::fs::read(path).ok())
        .ok_or_else(|| "No suitable TTF font found on system".to_string())?;
    Font::from_bytes(data, FontSettings::default())
        .map_err(|e| format!("Failed to parse font: {}", e))
}

/// A value containing `/` is a font file path (a leading `~/` expands to $HOME); anything else
/// is a family name resolved by fontconfig
fn resolve_font_file(spec: &str) -> Result<PathBuf, String> {
    if spec.contains('/') {
        return Ok(expand_home(spec));
    }
    // The family is its own argument, never passed through a shell
    let out = std::process::Command::new("fc-match")
        .arg("--format=%{file}")
        .arg("--")
        .arg(spec)
        .output()
        .map_err(|e| format!("fc-match: {}", e))?;
    let file = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || file.is_empty() {
        return Err("fc-match found no matching font".to_string());
    }
    Ok(PathBuf::from(file))
}

fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(path),
    }
}

fn font_from_file(path: &Path) -> Result<Font, String> {
    let data = std::fs::read(path).map_err(|e| format!("{}: {}", path.display(), e))?;
    Font::from_bytes(data, FontSettings::default())
        .map_err(|e| format!("{}: {}", path.display(), e))
}

fn rounded_rect_path(rect: Rect, r: f32) -> tiny_skia::Path {
    let mut pb = PathBuilder::new();
    let x = rect.x();
    let y = rect.y();
    let w = rect.width();
    let h = rect.height();

    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();

    pb.finish().unwrap_or_else(|| {
        let mut pb = PathBuilder::new();
        pb.push_rect(rect);
        pb.finish().unwrap()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_renderer_bar() {
        let renderer = Renderer::new().expect("Failed to initialize renderer");
        let candidates = vec![
            "program".to_string(),
            "project".to_string(),
            "progress".to_string(),
        ];

        let pixmap_suggesting = renderer.render_bar(&candidates, None).unwrap();
        assert!(pixmap_suggesting.width() > 200);
        assert_eq!(pixmap_suggesting.height(), 64);

        let pixmap_navigating = renderer.render_bar(&candidates, Some(0)).unwrap();
        assert_eq!(pixmap_navigating.height(), 64);
    }

    #[test]
    fn test_renderer_bar_scale() {
        let mut renderer = Renderer::new().expect("Failed to initialize renderer");
        let candidates = vec!["program".to_string(), "project".to_string()];
        let normal = renderer.render_bar(&candidates, None).unwrap();

        renderer.bar_scale = 1.5;
        let large = renderer.render_bar(&candidates, None).unwrap();
        assert_eq!(large.height(), 96);
        assert!(large.width() > normal.width() * 14 / 10);

        renderer.bar_scale = 0.5;
        let small = renderer.render_bar(&candidates, None).unwrap();
        assert_eq!(small.height(), 32);
    }

    /// The buffer is attached at scale 2, so odd sizes would be rejected by wlroots compositors
    #[test]
    fn test_renderer_bar_size_is_even() {
        let mut renderer = Renderer::new().expect("Failed to initialize renderer");
        let candidates = vec![
            "program".to_string(),
            "project".to_string(),
            "progress".to_string(),
        ];
        for scale in [0.5, 0.8, 1.0, 1.1, 1.25, 1.33, 2.0, 2.7] {
            renderer.bar_scale = scale;
            let bar = renderer.render_bar(&candidates, Some(1)).unwrap();
            assert_eq!(bar.width() % 2, 0, "width {} at {scale}", bar.width());
            assert_eq!(bar.height() % 2, 0, "height {} at {scale}", bar.height());
        }
    }

    #[test]
    fn test_renderer_uses_palette() {
        let mut renderer = Renderer::new().expect("Failed to initialize renderer");
        renderer.palette.accent = Rgba::new(255, 0, 0, 255);
        let candidates = vec!["program".to_string(), "project".to_string()];
        let pixmap = renderer.render_bar(&candidates, Some(0)).unwrap();

        // Inside the selected pill's left padding, clear of the text
        let pixel = pixmap.pixel(18, pixmap.height() / 2).unwrap();
        assert_eq!(
            (pixel.red(), pixel.green(), pixel.blue(), pixel.alpha()),
            (255, 0, 0, 255)
        );
    }

    #[test]
    fn test_font_fallback() {
        // An unusable font falls back to the default chain instead of failing
        assert!(Renderer::with_font("/nonexistent/typesuggest-font.ttf").is_ok());
        let mut renderer = Renderer::new().unwrap();
        assert!(
            renderer
                .set_font("~/nonexistent/typesuggest-font.ttf")
                .is_ok()
        );

        if let Some(home) = std::env::var_os("HOME") {
            assert_eq!(
                expand_home("~/fonts/a.ttf"),
                PathBuf::from(home).join("fonts/a.ttf")
            );
        }
        assert_eq!(expand_home("/usr/a.ttf"), PathBuf::from("/usr/a.ttf"));
        assert_eq!(expand_home("rel/~/a.ttf"), PathBuf::from("rel/~/a.ttf"));
    }
}
