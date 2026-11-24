//! The look: themes, fonts, and the few widgets Vessel draws its own way.

pub mod app;
pub mod covers;
pub mod dialogs;
pub mod edit;
pub mod pics;
pub mod quickrun;
pub mod settings;

use crate::store::Settings;
use egui::{Align2, Color32, CornerRadius, FontFamily, FontId, Rect, Response, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use std::f32::consts::PI;

// ---- themes ------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub bg: Color32,
    pub surface: Color32,
    pub surface2: Color32,
    pub border: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub on_accent: Color32,
    pub accent_hover: Color32,
    pub light: bool,
}

pub const THEMES: &[(&str, [&str; 7])] = &[
    ("Midnight", ["#101014", "#18181e", "#22222a", "#2c2c36", "#e8e8ef", "#8a8a99", "#7c8cff"]),
    ("Gold", ["#121216", "#1b1b21", "#24242c", "#30303a", "#e8e8ef", "#8a8a99", "#c9a96e"]),
    ("Tokyo Night", ["#16161e", "#1a1b26", "#24283b", "#2f3549", "#c0caf5", "#7f88b3", "#7aa2f7"]),
    ("Catppuccin", ["#181825", "#1e1e2e", "#313244", "#45475a", "#cdd6f4", "#9399b2", "#cba6f7"]),
    ("Dracula", ["#1e1f29", "#282a36", "#343746", "#44475a", "#f8f8f2", "#9ea1b8", "#bd93f9"]),
    ("Rosé Pine", ["#191724", "#1f1d2e", "#26233a", "#403d52", "#e0def4", "#908caa", "#ebbcba"]),
    ("Nord", ["#2e3440", "#3b4252", "#434c5e", "#4c566a", "#eceff4", "#a7b0c0", "#88c0d0"]),
    ("Gruvbox", ["#1d2021", "#282828", "#32302f", "#45403d", "#ebdbb2", "#a89984", "#fabd2f"]),
    ("Emerald", ["#0e1311", "#151c19", "#1d2622", "#29342f", "#e4efe9", "#86998f", "#4ade80"]),
    ("Crimson", ["#141012", "#1d1619", "#271d21", "#36282e", "#f0e6e9", "#9c8a90", "#f43f5e"]),
    ("OLED", ["#000000", "#0b0b0d", "#151518", "#24242a", "#ededf2", "#85858f", "#e8e8ef"]),
    ("Light", ["#f3f3f6", "#ffffff", "#ececf1", "#d9d9e1", "#1d1d24", "#6b6b78", "#4f5bd5"]),
];

pub const RED: Color32 = Color32::from_rgb(0xe5, 0x53, 0x4b);
pub const RED_HOVER: Color32 = Color32::from_rgb(0xee, 0x6a, 0x62);

/// "#rgb", "#rrggbb" or "#aarrggbb" (Qt's forms)
pub fn parse_color(s: &str) -> Option<Color32> {
    let h = s.trim().strip_prefix('#')?;
    let v = |i: usize, n: usize| u8::from_str_radix(&h[i..i + n], 16).ok();
    match h.len() {
        3 => Some(Color32::from_rgb(v(0, 1)? * 17, v(1, 1)? * 17, v(2, 1)? * 17)),
        6 => Some(Color32::from_rgb(v(0, 2)?, v(2, 2)?, v(4, 2)?)),
        8 => Some(Color32::from_rgb(v(2, 2)?, v(4, 2)?, v(6, 2)?)),
        _ => None,
    }
}

pub fn hex(c: Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
}

/// HSL lightness, 0..1 (QColor.lightnessF)
fn lightness(c: Color32) -> f32 {
    let mx = c.r().max(c.g()).max(c.b()) as f32;
    let mn = c.r().min(c.g()).min(c.b()) as f32;
    (mx + mn) / 2.0 / 255.0
}

fn to_hsv(c: Color32) -> (f32, f32, f32) {
    let (r, g, b) = (c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0);
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    let d = mx - mn;
    let h = if d == 0.0 {
        0.0
    } else if mx == r {
        60.0 * (((g - b) / d) % 6.0)
    } else if mx == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    (if h < 0.0 { h + 360.0 } else { h }, if mx == 0.0 { 0.0 } else { d / mx }, mx)
}

fn from_hsv(h: f32, s: f32, v: f32) -> Color32 {
    let c = v * s;
    let hp = h / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    let f = |x: f32| ((x + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(f(r), f(g), f(b))
}

/// QColor.lighter(factor): brighter, then less saturated once it's as bright as it gets
fn lighter(c: Color32, factor: f32) -> Color32 {
    let (h, mut s, mut v) = to_hsv(c);
    v *= factor;
    if v > 1.0 {
        s = (s - (v - 1.0)).max(0.0);
        v = 1.0;
    }
    from_hsv(h, s, v)
}

pub fn alpha(c: Color32, a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (a * 255.0).round() as u8)
}

pub fn theme_named(name: &str, accent: &str) -> Theme {
    let cols = THEMES.iter().find(|(n, _)| *n == name).unwrap_or(&THEMES[0]).1;
    let p = |i: usize| parse_color(cols[i]).unwrap();
    let acc = if accent.is_empty() { None } else { parse_color(accent) }.unwrap_or(p(6));
    Theme {
        bg: p(0),
        surface: p(1),
        surface2: p(2),
        border: p(3),
        text: p(4),
        muted: p(5),
        accent: acc,
        on_accent: if lightness(acc) > 0.55 { Color32::from_rgb(0x11, 0x11, 0x16) } else { Color32::WHITE },
        accent_hover: if lightness(acc) < 0.8 { lighter(acc, 1.12) } else { lighter(acc, 1.0 / 1.08) },
        light: lightness(p(0)) > 0.5,
    }
}

pub fn current_theme(s: &Settings) -> Theme {
    theme_named(&s.theme, &s.accent)
}

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}

pub fn body(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

/// the egui style of a theme
pub fn apply_theme(ctx: &egui::Context, t: &Theme) {
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    let mut v = if t.light { egui::Visuals::light() } else { egui::Visuals::dark() };
    v.panel_fill = t.bg;
    v.window_fill = t.surface;
    v.extreme_bg_color = t.surface;
    v.text_edit_bg_color = Some(t.surface);
    v.faint_bg_color = t.surface2;
    v.code_bg_color = t.surface2;
    v.window_stroke = Stroke::new(1.0, t.border);
    v.window_corner_radius = CornerRadius::same(14);
    v.menu_corner_radius = CornerRadius::same(10);
    v.hyperlink_color = t.accent;
    v.weak_text_color = Some(t.muted);
    v.selection.bg_fill = alpha(t.accent, 0.45);
    v.selection.stroke = Stroke::new(1.0, t.accent);
    v.window_shadow = egui::Shadow { offset: [0, 6], blur: 24, spread: 0, color: Color32::from_black_alpha(90) };
    v.popup_shadow = egui::Shadow { offset: [0, 4], blur: 14, spread: 0, color: Color32::from_black_alpha(70) };
    let w = &mut v.widgets;
    for (wv, fill, stroke) in [
        (&mut w.noninteractive, t.surface, t.border),
        (&mut w.inactive, t.surface2, t.border),
        (&mut w.hovered, t.surface2, alpha(t.accent, 0.35)),
        (&mut w.active, t.surface2, t.accent),
        (&mut w.open, t.surface2, t.accent),
    ] {
        wv.bg_fill = fill;
        wv.weak_bg_fill = t.surface;
        wv.bg_stroke = Stroke::new(1.0, stroke);
        wv.fg_stroke = Stroke::new(1.5, t.text);
        wv.corner_radius = CornerRadius::same(9);
        wv.expansion = 0.0;
    }
    w.noninteractive.bg_stroke = Stroke::new(1.0, t.border);
    style.visuals = v;
    style.spacing.item_spacing = vec2(8.0, 8.0);
    style.spacing.button_padding = vec2(12.0, 6.0);
    style.spacing.interact_size = vec2(40.0, 30.0);
    style.spacing.menu_margin = egui::Margin::same(6);
    style.spacing.combo_height = 320.0;
    style.spacing.scroll = egui::style::ScrollStyle::solid();
    style.spacing.scroll.bar_width = 8.0;
    style.text_styles.insert(egui::TextStyle::Body, body(13.5));
    style.text_styles.insert(egui::TextStyle::Button, body(13.5));
    style.text_styles.insert(egui::TextStyle::Small, body(11.0));
    style.text_styles.insert(egui::TextStyle::Heading, bold(15.0));
    style.text_styles.insert(egui::TextStyle::Monospace, FontId::new(12.0, FontFamily::Monospace));
    style.interaction.tooltip_delay = 0.5;
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.set_style_of(egui::Theme::Dark, style);
}

// ---- fonts: the desktop's own (fontconfig), with fallbacks for the symbols the buttons use --------

fn fc_match(pattern: &str) -> Option<(String, u32)> {
    let out = std::process::Command::new("fc-match").args(["-f", "%{file}|%{index}", pattern]).output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout).to_string();
    let (file, index) = s.split_once('|')?;
    (!file.is_empty() && crate::util::isfile(file)).then(|| (file.to_string(), index.trim().parse().unwrap_or(0)))
}

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let add = |fonts: &mut egui::FontDefinitions, key: &str, found: Option<(String, u32)>| -> bool {
        let Some((file, index)) = found else { return false };
        let Ok(bytes) = std::fs::read(&file) else { return false };
        let blob: egui::epaint::text::Blob = std::sync::Arc::new(bytes);
        fonts.font_data.insert(key.to_string(), std::sync::Arc::new(egui::FontData::from_blob(blob, index)));
        true
    };
    let mut fallbacks = Vec::new();
    // symbols in buttons and badges: ⟳ ⚙ ▦ ☰ ⋯ ⬆ ★ ▶ ■ 🔒 ✓ ▾
    let mut seen_files = Vec::new();
    for cp in ["27f3", "2699", "25a6", "2630", "22ef", "2b06", "2605", "25b6", "25a0", "1f512", "2713", "25be"] {
        if let Some(found) = fc_match(&format!("sans-serif:charset={cp}")) {
            if !seen_files.contains(&found.0) {
                seen_files.push(found.0.clone());
                let key = format!("sym-{cp}");
                if add(&mut fonts, &key, Some(found)) {
                    fallbacks.push(key);
                }
            }
        }
    }
    let has_sans = add(&mut fonts, "sans", fc_match("sans-serif"));
    let has_bold = add(&mut fonts, "sans-bold", fc_match("sans-serif:weight=bold"));
    let has_mono = add(&mut fonts, "mono", fc_match("monospace"));
    let defaults = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let mut prop = Vec::new();
    if has_sans {
        prop.push("sans".to_string());
    }
    prop.extend(fallbacks.iter().cloned());
    prop.extend(defaults.iter().cloned());
    let mut boldf = Vec::new();
    if has_bold {
        boldf.push("sans-bold".to_string());
    }
    boldf.extend(prop.iter().cloned());
    let mut mono = Vec::new();
    if has_mono {
        mono.push("mono".to_string());
    }
    mono.extend(fonts.families.get(&FontFamily::Monospace).cloned().unwrap_or_default());
    mono.extend(fallbacks);
    fonts.families.insert(FontFamily::Proportional, prop);
    fonts.families.insert(FontFamily::Name("bold".into()), boldf);
    fonts.families.insert(FontFamily::Monospace, mono);
    ctx.set_fonts(fonts);
}

// ---- widgets -----------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Normal,
    Primary,
    Stop,
    Round,
    RoundStop,
    Seg(bool),
    Tool,
    Update,
}

/// a button drawn like the Qt Vessel's: rounded, the accent for the main action
pub struct Btn {
    text: String,
    kind: Kind,
    min_width: f32,
    enabled: bool,
    tip: Option<String>,
    spin: Option<f64>,
    size: f32,
}

impl Btn {
    pub fn new(text: impl Into<String>) -> Self {
        Btn { text: text.into(), kind: Kind::Normal, min_width: 0.0, enabled: true, tip: None, spin: None, size: 13.5 }
    }
    pub fn kind(mut self, k: Kind) -> Self {
        self.kind = k;
        self
    }
    pub fn primary(self) -> Self {
        self.kind(Kind::Primary)
    }
    pub fn min_width(mut self, w: f32) -> Self {
        self.min_width = w;
        self
    }
    pub fn enabled(mut self, e: bool) -> Self {
        self.enabled = e;
        self
    }
    pub fn tip(mut self, t: impl Into<String>) -> Self {
        self.tip = Some(t.into());
        self
    }
    /// a turning ring before the text (loading)
    pub fn spin(mut self, time: f64) -> Self {
        self.spin = Some(time);
        self
    }
    pub fn size(mut self, s: f32) -> Self {
        self.size = s;
        self
    }

    pub fn show(self, ui: &mut Ui, t: &Theme) -> Response {
        let strong = matches!(self.kind, Kind::Primary | Kind::Stop | Kind::Update);
        let font = if strong { bold(self.size) } else { body(self.size) };
        let round = matches!(self.kind, Kind::Round | Kind::RoundStop);
        let galley = ui.painter().layout_no_wrap(self.text.clone(), font.clone(), Color32::WHITE);
        let pad = match self.kind {
            Kind::Tool => vec2(8.0, 4.0),
            Kind::Update => vec2(12.0, 5.0),
            Kind::Seg(_) => vec2(14.0, 6.0),
            _ => vec2(14.0, 7.0),
        };
        let spin_w = if self.spin.is_some() && !round { 21.0 } else { 0.0 };
        let mut size = galley.size() + pad * 2.0 + vec2(spin_w, 0.0);
        size.x = size.x.max(self.min_width);
        size.y = size.y.max(if matches!(self.kind, Kind::Tool) { 28.0 } else { 30.0 });
        if round {
            size = vec2(38.0, 38.0);
        }
        let sense = if self.enabled { Sense::click() } else { Sense::hover() };
        let (rect, mut resp) = ui.allocate_exact_size(size, sense);
        let (enabled, name) = (self.enabled, self.tip.clone().filter(|_| self.text.chars().count() <= 2).unwrap_or_else(|| self.text.clone()));
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, &name));
        let hovered = self.enabled && resp.hovered();
        if hovered {
            resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        }
        if ui.is_rect_visible(rect) {
            let p = ui.painter();
            let (fill, stroke, fg, radius) = match self.kind {
                Kind::Normal => (
                    if hovered { alpha(t.accent, 0.10) } else { t.surface2 },
                    if hovered { alpha(t.accent, 0.35) } else { t.border },
                    if self.enabled { t.text } else { t.muted },
                    9.0,
                ),
                Kind::Primary | Kind::Update => (
                    if !self.enabled { alpha(t.accent, 0.5) } else if hovered { t.accent_hover } else { t.accent },
                    Color32::TRANSPARENT, t.on_accent, 9.0),
                Kind::Stop => (if hovered { RED_HOVER } else { RED }, Color32::TRANSPARENT, Color32::WHITE, 9.0),
                Kind::Round => (if hovered { t.accent_hover } else { t.accent }, Color32::TRANSPARENT, t.on_accent, 19.0),
                Kind::RoundStop => (if hovered { RED_HOVER } else { RED }, Color32::TRANSPARENT, Color32::WHITE, 19.0),
                Kind::Seg(on) => (
                    if on { t.surface2 } else { Color32::TRANSPARENT },
                    if on { t.border } else { Color32::TRANSPARENT },
                    if on || hovered { t.text } else { t.muted },
                    8.0,
                ),
                Kind::Tool => (if hovered { t.surface2 } else { Color32::TRANSPARENT }, Color32::TRANSPARENT,
                               if hovered { t.text } else { t.muted }, 8.0),
            };
            p.rect(rect, radius, fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
            let galley = ui.painter().layout_no_wrap(self.text, font, fg);
            let content_w = galley.size().x + spin_w;
            let mut x = rect.center().x - content_w / 2.0;
            if let Some(time) = self.spin {
                let side = if round { 16.0 } else { 15.0 };
                let c = if round { rect.center() } else { pos2(x + side / 2.0, rect.center().y) };
                ring(p, c, side, (time * 300.0) as f32 % 360.0, 110.0, fg, false);
                x += spin_w;
            }
            if !(round && self.spin.is_some()) {
                p.galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, fg);
            }
        }
        match self.tip {
            Some(tip) => resp.on_hover_text(tip),
            None => resp,
        }
    }
}

/// a row of exclusive buttons in a box: the key clicked, if one was
pub fn segments(ui: &mut Ui, t: &Theme, options: &[(&str, String)], current: &str) -> Option<String> {
    let mut picked = None;
    egui::Frame::new()
        .fill(t.surface)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(10)
        .inner_margin(3)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(|ui| {
                for (key, text) in options {
                    if Btn::new(text.clone()).kind(Kind::Seg(*key == current)).show(ui, t).clicked() {
                        picked = Some(key.to_string());
                    }
                }
            });
        });
    picked
}

pub fn badge(ui: &mut Ui, text: &str, bg: Color32, fg: Color32) -> Response {
    let galley = ui.painter().layout_no_wrap(text.to_string(), body(11.0), fg);
    let size = galley.size() + vec2(16.0, 4.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, 7.0, bg);
    ui.painter().galley(rect.min + vec2(8.0, 2.0), galley, fg);
    resp
}

pub fn badge_plain(ui: &mut Ui, t: &Theme, text: &str) -> Response {
    badge(ui, text, t.surface2, t.muted)
}
pub fn badge_accent(ui: &mut Ui, t: &Theme, text: &str) -> Response {
    badge(ui, text, alpha(t.accent, 0.16), t.accent)
}
pub fn badge_nsfw(ui: &mut Ui, text: &str) -> Response {
    badge(ui, text, Color32::from_rgba_unmultiplied(224, 108, 180, 41), Color32::from_rgb(0xee, 0x93, 0xc8))
}
pub fn badge_run(ui: &mut Ui, text: &str) -> Response {
    badge(ui, text, Color32::from_rgba_unmultiplied(120, 200, 140, 41), Color32::from_rgb(0x79, 0xd4, 0x94))
}

/// a check box: the accent when ticked
pub fn check(ui: &mut Ui, t: &Theme, value: &mut bool, text: &str, enabled: bool) -> Response {
    let galley = ui.painter().layout_no_wrap(text.to_string(), body(13.5), if enabled { t.text } else { t.muted });
    let size = vec2(16.0 + 8.0 + galley.size().x, galley.size().y.max(20.0));
    let (rect, mut resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    if enabled && resp.clicked() {
        *value = !*value;
        resp.mark_changed();
    }
    let v = *value;
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, enabled, v, text));
    let boxr = Rect::from_center_size(pos2(rect.min.x + 8.0, rect.center().y), vec2(16.0, 16.0));
    let p = ui.painter();
    if *value {
        p.rect(boxr, 5.0, if enabled { t.accent } else { alpha(t.accent, 0.5) }, Stroke::new(1.0, t.accent), StrokeKind::Inside);
        let pts = vec![pos2(boxr.min.x + 4.0, boxr.center().y + 0.5), pos2(boxr.min.x + 7.0, boxr.max.y - 4.5),
                       pos2(boxr.max.x - 3.5, boxr.min.y + 4.5)];
        p.add(egui::Shape::line(pts, Stroke::new(1.8, t.on_accent)));
    } else {
        let border = if enabled && resp.hovered() { alpha(t.accent, 0.6) } else { t.border };
        p.rect(boxr, 5.0, t.surface, Stroke::new(1.0, border), StrokeKind::Inside);
    }
    p.galley(pos2(boxr.max.x + 8.0, rect.center().y - galley.size().y / 2.0), galley, t.text);
    if enabled && resp.hovered() {
        resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    }
    resp
}

/// a thin progress bar; None runs back and forth (busy, no amount known)
pub fn progress(ui: &mut Ui, t: &Theme, frac: Option<f32>, width: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 10.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 5.0, t.surface2);
    match frac {
        Some(f) => {
            let w = rect.width() * f.clamp(0.0, 1.0);
            if w > 0.5 {
                p.rect_filled(Rect::from_min_size(rect.min, vec2(w.max(10.0).min(rect.width()), rect.height())), 5.0, t.accent);
            }
        }
        None => {
            let time = ui.input(|i| i.time) as f32;
            let chunk = rect.width() * 0.3;
            let x = ((time * 0.8).sin() * 0.5 + 0.5) * (rect.width() - chunk);
            p.rect_filled(Rect::from_min_size(pos2(rect.min.x + x, rect.min.y), vec2(chunk, rect.height())), 5.0, t.accent);
            ui.ctx().request_repaint();
        }
    }
    resp
}

/// a loading ring: a faint full circle and a bright arc `span` degrees long, turned to `angle`; `full` closes it
pub fn ring(p: &egui::Painter, center: egui::Pos2, size: f32, angle: f32, span: f32, color: Color32, full: bool) {
    let w = (size / 9.0).max(2.0);
    let r = (size - w) / 2.0 - 0.5;
    p.circle_stroke(center, r, Stroke::new(w, alpha(color, 0.18)));
    if full {
        p.circle_stroke(center, r, Stroke::new(w, color));
        return;
    }
    arc(p, center, r, angle, span, Stroke::new(w, color));
}

/// an arc with round ends, clockwise from `start` degrees (0 = three o'clock)
pub fn arc(p: &egui::Painter, center: egui::Pos2, r: f32, start: f32, span: f32, stroke: Stroke) {
    let n = ((span / 6.0).ceil() as usize).max(2);
    let pts: Vec<egui::Pos2> = (0..=n)
        .map(|i| {
            let a = (start + span * i as f32 / n as f32) * PI / 180.0;
            pos2(center.x + r * a.cos(), center.y + r * a.sin())
        })
        .collect();
    p.circle_filled(pts[0], stroke.width / 2.0, stroke.color);
    p.circle_filled(*pts.last().unwrap(), stroke.width / 2.0, stroke.color);
    p.add(egui::Shape::line(pts, stroke));
}

/// text cut short with … to fit `width`
pub fn elided(ui: &Ui, text: &str, font: FontId, color: Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(1.0));
    ui.painter().layout_job(job)
}

/// a one-line label shortened with … (a tooltip shows all of it)
pub fn elided_label(ui: &mut Ui, text: &str, font: FontId, color: Color32, width: f32) -> Response {
    let g = elided(ui, text, font, color, width);
    let cut = g.elided;
    let (rect, resp) = ui.allocate_exact_size(vec2(width, g.size().y), Sense::hover());
    ui.painter().galley(rect.min, g, color);
    if cut { resp.on_hover_text(text) } else { resp }
}

pub fn label(ui: &mut Ui, text: &str, font: FontId, color: Color32) -> Response {
    ui.add(egui::Label::new(egui::RichText::new(text).font(font).color(color)).selectable(false))
}

pub fn muted(ui: &mut Ui, t: &Theme, text: &str) -> Response {
    label(ui, text, body(13.0), t.muted)
}

pub fn muted_wrap(ui: &mut Ui, t: &Theme, text: &str) -> Response {
    ui.add(egui::Label::new(egui::RichText::new(text).font(body(13.0)).color(t.muted)).wrap().selectable(false))
}

pub fn h2(ui: &mut Ui, t: &Theme, text: &str) -> Response {
    label(ui, text, bold(15.0), t.text)
}

/// a panel: the surface with a border, rounded
pub fn panel_frame(t: &Theme) -> egui::Frame {
    egui::Frame::new().fill(t.surface).stroke(Stroke::new(1.0, t.border)).corner_radius(14).inner_margin(egui::Margin::symmetric(16, 14))
}

/// a text field
pub fn field<'a>(text: &'a mut String, hint: &str) -> egui::TextEdit<'a> {
    egui::TextEdit::singleline(text).hint_text(hint).margin(egui::Margin::symmetric(10, 6))
}

/// a form row: the label right-aligned in a column `label_w` wide, then the contents (the label is centred
/// on the row's height, measured the frame before)
pub fn form_row<R>(ui: &mut Ui, t: &Theme, label_w: f32, text: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    let id = ui.next_auto_id().with("form-row");
    let h: f32 = ui.data(|d| d.get_temp(id)).unwrap_or(30.0);
    let (out, used) = ui.horizontal_top(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(label_w, h.max(30.0)), Sense::hover());
        if !text.is_empty() {
            let y = rect.min.y + h.min(40.0).max(30.0) / 2.0;
            ui.painter().text(pos2(rect.max.x, y), Align2::RIGHT_CENTER, text, body(13.5), t.text);
        }
        ui.add_space(4.0);
        let r = ui.vertical(|ui| add(ui));
        (r.inner, r.response.rect.height())
    })
    .inner;
    ui.data_mut(|d| d.insert_temp(id, used));
    out
}
