//! Adding or editing a game or app.

use super::covers::CoverSearch;
use super::dialogs::{Dialog, Pick, PickOpts, IMAGE_EXTS, poll_pick, show_stack};
use super::pics::Still;
use super::{Btn, Theme, check, field, form_row, segments};
use crate::files;
use crate::imaging::{self, Overlay};
use crate::runners::{self, ANTICHEATS, is_windows};
use crate::shortcuts;
use crate::store::{self, Entry, Settings};
use crate::util::{self, dirname, isfile, join};
use egui::{Id, Key, Rect, vec2};
use image::RgbaImage;

pub const KINDS: &[(&str, &str)] = &[("game", "Games"), ("app", "Apps")];
const RUNNERS: &[(&str, &str)] = &[("native", "Native"), ("proton", "Proton"), ("wine", "Wine")];

enum IconSrc {
    Exe(String),
    File(String),
}

pub struct EditDialog {
    s: Settings,
    pub new: bool,
    start_dir: String,
    pub e: Entry,
    name: String,
    exe: String,
    args: String,
    prefix: String,
    cwd: String,
    wrapper: String,
    env: String,
    kind: String,
    runner: String,
    version: String,
    versions: Vec<(String, String)>,
    mangohud: bool,
    gamemode: bool,
    favorite: bool,
    nsfw: bool,
    vr: bool,
    hidden: bool,
    eac: bool,
    battleye: bool,
    desktop: bool,
    has_gamemode: bool,
    has_mangohud: bool,
    anticheat_tips: Vec<String>,
    cover_data: Option<Vec<u8>>,
    cover_cleared: bool,
    icon_src: Option<IconSrc>,
    icon_cleared: bool,
    cover_img: Option<RgbaImage>,
    icon_img: Option<RgbaImage>,
    cover_still: Option<Still>,
    icon_still: Option<Still>,
    preview_for: String,
    dirty: bool,
    exe_had_focus: bool,
    pick: Option<Pick<EditDialog>>,
    dialogs: Vec<Dialog<EditDialog>>,
    covers: Option<CoverSearch>,
    focus_name: bool,
}

impl EditDialog {
    /// `entry`: the one to edit; else a new one of `kind`, for `exe`, with `preset` changes
    pub fn new(entry: Option<Entry>, kind: &str, exe: &str, preset: Option<&dyn Fn(&mut Entry)>, start_dir: &str) -> Self {
        let s = store::settings();
        let new = entry.is_none();
        let mut e = entry.unwrap_or_else(Entry::new);
        let mut preset_vr = false;
        let mut preset_ac = false;
        if new {
            e.id = util::new_id();
            e.kind = kind.to_string();
            e.added = util::now_secs();
            if !exe.is_empty() {
                e.exe = exe.to_string();
                e.name = util::guess_name(exe);
                e.runner = if is_windows(exe) { s.windows_runner.clone() } else { "native".into() };
            }
            if let Some(p) = preset {
                let before = (e.vr, e.eac, e.battleye);
                p(&mut e);
                preset_vr = e.vr != before.0;
                preset_ac = (e.eac, e.battleye) != (before.1, before.2);
            }
            if isfile(&e.exe) && !preset_vr {
                e.vr = runners::looks_vr(&e.exe);
            }
            if isfile(&e.exe) && is_windows(&e.exe) && !preset_ac {
                let (eac, be) = runners::looks_anticheat(&e.exe);
                e.eac = eac;
                e.battleye = be;
            }
        }
        let anticheat_tips = ANTICHEATS.iter().map(|ac| {
            let have = runners::anticheat_runtime(ac);
            format!("Loads Steam's Proton {} Runtime, for games whose {} supports Linux (Proton/Wine only). {}", ac.label,
                    ac.label, if have.is_empty() {
                        format!("Not installed yet: get it free from Steam (steam://install/{})", ac.appid)
                    } else {
                        format!("Found: {have}")
                    })
        }).collect();
        let mut d = EditDialog {
            new,
            start_dir: start_dir.to_string(),
            name: e.name.clone(),
            exe: e.exe.clone(),
            args: e.args.clone(),
            prefix: e.prefix.clone(),
            cwd: e.cwd.clone(),
            wrapper: e.wrapper.clone(),
            env: e.env.clone(),
            kind: e.kind.clone(),
            runner: String::new(),
            version: String::new(),
            versions: vec![],
            mangohud: e.mangohud,
            gamemode: e.gamemode,
            favorite: e.favorite,
            nsfw: e.nsfw,
            vr: e.vr,
            hidden: e.hidden,
            eac: e.eac,
            battleye: e.battleye,
            desktop: !new && shortcuts::has_shortcut(&e, true),
            has_gamemode: runners::which("gamemoderun").is_some(),
            has_mangohud: runners::which("mangohud").is_some(),
            anticheat_tips,
            cover_data: None,
            cover_cleared: false,
            icon_src: None,
            icon_cleared: false,
            cover_img: None,
            icon_img: None,
            cover_still: None,
            icon_still: None,
            preview_for: String::new(),
            dirty: true,
            exe_had_focus: false,
            pick: None,
            dialogs: vec![],
            covers: None,
            focus_name: new,
            s,
            e,
        };
        d.cover_img = if d.e.cover.is_empty() { None } else { imaging::load(&join(&store::covers_dir(), &d.e.cover)) };
        d.icon_img = if d.e.icon.is_empty() { None } else { imaging::load(&join(&store::icons_dir(), &d.e.icon)) };
        let runner = d.e.runner.clone();
        d.set_runner(&runner, true);
        d
    }

    pub fn title(&self) -> String {
        if self.new {
            if self.e.kind == "game" { "Add a game".into() } else { "Add an app".into() }
        } else {
            format!("Edit {}", self.e.name)
        }
    }

    fn set_runner(&mut self, runner: &str, initial: bool) {
        self.runner = runner.to_string();
        self.versions.clear();
        let vs = match runner {
            "proton" => {
                let d = runners::default_proton(&self.s);
                let dname = if d.is_empty() { "none installed".into() } else { runners::proton_label(&util::basename(&d)) };
                self.versions.push((format!("Default ({dname})"), String::new()));
                runners::proton_versions()
            }
            "wine" => {
                let vs = runners::wine_versions();
                let d = runners::default_wine(&self.s);
                let dname = vs.iter().find(|v| v.path == d).map(|v| v.name.clone()).unwrap_or_else(|| "none installed".into());
                self.versions.push((format!("Default ({dname})"), String::new()));
                vs
            }
            _ => vec![],
        };
        for v in vs {
            self.versions.push((v.name, v.path));
        }
        let cur = if initial || runner == self.e.runner { self.e.version.clone() } else { String::new() };
        self.version = String::new();
        if !cur.is_empty() && runner != "native" {
            if !self.versions.iter().any(|(_, p)| *p == cur) {
                self.versions.push((format!("{} (missing)", util::basename(&cur)), cur.clone()));
            }
            self.version = cur;
        }
    }

    fn prefix_hint(&self) -> String {
        let mut tmp = self.e.clone();
        tmp.name = if self.name.trim().is_empty() { "game".into() } else { self.name.trim().to_string() };
        tmp.prefix = String::new();
        format!("{}  (default)", runners::prefix_of(&tmp, &self.s))
    }

    fn exe_changed(&mut self) {
        let exe = self.exe.trim().to_string();
        if exe.is_empty() {
            return;
        }
        if self.name.trim().is_empty() && isfile(&exe) {
            self.name = util::guess_name(&exe);
            self.dirty = true;
        }
        if isfile(&exe) {
            let want = if is_windows(&exe) { self.s.windows_runner.clone() } else { "native".into() };
            if (want == "native") != (self.runner == "native") {
                self.set_runner(&want, false);
            }
            if is_windows(&exe) && self.e.icon.is_empty() && self.icon_src.is_none() && !self.icon_cleared {
                self.icon_src = Some(IconSrc::Exe(exe.clone()));
                self.icon_img = files::exe_icon(&exe).and_then(|d| imaging::decode(&d));
                self.dirty = true;
            }
            if self.new && !self.vr && runners::looks_vr(&exe) {
                self.vr = true;
            }
            if self.new && is_windows(&exe) {
                let (eac, be) = runners::looks_anticheat(&exe);
                self.eac |= eac;
                self.battleye |= be;
            }
        }
    }

    fn update_previews(&mut self, ctx: &egui::Context) {
        let name = if self.name.trim().is_empty() { "?".to_string() } else { self.name.trim().to_string() };
        if !self.dirty && self.preview_for == name {
            return;
        }
        self.dirty = false;
        self.preview_for = name.clone();
        let ppp = ctx.pixels_per_point();
        let (cw, ch) = ((180.0 * ppp) as u32, (270.0 * ppp) as u32);
        let icon = if self.icon_cleared { None } else { self.icon_img.as_ref() };
        let cover = if self.cover_cleared && self.cover_data.is_none() { None } else { self.cover_img.as_ref() };
        let (img, ov) = match cover {
            Some(c) => (imaging::cover(c, cw, ch), Overlay::None),
            None => imaging::placeholder(&name, cw, ch, icon),
        };
        self.cover_still = Some(Still::new(ctx, "edit-cover", &img, ov));
        let s = (40.0 * ppp) as u32;
        let (img, ov) = match icon {
            Some(i) => (imaging::fit(i, s, s, true), Overlay::None),
            None => imaging::placeholder(&name, s, s, None),
        };
        self.icon_still = Some(Still::new(ctx, "edit-icon", &img, ov));
    }

    fn warn(&mut self, title: &str, text: &str) {
        self.dialogs.push(Dialog::new(title, text));
    }

    fn browse_exe(&mut self, ctx: &egui::Context) {
        let start = if isfile(&self.exe) { dirname(&self.exe) } else if !self.start_dir.is_empty() { self.start_dir.clone() } else { util::home() };
        let opts = PickOpts::file("Choose the program", &start)
            .filter("Programs", &["exe", "msi", "bat", "lnk", "AppImage", "appimage", "sh", "x86_64", "x86", "bin", "EXE"]);
        self.pick = Some(Pick::start(opts, ctx, |d: &mut EditDialog, path| {
            d.exe = path;
            d.exe_changed();
        }));
    }

    fn choose_cover(&mut self, ctx: &egui::Context) {
        let opts = PickOpts::file("Choose a cover (portrait works best)", &util::home()).filter("Images", IMAGE_EXTS);
        self.pick = Some(Pick::start(opts, ctx, |d: &mut EditDialog, path| {
            if let Ok(data) = std::fs::read(&path) {
                d.set_cover(data);
            }
        }));
    }

    fn set_cover(&mut self, data: Vec<u8>) {
        if let Some(img) = imaging::decode(&data) {
            self.cover_img = Some(img);
            self.cover_data = Some(data);
            self.cover_cleared = false;
            self.dirty = true;
        } else {
            self.warn("Cover", "That file isn't a picture Vessel can read.");
        }
    }

    fn icon_from_exe(&mut self) {
        let exe = self.exe.trim().to_string();
        match files::exe_icon(&exe).filter(|_| isfile(&exe)).and_then(|d| imaging::decode(&d)) {
            Some(img) => {
                self.icon_img = Some(img);
                self.icon_src = Some(IconSrc::Exe(exe));
                self.icon_cleared = false;
                self.dirty = true;
            }
            None => self.warn("No icon", "This program has no icon to take (only Windows .exe files do)."),
        }
    }

    fn choose_icon(&mut self, ctx: &egui::Context) {
        let opts = PickOpts::file("Choose an icon", &util::home()).filter("Images", IMAGE_EXTS);
        self.pick = Some(Pick::start(opts, ctx, |d: &mut EditDialog, path| {
            if let Some(img) = imaging::load(&path) {
                d.icon_img = Some(img);
                d.icon_src = Some(IconSrc::File(path));
                d.icon_cleared = false;
                d.dirty = true;
            }
        }));
    }

    /// checks what's typed and saves it; true when saved
    fn save(&mut self) -> bool {
        let name = self.name.trim().to_string();
        let exe = util::expanduser(self.exe.trim());
        if name.is_empty() {
            self.warn("Name missing", "Give it a name.");
            return false;
        }
        if exe.is_empty() {
            self.warn("Program missing", "Pick the program to run.");
            return false;
        }
        if self.runner != "native" && !isfile(&exe) {
            self.warn("Program not found", &format!("{exe} doesn't exist."));
            return false;
        }
        if self.runner == "native" && !isfile(&exe) {
            let first = util::shlex_split(&exe).ok().and_then(|w| w.into_iter().next()).unwrap_or_default();
            if !(!first.is_empty() && (isfile(&first) || runners::which(&first).is_some())) {
                let f = if first.is_empty() { exe.clone() } else { first };
                self.warn("Program not found", &format!("{exe} isn't a file, and there's no command called {f}."));
                return false;
            }
        }
        if let Err(ex) = runners::parse_launch_options(&self.args) {
            self.warn("Launch options", &ex.0);
            return false;
        }
        let e = &mut self.e;
        e.name = name;
        e.exe = exe.clone();
        e.args = self.args.trim().to_string();
        e.runner = self.runner.clone();
        e.vr = self.vr;
        e.kind = self.kind.clone();
        e.version = if self.runner != "native" { self.version.clone() } else { String::new() };
        e.prefix = self.prefix.trim().to_string();
        e.cwd = self.cwd.trim().to_string();
        e.wrapper = self.wrapper.trim().to_string();
        e.env = self.env.trim().to_string();
        e.mangohud = self.mangohud;
        e.gamemode = self.gamemode;
        e.eac = self.eac;
        e.battleye = self.battleye;
        e.favorite = self.favorite;
        e.nsfw = self.nsfw;
        e.hidden = self.hidden;
        if self.runner != "native" && e.prefix.is_empty() {
            // fixed at creation, so renaming it or changing the prefixes folder later keeps its saves
            let mut tmp = e.clone();
            tmp.prefix = String::new();
            e.prefix = runners::prefix_of(&tmp, &self.s);
        }
        let mut cover_err = None;
        if let Some(data) = &self.cover_data {
            match imaging::save_cover(&e.id, data) {
                Ok(n) => {
                    let old = std::mem::replace(&mut e.cover, n);
                    store::remove_file(&store::covers_dir(), &old);
                }
                Err(x) => cover_err = Some(x),
            }
        } else if self.cover_cleared {
            store::remove_file(&store::covers_dir(), &e.cover);
            e.cover = String::new();
        }
        let old_icon = e.icon.clone();
        if self.icon_cleared {
            e.icon = String::new();
        } else if let Some(src) = &self.icon_src {
            let made = match src {
                IconSrc::Exe(p) => imaging::save_exe_icon(&e.id, p),
                IconSrc::File(p) => imaging::save_icon_file(&e.id, p),
            };
            if !made.is_empty() {
                e.icon = made;
            }
        } else if self.new && is_windows(&exe) {
            e.icon = imaging::save_exe_icon(&e.id, &exe);
        }
        if e.icon != old_icon {
            store::remove_file(&store::icons_dir(), &old_icon);
        }
        store::put_entry(e);
        shortcuts::refresh_shortcuts(e);
        if self.desktop != shortcuts::has_shortcut(e, true) {
            if self.desktop {
                if let Err(x) = shortcuts::make_shortcut(e, true) {
                    eprintln!("vessel: couldn't make the desktop shortcut: {x}");
                }
            } else {
                shortcuts::remove_shortcut(e, true);
            }
        }
        if let Some(x) = cover_err {
            eprintln!("vessel: the cover couldn't be saved: {x}");
        }
        true
    }

    /// draws it: Some(true) once saved, Some(false) when cancelled
    pub fn show(&mut self, ctx: &egui::Context, t: &Theme) -> Option<bool> {
        poll_pick(self, |d| &mut d.pick);
        self.update_previews(ctx);
        let mut out = None;
        let busy = !self.dialogs.is_empty() || self.covers.is_some() || self.pick.is_some();
        let size = ctx.content_rect().size();
        let w = 760f32.min(size.x - 30.0);
        let h = 660f32.min(size.y - 30.0);
        let title = self.title();
        egui::Modal::new(Id::unique("edit-dialog"))
            .frame(super::panel_frame(t).fill(t.bg).inner_margin(egui::Margin { left: 18, right: 18, top: 16, bottom: 14 }))
            .show(ctx, |ui| {
                ui.set_width(w - 36.0);
                ui.set_height(h - 30.0);
                super::label(ui, &title, super::bold(18.0), t.text);
                ui.add_space(4.0);
                let body_h = ui.available_height() - 44.0;
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 18.0;
                    ui.vertical(|ui| {
                        ui.set_width(180.0);
                        self.left_column(ui, t);
                    });
                    ui.vertical(|ui| {
                        egui::ScrollArea::vertical().max_height(body_h).auto_shrink([false, false]).show(ui, |ui| {
                            ui.set_width(ui.available_width() - 10.0);
                            self.form(ui, t);
                        });
                    });
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Max), |ui| {
                    if Btn::new(if self.new { "Add" } else { "Save" }).primary().min_width(80.0).show(ui, t).clicked() && self.save() {
                        out = Some(true);
                    }
                    if Btn::new("Cancel").min_width(80.0).show(ui, t).clicked() {
                        out = Some(false);
                    }
                });
            });
        if let Some(c) = &mut self.covers {
            match c.show(ctx, t) {
                Some(true) => {
                    let data = c.picked.take();
                    self.covers = None;
                    if let Some(d) = data {
                        self.set_cover(d);
                    }
                }
                Some(false) => self.covers = None,
                None => {}
            }
        }
        show_stack(self, |d| &mut d.dialogs, ctx, t);
        if out.is_none() && !busy && ctx.input(|i| i.key_pressed(Key::Escape)) {
            out = Some(false);
        }
        out
    }

    fn left_column(&mut self, ui: &mut egui::Ui, t: &Theme) {
        let ctx = ui.ctx().clone();
        let (rect, _) = ui.allocate_exact_size(vec2(180.0, 270.0), egui::Sense::hover());
        if let Some(s) = &self.cover_still {
            s.paint(ui, rect, 12.0, &self.preview_for);
        }
        let full = |b: Btn| b.min_width(180.0);
        if full(Btn::new("Find online…")).tip("Search the Steam store / SteamGridDB").show(ui, t).clicked() {
            self.covers = Some(CoverSearch::new(&ctx, self.name.trim()));
        }
        if full(Btn::new("Choose image…")).show(ui, t).clicked() {
            self.choose_cover(&ctx);
        }
        if full(Btn::new("No cover")).show(ui, t).clicked() {
            self.cover_data = None;
            self.cover_img = None;
            self.cover_cleared = true;
            self.dirty = true;
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(40.0, 40.0), egui::Sense::hover());
            if let Some(s) = &self.icon_still {
                s.paint(ui, Rect::from_min_size(rect.min, rect.size()), 8.0, &self.preview_for);
            }
            let r = Btn::new("Icon ▾").kind(super::Kind::Tool).size(13.0).show(ui, t);
            egui::Popup::menu(&r).show(|ui| {
                if ui.button("Take it from the .exe").clicked() {
                    self.icon_from_exe();
                }
                if ui.button("Choose image…").clicked() {
                    self.choose_icon(&ctx);
                }
                if ui.button("No icon").clicked() {
                    self.icon_src = None;
                    self.icon_img = None;
                    self.icon_cleared = true;
                    self.dirty = true;
                }
            });
        });
        ui.add_space(10.0);
        super::muted(ui, t, "Make shortcut");
        let text = if self.desktop { "On the desktop ✓" } else { "Add to desktop" };
        if full(Btn::new(text)).tip("A launcher for this on your desktop; it's made or removed when you save").show(ui, t).clicked() {
            self.desktop = !self.desktop;
        }
    }

    fn form(&mut self, ui: &mut egui::Ui, t: &Theme) {
        let ctx = ui.ctx().clone();
        let lw = 118.0;
        let full = f32::INFINITY;
        form_row(ui, t, lw, "Name", |ui| {
            let r = ui.add(field(&mut self.name, "Name").desired_width(full));
            if self.focus_name {
                r.request_focus();
                self.focus_name = false;
            }
            if r.changed() {
                self.dirty = true;
            }
        });
        form_row(ui, t, lw, "Shows in", |ui| {
            let opts: Vec<(&str, String)> = KINDS.iter().map(|(k, v)| (*k, v.to_string())).collect();
            if let Some(k) = segments(ui, t, &opts, &self.kind) {
                self.kind = k;
            }
        });
        let mut browse = false;
        form_row(ui, t, lw, "Runs", |ui| {
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    browse = Btn::new("Browse…").show(ui, t).clicked();
                    let r = ui.add(field(&mut self.exe, "Program file, or a command (flatpak run …)").desired_width(ui.available_width()));
                    if r.has_focus() {
                        self.exe_had_focus = true;
                    } else if self.exe_had_focus {
                        self.exe_had_focus = false;
                        self.exe_changed();
                    }
                });
            });
        });
        if browse {
            self.browse_exe(&ctx);
        }
        form_row(ui, t, lw, "Launch options", |ui| {
            ui.add(field(&mut self.args, "Optional, like Steam: -windowed, or VAR=1 %command% -vr").desired_width(full))
                .on_hover_text("Works like Steam's launch options. Words after the game are arguments. With %command%, \
                                VAR=value words before it go into the environment and any other words run in front of \
                                the game (e.g. gamescope -f --).");
        });
        form_row(ui, t, lw, "Run with", |ui| {
            let opts: Vec<(&str, String)> = RUNNERS.iter().map(|(k, v)| (*k, v.to_string())).collect();
            if let Some(r) = segments(ui, t, &opts, &self.runner) {
                if r != self.runner {
                    self.set_runner(&r, false);
                }
            }
        });
        let windows = self.runner != "native";
        if windows {
            form_row(ui, t, lw, "Version", |ui| {
                let current = self.versions.iter().find(|(_, p)| *p == self.version).map(|(n, _)| n.clone()).unwrap_or_default();
                egui::ComboBox::from_id_salt("edit-version").selected_text(current).width(ui.available_width() - 4.0)
                    .show_ui(ui, |ui| {
                        for (name, path) in &self.versions {
                            ui.selectable_value(&mut self.version, path.clone(), name);
                        }
                    });
            });
            let hint = self.prefix_hint();
            let mut browse = false;
            form_row(ui, t, lw, "Prefix", |ui| {
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        browse = Btn::new("Browse…").show(ui, t).clicked();
                        ui.add(field(&mut self.prefix, &hint).desired_width(ui.available_width()));
                    });
                });
            });
            if browse {
                let start = if self.prefix.is_empty() { util::expanduser(&self.s.prefix_root) } else { self.prefix.clone() };
                self.pick = Some(Pick::start(PickOpts::folder("Choose the prefix folder", &start), &ctx,
                                             |d: &mut EditDialog, p| d.prefix = p));
            }
        }
        let mut browse = false;
        form_row(ui, t, lw, "Start in", |ui| {
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    browse = Btn::new("Browse…").show(ui, t).clicked();
                    ui.add(field(&mut self.cwd, "Optional; the program's own folder by default").desired_width(ui.available_width()));
                });
            });
        });
        if browse {
            let start = if self.cwd.is_empty() { util::home() } else { self.cwd.clone() };
            self.pick = Some(Pick::start(PickOpts::folder("Start in", &start), &ctx, |d: &mut EditDialog, p| d.cwd = p));
        }
        form_row(ui, t, lw, "Command prefix", |ui| {
            ui.add(field(&mut self.wrapper, "Optional, e.g. gamescope -f -W 2560 -H 1440 --").desired_width(full));
        });
        form_row(ui, t, lw, "Environment", |ui| {
            ui.add(egui::TextEdit::multiline(&mut self.env)
                .hint_text("Optional, one per line, e.g.\nDXVK_HUD=fps\nPROTON_ENABLE_WAYLAND=1")
                .margin(egui::Margin::symmetric(10, 6))
                .desired_rows(4)
                .desired_width(full));
        });
        form_row(ui, t, lw, "Options", |ui| {
            egui::Grid::new("edit-options").num_columns(3).spacing(vec2(18.0, 6.0)).show(ui, |ui| {
                check(ui, t, &mut self.vr, "VR game", true).on_hover_text(
                    "Sets up WiVRn (OpenXR) and xrizer (OpenVR) for it, starts the WiVRn server if it isn't running, and \
                     adds the VR launch options from Settings");
                let r = check(ui, t, &mut self.mangohud, "MangoHud", true);
                if !self.has_mangohud {
                    r.on_hover_text("mangohud isn't installed");
                }
                let gm_ok = self.has_gamemode || self.gamemode;
                let r = check(ui, t, &mut self.gamemode, "GameMode", gm_ok);
                if !self.has_gamemode {
                    r.on_hover_text("gamemoderun isn't installed");
                }
                ui.end_row();
                check(ui, t, &mut self.favorite, "Favourite", true);
                check(ui, t, &mut self.nsfw, "NSFW", true);
                check(ui, t, &mut self.hidden, "Hidden", true);
                ui.end_row();
                check(ui, t, &mut self.eac, "EasyAntiCheat", windows).on_hover_text(&self.anticheat_tips[0]);
                check(ui, t, &mut self.battleye, "BattlEye", windows).on_hover_text(&self.anticheat_tips[1]);
                ui.end_row();
            });
        });
    }
}
