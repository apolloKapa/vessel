//! The main window: the library as a grid of covers or a list, and everything you can do from there.

use super::dialogs::{Dialog, Pick, PickOpts, poll_pick, show_stack};
use super::edit::{EditDialog, KINDS};
use super::pics::{Pictures, Shape};
use super::settings::{SIZES, SettingsDialog, parse_shortcut};
use super::{Btn, Kind, Theme, badge_accent, badge_nsfw, badge_plain, badge_run, bold, body, elided_label, field, h2, muted,
            panel_frame, progress};
use crate::files;
use crate::launcher::{self, Launcher};
use crate::net::{self, UpdateInfo};
use crate::runners::{self, is_windows};
use crate::shortcuts;
use crate::store::{self, Entry, Settings, VERSION};
use crate::task::Job;
use crate::util::{self, basename, dirname, isdir, isfile, join};
use egui::{Align, Color32, Id, Key, Layout, Rect, Sense, Stroke, StrokeKind, TextureHandle, UiBuilder, vec2};
use std::process::Child;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

pub const TABS: &[(&str, &str)] = &[("library", "Library"), ("favorites", "Favourites"), ("vr", "VR"), ("nsfw", "NSFW"), ("hidden", "Hidden")];
pub const SORTS: &[(&str, &str)] = &[("name", "Name"), ("recent", "Last played"), ("playtime", "Most played"), ("added", "Recently added")];
const TIP: &str = "Tip: drop a program, folder or archive (.zip/.rar/.7z) on this window to add it · drop files on a game to put them in its folder";

/// what reaches the window from outside: another Vessel start, the tray
pub enum AppEvent {
    Show(Vec<String>),
    TrayActivate,
    TrayOpen,
    TrayQuit,
}

/// what an edit window was opened for (what happens once it's closed)
enum Purpose {
    Add,
    Edit,
    Installer { prefix: String },
}

struct BgTask {
    text: String,
    job: Job<String>,
    then: Option<Box<dyn FnOnce(&mut VesselApp, Result<String, String>)>>,
}

struct Installer {
    child: Child,
    e: Entry,
    started: f64,
}

struct PendingDrop {
    paths: Vec<String>,
    at: f64,
    pos: Option<egui::Pos2>,
}

struct LogView {
    title: String,
    path: String,
    text: String,
}

pub struct VesselApp {
    pub s: Settings,
    pub t: Theme,
    entries: Vec<Entry>,
    pub launcher: Launcher,
    launcher_rx: Receiver<launcher::Event>,
    pub events: Receiver<AppEvent>,
    /// the window's context, for threads that need to wake it (None while it's closed)
    pub wake: Arc<Mutex<Option<egui::Context>>>,
    search: String,
    focus_search: bool,
    status: String,
    status_until: f64,
    tasks: Vec<BgTask>,
    dialogs: Vec<Dialog<VesselApp>>,
    edit: Option<(EditDialog, Purpose)>,
    edit_queue: Vec<(EditDialog, Purpose)>,
    settings: Option<SettingsDialog>,
    log: Option<LogView>,
    pics: Pictures,
    icon_tex: Option<TextureHandle>,
    nsfw_unlocked: bool,
    nsfw_shown: bool,
    unlock_text: String,
    unlock_err: String,
    unlock_focus: bool,
    update_job: Option<Job<UpdateInfo>>,
    update: Option<UpdateInfo>,
    opened_at: f64,
    checked_update: bool,
    installers: Vec<Installer>,
    pending_drop: Option<PendingDrop>,
    hit: Vec<(String, Rect)>,
    pick: Option<Pick<VesselApp>>,
    stopping: Option<(Job<String>, Entry)>,
    /// files to take in once the window is up
    pub pending_paths: Vec<String>,
    /// an installer to run once the window is up (from the run window)
    pub pending_installer: Option<String>,
    /// the window is opening (again): textures, fonts and the theme are new
    pub new_window: bool,
    /// closing the window ends Vessel
    pub quit: bool,
    /// the window closed to the tray
    pub hidden: bool,
    hide_now: bool,
    told_tray: bool,
    pub tray: Option<crate::tray::Handle>,
    applied_theme: String,
    was_minimized: bool,
}

impl VesselApp {
    pub fn new(launcher: Launcher, launcher_rx: Receiver<launcher::Event>, events: Receiver<AppEvent>,
               wake: Arc<Mutex<Option<egui::Context>>>) -> Self {
        let s = store::settings();
        VesselApp {
            t: super::current_theme(&s),
            entries: store::library(),
            s,
            launcher,
            launcher_rx,
            events,
            wake,
            search: String::new(),
            focus_search: false,
            status: String::new(),
            status_until: 0.0,
            tasks: vec![],
            dialogs: vec![],
            edit: None,
            edit_queue: vec![],
            settings: None,
            log: None,
            pics: Pictures::new(),
            icon_tex: None,
            nsfw_unlocked: false,
            nsfw_shown: false,
            unlock_text: String::new(),
            unlock_err: String::new(),
            unlock_focus: true,
            update_job: None,
            update: None,
            opened_at: util::now(),
            checked_update: false,
            installers: vec![],
            pending_drop: None,
            hit: vec![],
            pick: None,
            stopping: None,
            pending_paths: vec![],
            pending_installer: None,
            new_window: true,
            quit: false,
            hidden: false,
            hide_now: false,
            told_tray: false,
            tray: None,
            applied_theme: String::new(),
            was_minimized: false,
        }
    }

    // ---- helpers ---------------------------------------------------------------------------------

    pub fn say(&mut self, text: impl Into<String>, secs: f64) {
        self.status = text.into();
        self.status_until = util::now() + secs;
    }

    fn warn(&mut self, title: impl Into<String>, text: impl Into<String>) {
        self.dialogs.push(Dialog::new(title, text));
    }

    fn reload(&mut self) {
        self.s = store::settings();
        self.entries = store::library();
        if self.s.tab == "nsfw" && !self.nsfw_tab_visible() {
            store::save_settings(&[("tab", "library".into())]);
            self.s = store::settings();
        }
    }

    fn set(&mut self, key: &str, value: &str) {
        store::save_settings(&[(key, value.into())]);
        self.reload();
    }

    // ---- NSFW privacy ----------------------------------------------------------------------------

    fn nsfw_tab_visible(&self) -> bool {
        !self.s.nsfw_hidden || self.nsfw_shown
    }

    fn nsfw_locked(&self) -> bool {
        self.s.has_password() && !self.nsfw_unlocked
    }

    /// NSFW entries are kept out of every other tab and count while the tab is hidden or locked
    fn nsfw_private(&self) -> bool {
        !self.nsfw_tab_visible() || self.nsfw_locked()
    }

    fn toggle_nsfw_tab(&mut self) {
        let s = store::settings();
        if s.nsfw_hidden {
            self.nsfw_shown = !self.nsfw_shown;
            if !self.nsfw_shown {
                self.nsfw_unlocked = false;
                if s.tab == "nsfw" {
                    store::save_settings(&[("tab", "library".into())]);
                }
            } else {
                store::save_settings(&[("tab", "nsfw".into())]);
            }
        } else if s.tab == "nsfw" {
            self.nsfw_unlocked = false;
            store::save_settings(&[("tab", "library".into())]);
        } else {
            store::save_settings(&[("tab", "nsfw".into())]);
        }
        self.unlock_focus = true;
        self.reload();
    }

    /// hiding Vessel locks NSFW again and hides a hidden tab
    pub fn lock_nsfw(&mut self) {
        if self.nsfw_unlocked || self.nsfw_shown {
            self.nsfw_unlocked = false;
            self.nsfw_shown = false;
            let s = store::settings();
            if s.tab == "nsfw" && s.nsfw_hidden {
                store::save_settings(&[("tab", "library".into())]);
            }
            self.reload();
        }
    }

    fn in_tab(&self, e: &Entry, tab: &str) -> bool {
        if e.nsfw && tab != "nsfw" && self.nsfw_private() {
            return false;
        }
        if tab == "hidden" {
            return e.hidden;
        }
        if e.hidden {
            return false;
        }
        if tab == "nsfw" {
            return e.nsfw;
        }
        if e.nsfw && !self.s.show_nsfw {
            return false;
        }
        tab == "library" || (tab == "favorites" && e.favorite) || (tab == "vr" && e.vr)
    }

    fn visible_entries(&self) -> Vec<Entry> {
        let q = self.search.trim().to_lowercase();
        let mut out: Vec<Entry> = self.entries.iter()
            .filter(|e| e.kind == self.s.kind)
            .filter(|e| q.is_empty() || e.name.to_lowercase().contains(&q) || e.exe.to_lowercase().contains(&q))
            .filter(|e| self.in_tab(e, &self.s.tab))
            .cloned()
            .collect();
        out.sort_by(|a, b| util::natural_cmp(&a.name, &b.name));
        match self.s.sort.as_str() {
            "recent" => out.sort_by_key(|e| -e.last_played),
            "playtime" => out.sort_by_key(|e| -e.playtime),
            "added" => out.sort_by_key(|e| -e.added),
            _ => {}
        }
        out
    }

    fn blur(&self, e: &Entry) -> bool {
        e.nsfw && self.s.blur_nsfw
    }

    // ---- the frame -------------------------------------------------------------------------------

    fn init_window(&mut self, ctx: &egui::Context) {
        self.new_window = false;
        super::install_fonts(ctx);
        self.applied_theme.clear();
        self.pics.reset(ctx);
        self.icon_tex = Some(super::pics::app_icon_texture(ctx, (42.0 * ctx.pixels_per_point() * 2.0) as u32));
        *self.wake.lock().unwrap() = Some(ctx.clone());
        let c = ctx.clone();
        self.launcher.on_event(move || c.request_repaint());
        self.reload();
        self.hidden = false;
        self.hide_now = false;
        self.unlock_focus = true;
    }

    fn apply_theme_if_changed(&mut self, ctx: &egui::Context) {
        let t = match &self.settings {
            Some(d) => d.preview_theme(),
            None => super::current_theme(&self.s),
        };
        let key = format!("{:?}", (t.bg, t.accent));
        if key != self.applied_theme {
            self.applied_theme = key;
            super::apply_theme(ctx, &t);
        }
        self.t = t;
    }

    pub fn frame(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if self.new_window {
            self.init_window(&ctx);
        }
        self.apply_theme_if_changed(&ctx);
        self.poll(&ctx);
        self.handle_close(&ctx);
        self.shortcuts(&ctx);
        let t = self.t;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(t.bg).inner_margin(egui::Margin { left: 22, right: 22, top: 18, bottom: 12 }))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(10.0, 12.0);
                self.header(ui);
                self.bar(ui);
                let list_h = ui.available_height() - 30.0;
                self.hit.clear();
                egui::ScrollArea::vertical().auto_shrink([false, false]).max_height(list_h).show(ui, |ui| {
                    ui.set_width(ui.available_width() - 6.0);
                    self.list(ui);
                });
                self.status_row(ui);
            });
        self.drop_overlay(&ctx);
        self.modals(&ctx);
    }

    fn poll(&mut self, ctx: &egui::Context) {
        // the window's own: minimised locks NSFW
        let minimized = ctx.input(|i| i.viewport().minimized.unwrap_or(false));
        if minimized && !self.was_minimized {
            self.lock_nsfw();
        }
        self.was_minimized = minimized;
        while let Ok(ev) = self.launcher_rx.try_recv() {
            match ev {
                launcher::Event::Started(eid) => {
                    let name = store::get_entry(&eid).map(|e| e.name).unwrap_or_else(|| "It".into());
                    self.say(format!("{name} is up."), 5.0);
                }
                launcher::Event::Finished(eid, code, secs) => self.on_finished(&eid, code, secs),
                launcher::Event::Changed => self.reload(),
            }
        }
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                AppEvent::Show(paths) => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    self.pending_paths.extend(paths);
                }
                AppEvent::TrayActivate => self.hide_to_tray(ctx),
                AppEvent::TrayOpen => ctx.send_viewport_cmd(egui::ViewportCommand::Focus),
                AppEvent::TrayQuit => self.quit_vessel(ctx),
            }
        }
        if !self.pending_paths.is_empty() {
            let paths = std::mem::take(&mut self.pending_paths);
            self.import_paths(ctx, paths);
        }
        if let Some(inst) = self.pending_installer.take() {
            let name = util::guess_name(&inst);
            self.run_installer(&inst, &name);
        }
        // background work
        let mut i = 0;
        while i < self.tasks.len() {
            if let Some(r) = self.tasks[i].job.poll() {
                let mut task = self.tasks.remove(i);
                if let Some(then) = task.then.take() {
                    then(self, r);
                }
            } else {
                i += 1;
            }
        }
        if let Some((job, e)) = &mut self.stopping {
            if job.poll().is_some() {
                let e = e.clone();
                self.stopping = None;
                self.do_launch(&e);
            }
        }
        self.poll_installers();
        poll_pick(self, |a| &mut a.pick);
        // asks GitHub once, a few seconds after start
        if !self.checked_update && util::now() - self.opened_at > 3.0 {
            self.checked_update = true;
            self.update_job = Some(Job::start(ctx, |_| net::check_update()));
        }
        if let Some(job) = &mut self.update_job {
            if let Some(r) = job.poll() {
                self.update_job = None;
                if let Ok(info) = r {
                    self.update = Some(info);
                }
            }
        }
        self.poll_drop(ctx);
        // timers: the status line, loading rings, the start check, installers
        if self.status_until > 0.0 {
            let left = self.status_until - util::now();
            if left <= 0.0 {
                self.status_until = 0.0;
                self.status.clear();
            } else {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(left));
            }
        }
        if self.launcher.any_starting() {
            ctx.request_repaint_after(std::time::Duration::from_millis(30));
        }
        if !self.checked_update || !self.installers.is_empty() || !self.tasks.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if self.edit.is_some() || self.settings.is_some() || !self.dialogs.is_empty() || self.log.is_some() {
            return;
        }
        let ctrl = |k| egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, k);
        if let Some(sc) = parse_shortcut(&self.s.nsfw_key).or_else(|| parse_shortcut("Ctrl+Shift+N")) {
            if ctx.input_mut(|i| i.consume_shortcut(&sc)) {
                self.toggle_nsfw_tab();
            }
        }
        if ctx.input_mut(|i| i.consume_shortcut(&ctrl(Key::F))) {
            self.focus_search = true;
        }
        if ctx.input_mut(|i| i.consume_shortcut(&ctrl(Key::N))) {
            self.add(ctx, "");
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::F5)) {
            self.pics.reset(ctx);
            self.reload();
        }
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        let t = self.t;
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            if let Some(tex) = &self.icon_tex {
                ui.add(egui::Image::from_texture((tex.id(), vec2(42.0, 42.0))));
            }
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                super::label(ui, "Vessel", bold(22.0), t.text);
                let private = self.nsfw_private();
                let counted: Vec<&Entry> = self.entries.iter().filter(|e| !(private && e.nsfw)).collect();
                let games = counted.iter().filter(|e| e.kind == "game").count();
                let apps = counted.len() - games;
                let mut sub = format!("{games} game{} · {apps} app{}", if games != 1 { "s" } else { "" }, if apps != 1 { "s" } else { "" });
                let running = self.launcher.running_ids().len();
                if running > 0 {
                    sub += &format!(" · {running} running");
                }
                muted(ui, &t, &sub);
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if Btn::new("⚙").kind(Kind::Tool).size(16.0).tip("Settings").show(ui, &t).clicked() {
                    self.open_settings(&ctx, false);
                }
                let upd = match &self.update {
                    Some(i) if i.newer => Btn::new(format!("⬆  Update to {}", i.version)).kind(Kind::Update).size(13.0)
                        .tip(format!("Vessel {} is out (you have {VERSION})", i.version)),
                    _ => Btn::new("⟳").kind(Kind::Tool).size(16.0).tip("Check for updates"),
                };
                if upd.show(ui, &t).clicked() {
                    self.open_settings(&ctx, true);
                }
                let add = if self.s.kind == "game" { "+  Add a game" } else { "+  Add an app" };
                if Btn::new(add).primary().show(ui, &t).clicked() {
                    self.add(&ctx, "");
                }
                ui.add_space(6.0);
                let private = self.nsfw_private();
                let opts: Vec<(&str, String)> = KINDS.iter().rev().map(|(k, label)| {
                    let n = self.entries.iter().filter(|e| !(private && e.nsfw) && e.kind == *k && !e.hidden).count();
                    (*k, if n > 0 { format!("{label}  {n}") } else { label.to_string() })
                }).collect::<Vec<_>>().into_iter().rev().collect();
                let sw = segments_width(ui, &opts);
                ui.allocate_ui_with_layout(vec2(sw, 38.0), Layout::left_to_right(Align::Center), |ui| {
                    if let Some(k) = super::segments(ui, &t, &opts, &self.s.kind) {
                        self.set("kind", &k);
                    }
                });
            });
        });
    }

    fn bar(&mut self, ui: &mut egui::Ui) {
        let t = self.t;
        ui.horizontal(|ui| {
            let opts: Vec<(&str, String)> = TABS.iter().filter(|(k, _)| *k != "nsfw" || self.nsfw_tab_visible()).map(|(k, label)| {
                if *k == "nsfw" {
                    // never says how many there are
                    return (*k, format!("{}{label}", if self.nsfw_locked() { "🔒  " } else { "" }));
                }
                let n = self.entries.iter().filter(|e| e.kind == self.s.kind && self.in_tab(e, k)).count();
                (*k, if n > 0 && *k != "library" { format!("{label}  {n}") } else { label.to_string() })
            }).collect();
            if let Some(k) = super::segments(ui, &t, &opts, &self.s.tab) {
                if k != "nsfw" {
                    self.nsfw_unlocked = false;
                }
                self.unlock_focus = true;
                self.set("tab", &k);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let vopts: Vec<(&str, String)> = vec![("list", "☰  List".to_string()), ("grid", "▦  Grid".to_string())];
                let sw = segments_width(ui, &vopts);
                ui.allocate_ui_with_layout(vec2(sw, 38.0), Layout::left_to_right(Align::Center), |ui| {
                    if let Some(k) = super::segments(ui, &t, &vopts, &self.s.view) {
                        self.set("view", &k);
                    }
                });
                let cur = SORTS.iter().find(|(k, _)| *k == self.s.sort).map(|(_, v)| *v).unwrap_or("Name");
                let mut sort = self.s.sort.clone();
                egui::ComboBox::from_id_salt("sort").selected_text(cur).width(130.0).show_ui(ui, |ui| {
                    for (k, v) in SORTS {
                        ui.selectable_value(&mut sort, k.to_string(), *v);
                    }
                });
                if sort != self.s.sort {
                    self.set("sort", &sort);
                }
                if !self.search.is_empty() && Btn::new("✕").kind(Kind::Tool).size(13.0).tip("Clear the search").show(ui, &t).clicked() {
                    self.search.clear();
                }
                let r = ui.add(field(&mut self.search, "Search").desired_width(ui.available_width()));
                if self.focus_search {
                    r.request_focus();
                    self.focus_search = false;
                }
                if r.has_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
                    self.search.clear();
                }
            });
        });
    }

    fn list(&mut self, ui: &mut egui::Ui) {
        let t = self.t;
        if self.s.tab == "nsfw" && self.nsfw_locked() {
            self.lock_panel(ui);
            return;
        }
        let shown = self.visible_entries();
        if shown.is_empty() {
            self.empty_state(ui);
            return;
        }
        let clip = ui.clip_rect();
        if self.s.view == "grid" {
            let w = SIZES.iter().find(|(k, _)| *k == self.s.size).map(|(_, w)| *w).unwrap_or(172.0);
            let avail = ui.available_width() - 2.0;
            let cols = (((avail + 16.0) / (w + 16.0)) as usize).max(1);
            for row in shown.chunks(cols) {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 16.0;
                    for e in row {
                        self.tile(ui, e, w, clip);
                    }
                });
                ui.add_space(6.0);
            }
        } else {
            ui.spacing_mut().item_spacing.y = 8.0;
            for e in &shown {
                self.card(ui, e, clip);
            }
        }
        let _ = t;
    }

    fn running_badge(&self, ui: &mut egui::Ui, e: &Entry) {
        if self.launcher.is_running(&e.id) {
            badge_run(ui, if self.launcher.is_starting(&e.id) { "Starting" } else { "Running" });
        }
    }

    fn play_button(&mut self, ui: &mut egui::Ui, e: &Entry, round: bool) {
        let t = self.t;
        let time = ui.input(|i| i.time);
        let running = self.launcher.is_running(&e.id);
        let b = if self.launcher.is_starting(&e.id) {
            // loading: a ring until its window shows (a click still stops it)
            let b = if round { Btn::new("").kind(Kind::Round) } else { Btn::new("Starting").primary().min_width(92.0) };
            b.spin(time).tip("Starting… (click to stop)")
        } else if round {
            if running { Btn::new("■").kind(Kind::RoundStop).size(13.0).tip("Stop") } else { Btn::new("▶").kind(Kind::Round).size(15.0).tip("Play") }
        } else if running {
            Btn::new("■  Stop").kind(Kind::Stop).min_width(92.0)
        } else {
            Btn::new("▶  Play").primary().min_width(92.0)
        };
        if b.show(ui, &t).clicked() {
            self.play(ui.ctx(), &e.id);
        }
    }

    fn more_button(&mut self, ui: &mut egui::Ui, e: &Entry) {
        let t = self.t;
        let r = Btn::new("⋯").kind(Kind::Tool).size(16.0).show(ui, &t);
        egui::Popup::menu(&r).show(|ui| self.entry_menu(ui, e));
    }

    fn tile(&mut self, ui: &mut egui::Ui, e: &Entry, w: f32, clip: Rect) {
        let t = self.t;
        let h = (w * 1.5).round();
        ui.allocate_ui_with_layout(vec2(w, h + 46.0), Layout::top_down(Align::Min), |ui| {
            ui.set_width(w);
            ui.spacing_mut().item_spacing.y = 6.0;
            let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
            if rect.intersects(clip) {
                self.hit.push((e.id.clone(), rect.intersect(clip)));
                let hovered = resp.hovered();
                let inner = rect.shrink(2.0);
                // blurred covers show themselves while you point at them
                self.pics.paint(ui, e, inner, Shape::Cover, self.blur(e) && !hovered, 10.0, t.surface2);
                if hovered {
                    ui.painter().rect_stroke(rect, 12.0, Stroke::new(2.0, t.accent), StrokeKind::Inside);
                }
            }
            let resp = resp.on_hover_text_at_pointer("Double-click to play, right-click for more")
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if resp.double_clicked() {
                self.play(ui.ctx(), &e.id);
            }
            resp.context_menu(|ui| self.entry_menu(ui, e));
            let pb = Rect::from_min_size(rect.max - vec2(48.0, 48.0), vec2(38.0, 38.0));
            // on top of the cover, without moving what comes after it down
            let mut child = ui.new_child(UiBuilder::new().max_rect(pb));
            self.play_button(&mut child, e, true);
            if self.launcher.is_running(&e.id) {
                let br = Rect::from_min_size(rect.min + vec2(10.0, 10.0), vec2(w - 20.0, 20.0));
                let mut child = ui.new_child(UiBuilder::new().max_rect(br));
                self.running_badge(&mut child, e);
            }
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 1.0);
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    let tw = w - 36.0;
                    let name = format!("{}{}", if e.favorite { "★ " } else { "" }, e.name);
                    elided_label(ui, &name, bold(13.5), t.text, tw);
                    let mut sub = format!("{}{}", if e.vr { "VR · " } else { "" }, runners::runner_label(e, &self.s));
                    if e.playtime > 0 {
                        sub += &format!(" · {}", util::human_time(e.playtime));
                    }
                    elided_label(ui, &sub, body(12.5), t.muted, tw);
                });
                self.more_button(ui, e);
            });
        });
    }

    fn card(&mut self, ui: &mut egui::Ui, e: &Entry, clip: Rect) {
        let t = self.t;
        let w = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(vec2(w, 72.0), Sense::click());
        if !rect.intersects(clip) {
            return;
        }
        self.hit.push((e.id.clone(), rect.intersect(clip)));
        let hovered = ui.rect_contains_pointer(rect);
        ui.painter().rect(rect, 14.0, t.surface, Stroke::new(1.0, if hovered { super::alpha(t.accent, 0.45) } else { t.border }),
                          StrokeKind::Inside);
        if resp.double_clicked() {
            self.play(ui.ctx(), &e.id);
        }
        resp.context_menu(|ui| self.entry_menu(ui, e));
        let inner = Rect::from_min_max(rect.min + vec2(10.0, 10.0), rect.max - vec2(14.0, 10.0));
        ui.scope_builder(UiBuilder::new().max_rect(inner).layout(Layout::left_to_right(Align::Center)), |ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            let (thumb, _) = ui.allocate_exact_size(vec2(52.0, 52.0), Sense::hover());
            self.pics.paint(ui, e, thumb, Shape::Icon, self.blur(e), 10.0, t.surface2);
            let mid_w = ui.available_width() - 92.0 - 36.0 - 28.0;
            ui.allocate_ui_with_layout(vec2(mid_w, 52.0), Layout::top_down(Align::Min), |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.add_space(3.0);
                let name = format!("{}{}", if e.favorite { "★ " } else { "" }, e.name);
                elided_label(ui, &name, bold(14.0), t.text, mid_w);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    self.running_badge(ui, e);
                    let rl = runners::runner_label(e, &self.s);
                    if e.runner != "native" { badge_accent(ui, &t, &rl) } else { badge_plain(ui, &t, &rl) };
                    if e.nsfw {
                        badge_nsfw(ui, "NSFW");
                    }
                    if e.vr {
                        badge_accent(ui, &t, "VR");
                    }
                    if e.hidden {
                        badge_plain(ui, &t, "Hidden");
                    }
                    let mut played = if e.playtime > 0 {
                        format!("Played {}", util::human_time(e.playtime))
                    } else if e.last_played > 0 {
                        "Played briefly".into()
                    } else {
                        "Not played yet".into()
                    };
                    if e.last_played > 0 {
                        played += &format!(" · last {}", util::human_when(e.last_played));
                    }
                    muted(ui, &t, &played);
                });
            });
            self.play_button(ui, e, false);
            self.more_button(ui, e);
        });
    }

    fn entry_menu(&mut self, ui: &mut egui::Ui, e: &Entry) {
        let ctx = ui.ctx().clone();
        let running = self.launcher.is_running(&e.id);
        let eid = e.id.clone();
        if ui.button(if running { "Stop" } else { "Play" }).clicked() {
            self.play(&ctx, &eid);
        }
        if ui.button("Edit…").clicked() {
            self.edit(&eid);
        }
        if ui.button(if e.favorite { "Unfavourite" } else { "Favourite" }).clicked() {
            let v = !e.favorite;
            self.mark(&eid, move |x| x.favorite = v);
        }
        ui.separator();
        let target = if util::exists(&e.exe) { e.exe.clone() } else { String::new() };
        if ui.add_enabled(!target.is_empty(), egui::Button::new("Open its folder")).clicked() {
            runners::open_path(&if isfile(&target) { dirname(&target) } else { target.clone() });
        }
        if ui.add_enabled(!files::game_folder(e).is_empty(), egui::Button::new("Add files to its folder…")).clicked() {
            let opts = PickOpts::file(&format!("Put a file into {}'s folder", e.name), &util::home());
            let id = eid.clone();
            self.pick = Some(Pick::start(opts, &ctx, move |a: &mut VesselApp, p| a.add_to_game_folder(&id, vec![p])));
        }
        if e.runner != "native" {
            ui.menu_button("Wine / Proton", |ui| {
                let pfx = runners::prefix_of(e, &self.s);
                if ui.add_enabled(isdir(&pfx), egui::Button::new("Open the prefix folder")).clicked() {
                    runners::open_path(&pfx);
                }
                if ui.button("Open C: drive").clicked() {
                    runners::open_path(&join(&runners::wine_prefix(e, &self.s), "drive_c"));
                }
                ui.separator();
                if ui.button("Wine settings (winecfg)").clicked() {
                    self.run_tool(e, "winecfg");
                }
                if ui.button("Run an .exe in this prefix…").clicked() {
                    self.run_in_prefix(&ctx, e);
                }
                let wt = runners::which("winetricks");
                if ui.add_enabled(wt.is_some(), egui::Button::new(if wt.is_some() { "Winetricks" } else { "Winetricks (get it in Settings)" })).clicked() {
                    self.winetricks(e);
                }
                if ui.button("Registry editor").clicked() {
                    self.run_tool(e, "regedit");
                }
                ui.separator();
                if ui.button("Stop everything in this prefix").clicked() {
                    runners::kill_prefix(e, &self.s);
                    self.say("Stopped the prefix.", 7.0);
                }
            });
        }
        if ui.button("Show the last log").clicked() {
            self.show_log(e);
        }
        ui.separator();
        if ui.button(if e.nsfw { "Not NSFW" } else { "Mark as NSFW" }).clicked() {
            let v = !e.nsfw;
            self.mark(&eid, move |x| x.nsfw = v);
        }
        if ui.button(if e.hidden { "Unhide" } else { "Hide" }).clicked() {
            let v = !e.hidden;
            self.mark(&eid, move |x| x.hidden = v);
        }
        let other = if e.kind == "game" { "app" } else { "game" };
        if ui.button(format!("Move to {}", if other == "game" { "Games" } else { "Apps" })).clicked() {
            self.mark(&eid, move |x| x.kind = other.into());
            if let Some(e) = store::get_entry(&eid) {
                shortcuts::refresh_shortcuts(&e);
            }
        }
        let has = shortcuts::has_shortcut(e, false);
        if ui.button(if has { "Remove from app menu" } else { "Add to app menu" }).clicked() {
            self.toggle_shortcut(&eid, false);
        }
        let has = shortcuts::has_shortcut(e, true);
        if ui.button(if has { "Remove desktop shortcut" } else { "Make a desktop shortcut" }).clicked() {
            self.toggle_shortcut(&eid, true);
        }
        ui.separator();
        if ui.button("Remove…").clicked() {
            self.remove(&eid);
        }
    }

    fn lock_panel(&mut self, ui: &mut egui::Ui) {
        let t = self.t;
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            panel_frame(&t).inner_margin(30).show(ui, |ui| {
                ui.set_width(360.0);
                ui.vertical_centered(|ui| {
                    super::label(ui, "🔒", body(34.0), t.text);
                    h2(ui, &t, "NSFW is locked");
                    let r = ui.add(field(&mut self.unlock_text, "Password").password(true).desired_width(f32::INFINITY));
                    if self.unlock_focus {
                        r.request_focus();
                        self.unlock_focus = false;
                    }
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    if !self.unlock_err.is_empty() {
                        muted(ui, &t, &self.unlock_err);
                    }
                    if Btn::new("Unlock").primary().min_width(360.0).show(ui, &t).clicked() || enter {
                        if store::password_ok(&self.unlock_text, &self.s.nsfw_pass) {
                            self.nsfw_unlocked = true;
                            self.unlock_err.clear();
                        } else {
                            self.unlock_err = "That's not the password".into();
                            self.unlock_focus = true;
                        }
                        self.unlock_text.clear();
                    }
                });
            });
        });
    }

    fn empty_state(&mut self, ui: &mut egui::Ui) {
        let t = self.t;
        let ctx = ui.ctx().clone();
        let what = if self.s.kind == "game" { "games" } else { "apps" };
        let (title, text) = if !self.search.trim().is_empty() {
            ("Nothing matches".to_string(), "Try another search.".to_string())
        } else {
            match self.s.tab.as_str() {
                "library" => {
                    let any = self.entries.iter().any(|e| e.kind == self.s.kind);
                    (if any { format!("All your {what} are hidden or NSFW") } else { format!("No {what} yet") },
                     format!("Add {what} you have outside Steam: Windows .exe files run through Proton or Wine, native \
                              programs and AppImages run as they are. You can also drop files on this window."))
                }
                "vr" => ("No VR games".into(), "Tick “VR game” in a game's settings; games that ship VR libraries get it ticked when you add them.".into()),
                "favorites" => ("No favourites".into(), format!("Mark {what} as favourite from their ⋯ menu.")),
                "nsfw" => ("Nothing marked NSFW".into(), "Mark entries as NSFW from their ⋯ menu; they then only show here.".into()),
                _ => ("Nothing hidden".into(), "Hidden entries only show in this tab.".into()),
            }
        };
        ui.add_space(30.0);
        panel_frame(&t).inner_margin(30).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical_centered(|ui| {
                h2(ui, &t, &title);
                ui.add(egui::Label::new(egui::RichText::new(text).color(t.muted)).wrap());
                if self.s.tab == "library" && self.search.trim().is_empty() {
                    ui.add_space(4.0);
                    let add = if self.s.kind == "game" { "+  Add a game" } else { "+  Add an app" };
                    let no_proton = runners::proton_versions().is_empty();
                    let width = 140.0 + if no_proton { 150.0 } else { 0.0 };
                    ui.allocate_ui_with_layout(vec2(width, 34.0), Layout::left_to_right(Align::Center), |ui| {
                        if Btn::new(add).primary().show(ui, &t).clicked() {
                            self.add(&ctx, "");
                        }
                        if no_proton && Btn::new("Get GE-Proton").show(ui, &t).clicked() {
                            self.open_settings(&ctx, false);
                        }
                    });
                }
            });
        });
    }

    fn status_row(&mut self, ui: &mut egui::Ui) {
        let t = self.t;
        ui.horizontal(|ui| {
            let text = if let Some(task) = self.tasks.last() {
                task.text.clone()
            } else if self.status.is_empty() {
                TIP.to_string()
            } else {
                self.status.clone()
            };
            let busy = !self.tasks.is_empty();
            let w = ui.available_width() - if busy { 232.0 } else { 0.0 };
            elided_label(ui, &text, body(13.0), t.muted, w);
            if busy {
                let frac = if self.tasks.len() == 1 { self.tasks[0].job.fraction() } else { None };
                progress(ui, &t, frac, 220.0);
            }
        });
    }

    // ---- dropping files ----------------------------------------------------------------------------

    fn poll_drop(&mut self, ctx: &egui::Context) {
        let (dropped, moved, now) = ctx.input(|i| {
            let dropped: Vec<String> = i.raw.dropped_files.iter().map(|f| util::path_str(f.path())).collect();
            let moved = i.events.iter().rev().find_map(|e| match e {
                egui::Event::PointerMoved(p) => Some(*p),
                _ => None,
            });
            (dropped, moved, i.time)
        });
        if !dropped.is_empty() {
            match &mut self.pending_drop {
                Some(p) => p.paths.extend(dropped),
                None => self.pending_drop = Some(PendingDrop { paths: dropped, at: now, pos: None }),
            }
        }
        let Some(p) = &mut self.pending_drop else { return };
        // X11 says where the pointer is only once it moves again after the drop: wait a moment for that
        if let Some(m) = moved {
            p.pos = Some(m);
        }
        if p.pos.is_none() && now - p.at < 0.35 {
            ctx.request_repaint_after(std::time::Duration::from_millis(40));
            return;
        }
        let p = self.pending_drop.take().unwrap();
        let paths: Vec<String> = p.paths.into_iter().filter(|x| util::exists(x)).collect();
        if paths.is_empty() || self.edit.is_some() || self.settings.is_some() {
            return;
        }
        let target = p.pos.and_then(|pos| self.hit.iter().find(|(_, r)| r.contains(pos)).map(|(id, _)| id.clone()));
        match target {
            Some(eid) if paths.len() == 1 && files::is_image(&paths[0]) => self.set_cover_file(&eid, &paths[0]),
            Some(eid) => self.add_to_game_folder(&eid, paths),
            None if files::is_image(&paths[0]) => {}
            None => self.import_paths(ctx, paths),
        }
    }

    fn drop_overlay(&self, ctx: &egui::Context) {
        let hovering = ctx.input(|i| i.raw.hovered_files.clone());
        let Some(first) = hovering.first() else { return };
        if first.path.as_ref().is_some_and(|p| files::is_image(&util::path_str(p))) && hovering.len() == 1 {
            return;
        }
        let t = self.t;
        let rect = ctx.content_rect().shrink(10.0);
        let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::unique("drop")));
        p.rect(rect, 18.0, super::alpha(t.accent, 0.10), Stroke::new(2.0, t.accent), StrokeKind::Inside);
        p.rect_filled(rect.shrink(2.0), 16.0, super::alpha(t.bg, 0.75));
        p.text(rect.center() - vec2(0.0, 12.0), egui::Align2::CENTER_CENTER, "Drop to add it", bold(22.0), t.text);
        p.text(rect.center() + vec2(0.0, 18.0), egui::Align2::CENTER_CENTER,
               "or drop files on a game to put them in its folder (an image becomes its cover)", body(13.0), t.muted);
    }

    // ---- modal windows -----------------------------------------------------------------------------

    fn modals(&mut self, ctx: &egui::Context) {
        let t = self.t;
        if let Some((d, _)) = &mut self.edit {
            if let Some(saved) = d.show(ctx, &t) {
                let (d, purpose) = self.edit.take().unwrap();
                self.edit_closed(d, purpose, saved);
                if self.edit.is_none() && !self.edit_queue.is_empty() {
                    self.edit = Some(self.edit_queue.remove(0));
                }
            }
        }
        if let Some(d) = &mut self.settings {
            if let Some(saved) = d.show(ctx, &t) {
                let d = self.settings.take().unwrap();
                if d.restart {
                    self.restart_vessel(ctx);
                }
                let _ = saved;
                self.reload();
            }
        }
        if let Some(log) = &self.log {
            let mut close = false;
            let size = ctx.content_rect().size();
            egui::Modal::new(Id::unique("log-view")).frame(panel_frame(&t).fill(t.bg).inner_margin(16)).show(ctx, |ui| {
                ui.set_width(860f32.min(size.x - 40.0));
                h2(ui, &t, &log.title);
                egui::ScrollArea::both().max_height(500f32.min(size.y - 140.0)).stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                    ui.add(egui::TextEdit::multiline(&mut log.text.as_str()).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
                });
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if Btn::new("Close").primary().show(ui, &t).clicked() {
                            close = true;
                        }
                        if Btn::new("Open file").show(ui, &t).clicked() {
                            runners::open_path(&log.path);
                        }
                        muted(ui, &t, &log.path);
                    });
                });
            });
            if close || ctx.input(|i| i.key_pressed(Key::Escape)) {
                self.log = None;
            }
        }
        show_stack(self, |a| &mut a.dialogs, ctx, &t);
    }

    fn open_edit(&mut self, d: EditDialog, purpose: Purpose) {
        if self.edit.is_some() {
            self.edit_queue.push((d, purpose));
        } else {
            self.edit = Some((d, purpose));
        }
    }

    fn edit_closed(&mut self, d: EditDialog, purpose: Purpose, saved: bool) {
        if saved {
            self.pics.forget(&d.e.id);
            match purpose {
                Purpose::Edit => {}
                _ => {
                    if d.e.kind != self.s.kind {
                        store::save_settings(&[("kind", d.e.kind.clone().into())]);
                    }
                    self.say(format!("Added {}.", d.e.name), 7.0);
                }
            }
            self.reload();
            return;
        }
        if let Purpose::Installer { prefix } = purpose {
            if isdir(&prefix) {
                let p = prefix.clone();
                self.dialogs.push(Dialog::new("Delete the prefix?", format!("Not added. Delete the prefix the installer used?\n{prefix}"))
                    .yes_no().then(move |_a: &mut VesselApp, ans| {
                        if ans.button == Some(1) {
                            let _ = std::fs::remove_dir_all(&p);
                        }
                    }));
            }
        }
    }

    fn open_settings(&mut self, ctx: &egui::Context, check_updates: bool) {
        let mut d = SettingsDialog::new();
        if check_updates {
            d.check_updates(ctx);
            d.scroll_to_bottom = true;
        }
        self.settings = Some(d);
    }

    // ---- actions -----------------------------------------------------------------------------------

    fn add(&mut self, _ctx: &egui::Context, exe: &str) {
        let d = EditDialog::new(None, &self.s.kind, exe, None, "");
        self.open_edit(d, Purpose::Add);
    }

    fn edit(&mut self, eid: &str) {
        if let Some(e) = store::get_entry(eid) {
            self.open_edit(EditDialog::new(Some(e), "", "", None, ""), Purpose::Edit);
        }
    }

    fn run_task(&mut self, ctx: &egui::Context, text: String, f: impl FnOnce(&dyn Fn(files::Progress)) -> Result<String, String> + Send + 'static,
                then: impl FnOnce(&mut VesselApp, Result<String, String>) + 'static) {
        self.tasks.push(BgTask { text, job: Job::start(ctx, f), then: Some(Box::new(then)) });
    }

    /// what dropping on the window (or opening files with Vessel) does
    pub fn import_paths(&mut self, ctx: &egui::Context, paths: Vec<String>) {
        let archives: Vec<String> = paths.iter().filter(|p| isfile(p) && files::is_archive(p)).cloned().collect();
        let folders: Vec<String> = paths.iter().filter(|p| isdir(p)).cloned().collect();
        let programs: Vec<String> = paths.iter()
            .filter(|p| isfile(p) && !archives.contains(p) && !files::is_later_part(p)).cloned().collect();
        for a in archives.iter().take(5) {
            self.import_archive(ctx, a);
        }
        for f in folders.iter().take(5) {
            self.import_folder(f);
        }
        for p in programs.iter().take(5) {
            if is_windows(p) && files::is_installer(p) {
                let p2 = p.clone();
                self.dialogs.push(Dialog::new("Run the installer?", format!("{} looks like an installer.", basename(p)))
                    .info("Run it in a new prefix now? When it finishes, Vessel finds the installed game and adds it.\n\nNo adds the file itself as the program.")
                    .yes_no().then(move |a: &mut VesselApp, ans| {
                        if ans.button == Some(1) {
                            a.run_installer(&p2, &util::guess_name(&p2));
                        } else if ans.button == Some(0) {
                            let d = EditDialog::new(None, &a.s.kind, &p2, None, "");
                            a.open_edit(d, Purpose::Add);
                        }
                    }));
                continue;
            }
            self.add(ctx, p);
        }
    }

    fn import_archive(&mut self, ctx: &egui::Context, archive: &str) {
        let name = util::archive_name(archive);
        let root = util::expanduser(&self.s.games_root);
        let dest = util::unique_dir(&root, &name);
        let (a, d) = (archive.to_string(), dest.clone());
        let text = format!("Unpacking {} into {dest}…", basename(archive));
        let a2 = archive.to_string();
        self.run_task(ctx, text, move |report| {
            files::extract(&a, &d, report)?;
            files::flatten(&d).map_err(|e| e.to_string())?;
            Ok(d)
        }, move |app, r| match r {
            Err(e) => {
                let _ = std::fs::remove_dir_all(&dest);
                app.warn("Couldn't unpack it", format!("{}:\n\n{e}", basename(&a2)));
            }
            Ok(folder) => {
                app.say(format!("Unpacked into {folder}"), 7.0);
                app.add_from_folder(&folder, &name);
            }
        });
    }

    fn import_folder(&mut self, folder: &str) {
        let root = util::expanduser(&self.s.games_root);
        let f = folder.trim_end_matches('/').to_string();
        self.dialogs.push(Dialog::new("Add a folder", format!("Add the game in {}?", basename(&f)))
            .info(format!("Copy or move it into your games folder ({root}), or use it where it is."))
            .buttons(&[("Cancel", Kind::Normal), ("Copy there", Kind::Normal), ("Move there", Kind::Normal), ("Use where it is", Kind::Primary)], Some(3))
            .then(move |app: &mut VesselApp, ans| {
                let name = util::guess_name(&join(&f, "game.exe"));
                let choice = ans.button.unwrap_or(0);
                if choice == 3 {
                    app.add_from_folder(&f, &name);
                    return;
                }
                if choice != 1 && choice != 2 {
                    return;
                }
                let mv = choice == 2;
                if util::realpath(&f).starts_with(&format!("{}/", util::realpath(&root))) {
                    app.add_from_folder(&f, &name);
                    return;
                }
                let dest = util::unique_dir(&root, &basename(&f));
                let text = format!("{} {} to {dest}…", if mv { "Moving" } else { "Copying" }, basename(&f));
                let (src, d, r) = (f.clone(), dest.clone(), root.clone());
                let Some(ctx) = app.wake.lock().unwrap().clone() else { return };
                app.run_task(&ctx, text, move |_| {
                    if mv {
                        let _ = std::fs::create_dir_all(&r);
                        // instant on the same drive
                        if std::fs::rename(&src, &d).is_ok() {
                            return Ok(d);
                        }
                    }
                    files::copy_tree(&src, &d).map_err(|e| e.to_string())?;
                    if mv {
                        std::fs::remove_dir_all(&src).map_err(|e| e.to_string())?;
                    }
                    Ok(d)
                }, move |app, r| match r {
                    Err(e) => app.warn("Couldn't copy it", e),
                    Ok(path) => app.add_from_folder(&path, &name),
                });
            }));
    }

    fn add_from_folder(&mut self, folder: &str, name: &str) {
        let programs = files::find_programs(folder, 0.0, 6);
        let best = programs.first().cloned().unwrap_or_default();
        if !best.is_empty() && files::is_installer(&best) && programs.iter().all(|p| files::is_installer(p)) {
            let (b, n, f) = (best.clone(), name.to_string(), folder.to_string());
            self.dialogs.push(Dialog::new("Run the installer?", format!("{name} comes as an installer ({}).", basename(&best)))
                .info("Run it in a new prefix now? When it finishes, Vessel finds the installed game and adds it.")
                .yes_no().then(move |app: &mut VesselApp, ans| {
                    if ans.button == Some(1) {
                        app.run_installer(&b, &n);
                    } else if ans.button == Some(0) {
                        app.edit_from_folder(&f, &n, &b);
                    }
                }));
            return;
        }
        if best.is_empty() {
            self.say(format!("Couldn't find a program in {folder}: pick it with Browse."), 12.0);
        }
        self.edit_from_folder(folder, name, &best);
    }

    fn edit_from_folder(&mut self, folder: &str, name: &str, exe: &str) {
        let (n, x, wr) = (name.to_string(), exe.to_string(), self.s.windows_runner.clone());
        let preset = move |e: &mut Entry| {
            e.name = n.clone();
            if !x.is_empty() {
                e.exe = x.clone();
                e.runner = if is_windows(&x) { wr.clone() } else { "native".into() };
            }
        };
        let d = EditDialog::new(None, &self.s.kind, "", Some(&preset), folder);
        self.open_edit(d, Purpose::Add);
    }

    fn run_installer(&mut self, installer: &str, name: &str) {
        let mut e = Entry::new();
        e.id = util::new_id();
        e.name = name.to_string();
        e.kind = self.s.kind.clone();
        e.runner = self.s.windows_runner.clone();
        e.added = util::now_secs();
        e.prefix = runners::prefix_of(&e, &self.s);
        if util::lexists(&e.prefix) {
            e.prefix = util::unique_dir(&dirname(&e.prefix), &basename(&e.prefix));
        }
        let started = util::now();
        match runners::start_process(&e, &self.s, Some(installer), Some(&[]), false, Some(&format!("{}-install", e.id))) {
            Ok(child) => {
                self.say(format!("Installing {name}: finish the installer, then Vessel asks what to add. (A new prefix takes a minute to set up first.)"), 30.0);
                self.installers.push(Installer { child, e, started });
            }
            Err(ex) => self.warn("Can't run the installer", ex.0),
        }
    }

    fn poll_installers(&mut self) {
        let mut i = 0;
        while i < self.installers.len() {
            if !matches!(self.installers[i].child.try_wait(), Ok(None)) {
                let inst = self.installers.remove(i);
                let drive = join(&runners::wine_prefix(&inst.e, &self.s), "drive_c");
                let programs: Vec<String> = files::find_programs(&drive, inst.started - 5.0, 7).into_iter()
                    .filter(|x| !files::is_installer(x)).collect();
                if programs.is_empty() {
                    self.say("The installer finished, but Vessel couldn't tell what it installed: pick the program with Browse (it starts in the prefix's C: drive).", 15.0);
                }
                let e = inst.e.clone();
                let exe = programs.first().cloned().unwrap_or_default();
                let preset = move |x: &mut Entry| {
                    let (id, added) = (x.id.clone(), x.added);
                    *x = e.clone();
                    x.id = id;
                    x.added = added;
                    x.exe = exe.clone();
                };
                let d = EditDialog::new(None, &inst.e.kind, "", Some(&preset), &drive);
                self.open_edit(d, Purpose::Installer { prefix: inst.e.prefix.clone() });
            } else {
                i += 1;
            }
        }
    }

    pub fn add_to_game_folder(&mut self, eid: &str, paths: Vec<String>) {
        let Some(e) = store::get_entry(eid) else { return };
        let folder = files::game_folder(&e);
        if folder.is_empty() {
            self.say(format!("{} has no folder to put files in (it runs a command).", e.name), 7.0);
            return;
        }
        let archives = paths.iter().any(|p| isfile(p) && files::is_archive(p));
        let names = paths.iter().take(3).map(|p| basename(p.trim_end_matches('/'))).collect::<Vec<_>>().join(", ")
            + if paths.len() > 3 { " …" } else { "" };
        let buttons: Vec<(&str, Kind)> = if archives {
            vec![("Cancel", Kind::Normal), ("Copy as they are", Kind::Normal), ("Unpack into it", Kind::Primary)]
        } else {
            vec![("Cancel", Kind::Normal), ("Copy", Kind::Primary)]
        };
        let default = buttons.len() - 1;
        let ename = e.name.clone();
        self.dialogs.push(Dialog::new(format!("Add to {}", e.name), format!("Put {names} into {}'s folder?", e.name))
            .info(format!("{folder}\n\nFiles with the same names there are replaced."))
            .buttons(&buttons, Some(default))
            .then(move |app: &mut VesselApp, ans| {
                let Some(b) = ans.button.filter(|b| *b > 0) else { return };
                let unpack = archives && b == 2;
                let (f, p) = (folder.clone(), paths.clone());
                let Some(ctx) = app.wake.lock().unwrap().clone() else { return };
                app.run_task(&ctx, format!("Adding files to {ename}…"), move |report| files::copy_into(&p, &f, false, unpack, report),
                             move |app, r| match r {
                                 Err(e) => app.warn("Couldn't add the files", e),
                                 Ok(_) => app.say(format!("Added {names} to {folder}"), 7.0),
                             });
            }));
    }

    pub fn play(&mut self, _ctx: &egui::Context, eid: &str) {
        let Some(e) = store::get_entry(eid) else { return };
        if self.launcher.is_running(eid) {
            self.launcher.stop(&e);
            self.say(format!("Stopping {}…", e.name), 7.0);
            return;
        }
        if self.stopping.is_some() {
            return;
        }
        // anything still running in its prefix (it's open already, or its last run crashed or hung) would keep
        // it from starting (Proton waits for it to end): ask to stop that first
        if !runners::prefix_procs(&e, &self.s).is_empty() {
            let name = e.name.clone();
            self.dialogs.push(Dialog::new(format!("{name} is still running"), format!("{name} is still running, or what's left of its last run is stuck."))
                .info("It can't start again until that ends: Proton waits for it, so it would never load. Stop it and start again?")
                .buttons(&[("Cancel", Kind::Normal), ("Stop it and start", Kind::Primary)], Some(1))
                .then(move |app: &mut VesselApp, ans| {
                    if ans.button != Some(1) {
                        return;
                    }
                    let Some(ctx) = app.wake.lock().unwrap().clone() else { return };
                    let (e2, s) = (e.clone(), app.s.clone());
                    app.say(format!("Stopping what's left of {name}…"), 7.0);
                    app.stopping = Some((Job::start(&ctx, move |_| {
                        runners::stop_prefix(&e2, &s, 4.0);
                        Ok(String::new())
                    }), e));
                }));
            return;
        }
        self.do_launch(&e);
    }

    fn do_launch(&mut self, e: &Entry) {
        let started_wivrn = e.vr && self.s.start_wivrn && runners::start_wivrn();
        if let Err(ex) = self.launcher.launch(e) {
            self.warn(format!("Can't start {}", e.name), ex.0);
            return;
        }
        if started_wivrn {
            self.say(format!("Started the WiVRn server for {}: connect your headset.", e.name), 15.0);
            return;
        }
        let first = e.runner != "native" && !isdir(&join(&runners::wine_prefix(e, &self.s), "drive_c"));
        self.say(format!("Starting {}…{}", e.name, if first { " (the first start sets up its prefix, that takes a minute)" } else { "" }), 12.0);
    }

    fn on_finished(&mut self, eid: &str, code: i32, secs: f64) {
        let Some(e) = store::get_entry(eid) else { return };
        if ![0, -libc::SIGTERM, -libc::SIGKILL].contains(&code) && secs < 8.0 {
            self.say(format!("{} closed right away (exit code {code}). See ⋯ → Show the last log.", e.name), 15.0);
        } else {
            self.say(format!("{} closed after {}.", e.name, util::human_time(secs as i64)), 7.0);
        }
    }

    fn mark(&mut self, eid: &str, change: impl FnOnce(&mut Entry)) {
        store::update_entry(eid, change);
        self.reload();
    }

    fn remove(&mut self, eid: &str) {
        let Some(e) = store::get_entry(eid) else { return };
        let pfx = runners::prefix_of(&e, &self.s);
        let mut d = Dialog::new("Remove", format!("Remove {} from Vessel?", e.name)).info("The program's own files stay where they are.")
            .buttons(&[("Cancel", Kind::Normal), ("Remove", Kind::Stop)], None);
        let can_delete = e.runner != "native" && isdir(&pfx);
        if can_delete {
            d = d.check(format!("Also delete its prefix (saves inside it are lost):\n{pfx}"));
        }
        let eid = eid.to_string();
        self.dialogs.push(d.then(move |app: &mut VesselApp, ans| {
            if ans.button != Some(1) {
                return;
            }
            if app.launcher.is_running(&eid) {
                app.launcher.stop(&e);
            }
            store::remove_entry(&eid);
            shortcuts::remove_shortcut(&e, false);
            shortcuts::remove_shortcut(&e, true);
            store::remove_file(&store::covers_dir(), &e.cover);
            store::remove_file(&store::icons_dir(), &e.icon);
            if can_delete && ans.checked && isdir(&pfx) && pfx != util::home() && pfx != "/"
                && !store::library().iter().any(|x| x.id != eid && runners::prefix_of(x, &app.s) == pfx) {
                let _ = std::fs::remove_dir_all(&pfx);
            }
            app.reload();
            app.say(format!("Removed {}.", e.name), 7.0);
        }));
    }

    fn toggle_shortcut(&mut self, eid: &str, desktop: bool) {
        let Some(e) = store::get_entry(eid) else { return };
        let place = if desktop { "the desktop" } else { "the app menu" };
        if shortcuts::has_shortcut(&e, desktop) {
            shortcuts::remove_shortcut(&e, desktop);
            self.say(format!("Removed {} from {place}.", e.name), 7.0);
            return;
        }
        match shortcuts::make_shortcut(&e, desktop) {
            Err(ex) => self.say(format!("Couldn't make the shortcut: {ex}"), 7.0),
            Ok(()) => self.say(if desktop { format!("{} is on {place} now.", e.name) } else { format!("{} is in the app menu now.", e.name) }, 7.0),
        }
    }

    fn set_cover_file(&mut self, eid: &str, path: &str) {
        let name = match std::fs::read(path).map_err(|e| e.to_string()).and_then(|d| crate::imaging::save_cover(eid, &d)) {
            Ok(n) => n,
            Err(ex) => {
                self.say(format!("That image can't be used: {ex}"), 7.0);
                return;
            }
        };
        if let Some(e) = store::get_entry(eid) {
            store::remove_file(&store::covers_dir(), &e.cover);
            store::update_entry(eid, |x| x.cover = name);
        }
        self.pics.forget(eid);
        self.reload();
    }

    fn run_tool(&mut self, e: &Entry, target: &str) {
        match runners::start_process(e, &self.s, Some(target), Some(&[]), true, Some(&format!("{}-tool", e.id))) {
            Ok(child) => {
                runners::reap(child);
                self.say(format!("Starting {}…", basename(target)), 7.0);
            }
            Err(ex) => self.warn("Can't start it", ex.0),
        }
    }

    fn run_in_prefix(&mut self, ctx: &egui::Context, e: &Entry) {
        let start = if isfile(&e.exe) { dirname(&e.exe) } else { util::home() };
        let opts = PickOpts::file(&format!("Run in {}'s prefix", e.name), &start).filter("Windows programs", &["exe", "msi", "bat", "lnk", "EXE"]);
        let e = e.clone();
        self.pick = Some(Pick::start(opts, ctx, move |app: &mut VesselApp, p| app.run_tool(&e, &p)));
    }

    fn winetricks(&mut self, e: &Entry) {
        let Some(wt) = runners::which("winetricks") else { return };
        let (env, _) = runners::wine_tool_env(e, &self.s);
        if !isdir(&join(&env["WINEPREFIX"], "drive_c")) {
            self.warn("Prefix not set up yet", "Start the game once first (or open Wine settings), so its prefix exists.");
            return;
        }
        match runners::detach(&[wt, "--gui".into()], &env) {
            Ok(c) => {
                runners::reap(c);
                self.say("Starting winetricks…", 7.0);
            }
            Err(ex) => self.warn("Can't start winetricks", ex.to_string()),
        }
    }

    fn show_log(&mut self, e: &Entry) {
        let path = join(&store::logs_dir(), &format!("{}.log", e.id));
        if !util::exists(&path) {
            self.say("No log yet: it's written when the game starts.", 7.0);
            return;
        }
        let text = match std::fs::read(&path) {
            Ok(b) => String::from_utf8_lossy(&b[b.len().saturating_sub(400_000)..]).into_owned(),
            Err(ex) => ex.to_string(),
        };
        self.log = Some(LogView { title: format!("Log: {}", e.name), path, text });
    }

    // ---- closing, the tray -------------------------------------------------------------------------

    fn handle_close(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        if self.quit {
            return;
        }
        if self.hide_now || store::settings().close_to_tray {
            if self.ensure_tray() {
                self.hide_now = false;
                self.hidden = true;
                *self.wake.lock().unwrap() = None;
                if !self.told_tray {
                    self.told_tray = true;
                    runners::notify("Vessel is still running",
                                    "It's in the tray: click the icon to open it again, or right-click → Quit Vessel. (Settings can turn this off.)");
                }
                return;
            }
        }
        self.hide_now = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        self.quit_vessel(ctx);
    }

    fn ensure_tray(&mut self) -> bool {
        if self.tray.is_none() {
            self.tray = crate::tray::spawn();
        }
        self.tray.is_some()
    }

    fn hide_to_tray(&mut self, ctx: &egui::Context) {
        self.hide_now = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// quits, asking first when games are still running
    pub fn quit_vessel(&mut self, ctx: &egui::Context) {
        let running = self.launcher.running_ids();
        if running.is_empty() {
            self.quit = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if self.dialogs.iter().any(|d| d.title == "Still running") {
            return;
        }
        let names: Vec<String> = running.iter().filter_map(|i| store::get_entry(i).map(|e| e.name)).collect();
        let verb = if running.len() == 1 { "is" } else { "are" };
        self.dialogs.push(Dialog::new("Still running", format!("{} {verb} still running. Close Vessel anyway? They keep running; playtime is counted up to now.", names.join(", ")))
            .yes_no().then(|app: &mut VesselApp, ans| {
                if ans.button == Some(1) {
                    app.launcher.save_running();
                    app.quit = true;
                    if let Some(c) = app.wake.lock().unwrap().clone() {
                        c.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
            }));
    }

    /// starts a fresh Vessel (the updated AppImage) and quits this one
    fn restart_vessel(&mut self, ctx: &egui::Context) {
        crate::instance::close();
        let cmd = shortcuts::self_command();
        let env: runners::Env = std::env::vars().collect();
        if runners::detach(&cmd, &env).is_ok() {
            self.launcher.save_running();
            self.quit = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

fn segments_width(ui: &egui::Ui, opts: &[(&str, String)]) -> f32 {
    let mut w = 6.0 + 2.0 * (opts.len().saturating_sub(1)) as f32;
    for (_, text) in opts {
        w += ui.painter().layout_no_wrap(text.clone(), body(13.5), Color32::WHITE).size().x + 28.0;
    }
    w + 2.0
}

pub struct Wrapper(pub std::rc::Rc<std::cell::RefCell<VesselApp>>);

impl eframe::App for Wrapper {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.0.borrow_mut().frame(ui);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        self.0.borrow().t.bg.to_normalized_gamma_f32()
    }
}

#[cfg(test)]
mod screens {
    //! renders the window offscreen into PNGs: VESSEL_SCREENS=<folder> cargo test screens (with XDG_* set to a copy)
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn shot(h: &mut egui_kittest::Harness, dir: &str, name: &str) {
        for _ in 0..6 {
            h.step();
            std::thread::sleep(std::time::Duration::from_millis(120));
        }
        let img = h.render().expect("render");
        img.save(format!("{dir}/{name}.png")).unwrap();
    }

    #[test]
    fn screens() {
        let Ok(dir) = std::env::var("VESSEL_SCREENS") else { return };
        let (_tx, rx) = std::sync::mpsc::channel();
        let (launcher, lrx) = Launcher::new();
        let app = Rc::new(RefCell::new(VesselApp::new(launcher, lrx, rx, Default::default())));
        app.borrow_mut().checked_update = true;
        let a = app.clone();
        let mut h = egui_kittest::Harness::builder().with_size(vec2(1100.0, 720.0)).build_ui(move |ui| a.borrow_mut().frame(ui));
        shot(&mut h, &dir, "1-grid");
        app.borrow_mut().set("view", "list");
        shot(&mut h, &dir, "2-list");
        app.borrow_mut().set("view", "grid");
        app.borrow_mut().set("size", "small");
        shot(&mut h, &dir, "3-grid-small");
        app.borrow_mut().set("size", "medium");
        let first = app.borrow().entries.first().map(|e| e.id.clone()).unwrap();
        app.borrow_mut().edit(&first);
        shot(&mut h, &dir, "4-edit");
        app.borrow_mut().edit = None;
        let ctx = h.ctx.clone();
        app.borrow_mut().open_settings(&ctx, false);
        shot(&mut h, &dir, "5-settings");
        app.borrow_mut().settings = None;
        app.borrow_mut().nsfw_shown = true;
        app.borrow_mut().set("tab", "nsfw");
        shot(&mut h, &dir, "6-nsfw");
        app.borrow_mut().set("tab", "library");
        app.borrow_mut().nsfw_shown = false;
        app.borrow_mut().remove(&first);
        shot(&mut h, &dir, "7-dialog");
        app.borrow_mut().dialogs.clear();
        app.borrow_mut().set("theme", "Light");
        shot(&mut h, &dir, "8-light");
        app.borrow_mut().set("theme", "Midnight");
        app.borrow_mut().set("kind", "app");
        shot(&mut h, &dir, "9-empty");
        app.borrow_mut().set("kind", "game");
    }
}
