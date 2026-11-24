//! Message boxes, questions, a line of text to type, and the desktop's file pickers.

use super::{Btn, Kind, Theme, body, h2};
use egui::{Id, Key};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};

pub struct Answer {
    /// the button pressed (None: closed, Escape)
    pub button: Option<usize>,
    pub checked: bool,
    pub text: String,
}

type Then<A> = Box<dyn FnOnce(&mut A, Answer)>;

pub struct Dialog<A> {
    id: u64,
    pub title: String,
    pub text: String,
    pub info: String,
    pub buttons: Vec<(String, Kind)>,
    /// the button Enter presses
    pub default: Option<usize>,
    pub check: Option<(String, bool)>,
    /// (placeholder, password?, the text so far)
    pub input: Option<(String, bool, String)>,
    pub error: String,
    focused: bool,
    then: Option<Then<A>>,
}

static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl<A> Dialog<A> {
    pub fn new(title: impl Into<String>, text: impl Into<String>) -> Self {
        Dialog {
            id: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            title: title.into(),
            text: text.into(),
            info: String::new(),
            buttons: vec![("OK".into(), Kind::Primary)],
            default: Some(0),
            check: None,
            input: None,
            error: String::new(),
            focused: false,
            then: None,
        }
    }

    pub fn info(mut self, info: impl Into<String>) -> Self {
        self.info = info.into();
        self
    }

    /// the buttons, left to right; Enter presses `default`
    pub fn buttons(mut self, b: &[(&str, Kind)], default: Option<usize>) -> Self {
        self.buttons = b.iter().map(|(t, k)| (t.to_string(), *k)).collect();
        self.default = default;
        self
    }

    /// Yes / No
    pub fn yes_no(self) -> Self {
        self.buttons(&[("No", Kind::Normal), ("Yes", Kind::Primary)], Some(1))
    }

    pub fn check(mut self, text: impl Into<String>) -> Self {
        self.check = Some((text.into(), false));
        self
    }

    pub fn input(mut self, placeholder: &str, password: bool) -> Self {
        self.input = Some((placeholder.into(), password, String::new()));
        self.buttons = vec![("Cancel".into(), Kind::Normal), ("OK".into(), Kind::Primary)];
        self.default = Some(1);
        self
    }

    pub fn then(mut self, f: impl FnOnce(&mut A, Answer) + 'static) -> Self {
        self.then = Some(Box::new(f));
        self
    }

    /// draws it; Some(answer, what to do) once it's answered
    pub fn show(&mut self, ctx: &egui::Context, t: &Theme) -> Option<(Answer, Option<Then<A>>)> {
        let mut pressed: Option<Option<usize>> = None;
        let modal = egui::Modal::new(Id::unique(("vessel-dialog", self.id)))
            .frame(super::panel_frame(t).inner_margin(20).shadow(ctx.global_style().visuals.window_shadow))
            .show(ctx, |ui| {
                ui.set_max_width(460.0);
                ui.set_min_width(340.0);
                h2(ui, t, &self.title);
                ui.add_space(4.0);
                if !self.text.is_empty() {
                    ui.add(egui::Label::new(egui::RichText::new(&self.text).font(body(13.5)).color(t.text)).wrap());
                }
                if !self.info.is_empty() {
                    super::muted_wrap(ui, t, &self.info);
                }
                if let Some((text, value)) = &mut self.check {
                    ui.add_space(4.0);
                    super::check(ui, t, value, text, true);
                }
                if let Some((hint, password, value)) = &mut self.input {
                    ui.add_space(4.0);
                    let r = ui.add(super::field(value, hint).password(*password).desired_width(f32::INFINITY));
                    if !self.focused {
                        r.request_focus();
                        self.focused = true;
                    }
                    if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        pressed = Some(self.default);
                    }
                }
                if !self.error.is_empty() {
                    super::muted(ui, t, &self.error);
                }
                ui.add_space(8.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    for (i, (text, kind)) in self.buttons.iter().enumerate().rev() {
                        if Btn::new(text.clone()).kind(*kind).min_width(72.0).show(ui, t).clicked() {
                            pressed = Some(Some(i));
                        }
                    }
                });
            });
        if pressed.is_none() {
            if ctx.input(|i| i.key_pressed(Key::Escape)) {
                pressed = Some(None);
            } else if self.input.is_none() && ctx.input(|i| i.key_pressed(Key::Enter)) && self.default.is_some() {
                pressed = Some(self.default);
            } else if modal.should_close() {
                pressed = Some(None);
            }
        }
        let button = pressed?;
        Some((Answer {
            button,
            checked: self.check.as_ref().is_some_and(|c| c.1),
            text: self.input.as_ref().map(|i| i.2.clone()).unwrap_or_default(),
        }, self.then.take()))
    }
}

/// shows the top dialog of a stack; runs what it says once it's answered
pub fn show_stack<A>(app: &mut A, stack: impl Fn(&mut A) -> &mut Vec<Dialog<A>>, ctx: &egui::Context, t: &Theme) {
    let answered = match stack(app).last_mut() {
        Some(d) => d.show(ctx, t),
        None => None,
    };
    if let Some((answer, then)) = answered {
        stack(app).pop();
        if let Some(f) = then {
            f(app, answer);
        }
    }
}

// ---- file pickers (the desktop's portal, else zenity), on their own thread ------------------------

pub struct Pick<A> {
    rx: Receiver<Option<PathBuf>>,
    then: Option<Box<dyn FnOnce(&mut A, String)>>,
}

pub struct PickOpts {
    pub title: String,
    pub dir: String,
    pub filters: Vec<(String, Vec<String>)>,
    pub folder: bool,
}

impl PickOpts {
    pub fn file(title: &str, dir: &str) -> Self {
        PickOpts { title: title.into(), dir: dir.into(), filters: vec![], folder: false }
    }
    pub fn folder(title: &str, dir: &str) -> Self {
        PickOpts { title: title.into(), dir: dir.into(), filters: vec![], folder: true }
    }
    pub fn filter(mut self, name: &str, exts: &[&str]) -> Self {
        self.filters.push((name.into(), exts.iter().map(|e| e.to_string()).collect()));
        self
    }
}

pub const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp", "ico", "svg", "PNG", "JPG", "JPEG", "WEBP"];

impl<A> Pick<A> {
    pub fn start(opts: PickOpts, ctx: &egui::Context, then: impl FnOnce(&mut A, String) + 'static) -> Self {
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut d = rfd::FileDialog::new().set_title(&opts.title);
            let dir = if crate::util::isdir(&opts.dir) { opts.dir.clone() } else { crate::util::home() };
            d = d.set_directory(dir);
            for (name, exts) in &opts.filters {
                d = d.add_filter(name, exts);
            }
            if !opts.filters.is_empty() {
                d = d.add_filter("All files", &["*"]);
            }
            let r = if opts.folder { d.pick_folder() } else { d.pick_file() };
            let _ = tx.send(r);
            ctx.request_repaint();
        });
        Pick { rx, then: Some(Box::new(then)) }
    }
}

/// runs what a finished file picker says; true while it's still open
pub fn poll_pick<A>(app: &mut A, slot: impl Fn(&mut A) -> &mut Option<Pick<A>>) -> bool {
    let result = match slot(app) {
        Some(p) => match p.rx.try_recv() {
            Ok(r) => Some((r, p.then.take())),
            Err(std::sync::mpsc::TryRecvError::Empty) => return true,
            Err(_) => Some((None, None)),
        },
        None => return false,
    };
    *slot(app) = None;
    if let Some((Some(path), Some(then))) = result {
        then(app, crate::util::path_str(&path));
    }
    false
}
