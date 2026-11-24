//! Vessel's settings.

use super::dialogs::{Dialog, Pick, PickOpts, poll_pick, show_stack};
use super::{Btn, Kind, THEMES, Theme, check, field, form_row, h2, muted, muted_wrap, panel_frame, progress};
use crate::files::Progress;
use crate::net::{self, PROTON_SOURCES, UpdateInfo};
use crate::runners;
use crate::shortcuts;
use crate::store::{self, Settings, VERSION};
use crate::task::Job;
use crate::util;
use egui::{Color32, Id, Key, Modifiers, vec2};
use serde_json::Value;

pub const SIZES: &[(&str, f32)] = &[("small", 140.0), ("medium", 172.0), ("large", 214.0)];

pub fn tray_available() -> bool {
    (|| -> zbus::Result<bool> {
        let c = zbus::blocking::Connection::session()?;
        let reply = c.call_method(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus", Some("org.freedesktop.DBus"),
                                  "NameHasOwner", &("org.kde.StatusNotifierWatcher",))?;
        reply.body().deserialize::<bool>()
    })()
    .unwrap_or(false)
}

// ---- the NSFW tab's key combination, written the way Qt wrote it ("Ctrl+Shift+N") ----------------

pub fn parse_shortcut(s: &str) -> Option<egui::KeyboardShortcut> {
    let parts: Vec<&str> = s.split('+').map(|p| p.trim()).filter(|p| !p.is_empty()).collect();
    let (last, mods) = parts.split_last()?;
    let mut m = Modifiers::NONE;
    for p in mods {
        match p.to_lowercase().as_str() {
            "ctrl" | "control" => m.ctrl = true,
            "alt" => m.alt = true,
            "shift" => m.shift = true,
            "meta" | "super" | "win" => m.mac_cmd = true,
            _ => return None,
        }
    }
    let key = Key::from_name(last).or_else(|| Key::from_name(&last.to_uppercase()))?;
    Some(egui::KeyboardShortcut::new(m, key))
}

pub fn shortcut_text(m: Modifiers, key: Key) -> String {
    let mut out = String::new();
    if m.ctrl || m.command && !m.mac_cmd {
        out.push_str("Ctrl+");
    }
    if m.alt {
        out.push_str("Alt+");
    }
    if m.shift {
        out.push_str("Shift+");
    }
    if m.mac_cmd {
        out.push_str("Meta+");
    }
    out.push_str(key.name());
    out
}

pub struct SettingsDialog {
    s: Settings,
    theme: String,
    accent: String,
    size: String,
    close_to_tray: bool,
    tray_ok: bool,
    win_runner: String,
    proton: String,
    protons: Vec<(String, String)>,
    wine: String,
    wines: Vec<(String, String)>,
    prefix_root: String,
    wayland: String,
    exe_opener: bool,
    use_umu: bool,
    has_umu: bool,
    winetricks: Option<String>,
    wt_note: String,
    wt_job: Option<Job<String>>,
    show_nsfw: bool,
    blur_nsfw: bool,
    hide_nsfw: bool,
    nsfw_key: String,
    capturing: bool,
    games_root: String,
    vr_options: String,
    start_wivrn: bool,
    has_wivrn: bool,
    vr_note: String,
    sgdb: String,
    proton_note: String,
    proton_job: Option<(String, Job<String>)>,
    update_note: String,
    update_info: Option<UpdateInfo>,
    update_ready: bool,
    check_job: Option<Job<UpdateInfo>>,
    update_job: Option<Job<String>>,
    pub scroll_to_bottom: bool,
    /// Restart Vessel was clicked
    pub restart: bool,
    pick: Option<Pick<SettingsDialog>>,
    dialogs: Vec<Dialog<SettingsDialog>>,
    accent_color: Color32,
}

impl SettingsDialog {
    pub fn new() -> Self {
        let s = store::settings();
        let xr = runners::xrizer_dir();
        let wj = runners::wivrn_json().into_iter().any(|j| util::isfile(&j));
        let has_wivrn = runners::which("wivrn-server").is_some();
        let vr_note = format!("WiVRn: {}{} · OpenXR runtime: {} · OpenVR games: {}",
            if has_wivrn { "installed" } else { "not installed" },
            if runners::wivrn_running() { ", running" } else { "" },
            if wj { "WiVRn" } else { "not found" },
            if xr.is_empty() { "no xrizer/OpenComposite found".into() } else { format!("xrizer ({xr})") });
        let winetricks = runners::which("winetricks");
        let mut d = SettingsDialog {
            theme: if THEMES.iter().any(|(n, _)| *n == s.theme) { s.theme.clone() } else { "Midnight".into() },
            accent: s.accent.clone(),
            size: s.size.clone(),
            close_to_tray: s.close_to_tray,
            tray_ok: tray_available(),
            win_runner: s.windows_runner.clone(),
            proton: String::new(),
            protons: vec![],
            wine: String::new(),
            wines: vec![],
            prefix_root: s.prefix_root.clone(),
            wayland: s.wayland.clone(),
            exe_opener: s.exe_opener,
            use_umu: s.use_umu,
            has_umu: runners::which("umu-run").is_some(),
            wt_note: format!("Winetricks: {}", winetricks.clone().unwrap_or_else(|| "not installed".into())),
            winetricks,
            wt_job: None,
            show_nsfw: s.show_nsfw,
            blur_nsfw: s.blur_nsfw,
            hide_nsfw: s.nsfw_hidden,
            nsfw_key: if s.nsfw_key.is_empty() { "Ctrl+Shift+N".into() } else { s.nsfw_key.clone() },
            capturing: false,
            games_root: s.games_root.clone(),
            vr_options: s.vr_options.clone(),
            start_wivrn: s.start_wivrn,
            has_wivrn,
            vr_note,
            sgdb: s.sgdb_key.clone(),
            proton_note: format!("Goes into {}", runners::proton_install_dir()),
            proton_job: None,
            update_note: String::new(),
            update_info: None,
            update_ready: false,
            check_job: None,
            update_job: None,
            scroll_to_bottom: false,
            restart: false,
            pick: None,
            dialogs: vec![],
            accent_color: Color32::WHITE,
            s,
        };
        d.fill_runners();
        d
    }

    fn fill_runners(&mut self) {
        self.protons = runners::proton_versions().into_iter().map(|v| (v.name, v.path)).collect();
        if self.protons.is_empty() {
            self.protons.push(("None installed".into(), String::new()));
        }
        let dp = runners::default_proton(&self.s);
        self.proton = if self.protons.iter().any(|(_, p)| *p == dp) { dp } else { self.protons[0].1.clone() };
        self.wines = runners::wine_versions().into_iter().map(|v| (v.name, v.path)).collect();
        if self.wines.is_empty() {
            self.wines.push(("None installed".into(), String::new()));
        }
        let dw = runners::default_wine(&self.s);
        self.wine = if self.wines.iter().any(|(_, p)| *p == dw) { dw } else { self.wines[0].1.clone() };
    }

    /// the theme as it's set up here (shown at once, kept with Save)
    pub fn preview_theme(&self) -> Theme {
        super::theme_named(&self.theme, &self.accent)
    }

    pub fn check_updates(&mut self, ctx: &egui::Context) {
        if self.update_ready {
            self.restart = true;
            return;
        }
        if let Some(info) = self.update_info.clone().filter(|i| i.newer && !i.url.is_empty() && crate::appimage().is_some()) {
            self.update_job = Some(Job::start(ctx, move |report| net::install_update(&info, report)));
            return;
        }
        self.update_note = "Checking GitHub…".into();
        self.check_job = Some(Job::start(ctx, |_| net::check_update()));
    }

    fn poll_jobs(&mut self) {
        if let Some((source, job)) = &mut self.proton_job {
            let label = PROTON_SOURCES.iter().find(|p| p.key == source).map(|p| p.label).unwrap_or("Proton");
            match &job.last {
                Some(Progress::Download(done, total, tag)) if *total > 0 => {
                    self.proton_note = format!("Downloading {tag}: {} / {} MB", done >> 20, total >> 20)
                }
                Some(Progress::Unpack(tag)) => self.proton_note = format!("Unpacking {tag}…"),
                _ => {}
            }
            if let Some(r) = job.poll() {
                let source = source.clone();
                self.proton_job = None;
                match r {
                    Err(e) => self.proton_note = format!("Couldn't get {label}: {e}"),
                    Ok(tag) if source != "ge" => {
                        // only GE-Proton becomes the default; others are picked per game (or here)
                        self.fill_runners();
                        self.proton_note = format!("{tag} is installed: pick it for a game in its Proton version list, or as the default above.");
                    }
                    Ok(tag) => {
                        self.proton_note = format!("{tag} is installed and is now the default.");
                        self.s.proton = String::new();
                        self.fill_runners();
                        if let Some((_, p)) = self.protons.iter().find(|(n, _)| *n == tag) {
                            self.proton = p.clone();
                        }
                    }
                }
            }
        }
        if let Some(job) = &mut self.wt_job {
            if let Some(r) = job.poll() {
                self.wt_job = None;
                match r {
                    Ok(path) => {
                        self.wt_note = format!("Winetricks: {path}");
                        self.winetricks = Some(path);
                    }
                    Err(e) => self.wt_note = format!("Download failed: {e}"),
                }
            }
        }
        if let Some(job) = &mut self.check_job {
            if let Some(r) = job.poll() {
                self.check_job = None;
                match r {
                    Err(e) => self.update_note = format!("Couldn't check for updates: {e}"),
                    Ok(info) => {
                        self.update_note = if !info.newer {
                            format!("Vessel {VERSION} is the newest version.")
                        } else if !info.url.is_empty() && crate::appimage().is_some() {
                            format!("Vessel {} is out.", info.version)
                        } else {
                            format!("Vessel {} is out: get it from the release page.", info.version)
                        };
                        self.update_info = Some(info);
                    }
                }
            }
        }
        if let Some(job) = &mut self.update_job {
            if let Some(Progress::Download(done, total, ver)) = &job.last {
                self.update_note = format!("Downloading Vessel {ver}: {} / {} MB", done >> 20, total >> 20);
            }
            if let Some(r) = job.poll() {
                self.update_job = None;
                match r {
                    Err(e) => self.update_note = format!("Couldn't update: {e}"),
                    Ok(v) => {
                        self.update_ready = true;
                        self.update_note = format!("Vessel {v} is installed. Restart Vessel to use it (unsaved settings here are lost).");
                    }
                }
            }
        }
    }

    // NSFW password: changed at once (the old one is asked first), not with Save
    fn ask_old_password(&mut self, then: impl FnOnce(&mut SettingsDialog) + 'static) {
        let rec = store::settings().nsfw_pass;
        if !store::settings().has_password() {
            then(self);
            return;
        }
        self.dialogs.push(Dialog::new("NSFW password", "Current password:").input("Password", true).then(move |d: &mut SettingsDialog, a| {
            if a.button != Some(1) {
                return;
            }
            if store::password_ok(&a.text, &rec) {
                then(d);
            } else {
                d.dialogs.push(Dialog::new("NSFW password", "That's not the password."));
            }
        }));
    }

    fn set_password(&mut self) {
        self.ask_old_password(|d| {
            d.dialogs.push(Dialog::new("NSFW password", "New password:").input("Password", true).then(|d: &mut SettingsDialog, a| {
                if a.button != Some(1) || a.text.is_empty() {
                    return;
                }
                let pw = a.text;
                d.dialogs.push(Dialog::new("NSFW password", "The same password again:").input("Password", true).then(
                    move |d: &mut SettingsDialog, a| {
                        if a.button != Some(1) {
                            return;
                        }
                        if a.text != pw {
                            d.dialogs.push(Dialog::new("NSFW password", "The two didn't match, so the password wasn't changed."));
                            return;
                        }
                        store::save_settings(&[("nsfw_pass", store::hash_password(&pw))]);
                    }));
            }));
        });
    }

    fn save(&self) {
        store::save_settings(&[
            ("nsfw_hidden", self.hide_nsfw.into()),
            ("nsfw_key", Value::String(if self.nsfw_key.is_empty() { "Ctrl+Shift+N".into() } else { self.nsfw_key.clone() })),
            ("theme", self.theme.clone().into()),
            ("accent", self.accent.clone().into()),
            ("size", self.size.clone().into()),
            ("windows_runner", self.win_runner.clone().into()),
            ("proton", self.proton.clone().into()),
            ("wine", self.wine.clone().into()),
            ("prefix_root", Value::String(if self.prefix_root.trim().is_empty() { store::default_prefix_root() } else { self.prefix_root.trim().into() })),
            ("use_umu", self.use_umu.into()),
            ("show_nsfw", self.show_nsfw.into()),
            ("vr_options", self.vr_options.trim().to_string().into()),
            ("start_wivrn", self.start_wivrn.into()),
            ("wayland", self.wayland.clone().into()),
            ("exe_opener", self.exe_opener.into()),
            ("close_to_tray", self.close_to_tray.into()),
            ("blur_nsfw", self.blur_nsfw.into()),
            ("sgdb_key", self.sgdb.trim().to_string().into()),
            ("games_root", Value::String(if self.games_root.trim().is_empty() { store::default_games_root() } else { self.games_root.trim().into() })),
        ]);
        if self.exe_opener != self.s.exe_opener {
            if let Err(e) = shortcuts::register_opener(self.exe_opener) {
                eprintln!("vessel: couldn't set up opening .exe files: {e}");
            }
        }
    }

    /// draws it: Some(true) once saved, Some(false) when cancelled
    pub fn show(&mut self, ctx: &egui::Context, t: &Theme) -> Option<bool> {
        self.poll_jobs();
        poll_pick(self, |d| &mut d.pick);
        if self.capturing {
            let got = ctx.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Key { key, pressed: true, modifiers, .. } => Some((*key, *modifiers)),
                    _ => None,
                })
            });
            if let Some((key, m)) = got {
                if key != Key::Escape {
                    self.nsfw_key = shortcut_text(m, key);
                }
                self.capturing = false;
            }
        }
        let mut out = None;
        let size = ctx.content_rect().size();
        let w = 680f32.min(size.x - 30.0);
        let h = 680f32.min(size.y - 30.0);
        let busy = !self.dialogs.is_empty() || self.pick.is_some() || self.capturing;
        egui::Modal::new(Id::unique("settings-dialog"))
            .frame(panel_frame(t).fill(t.bg).inner_margin(egui::Margin { left: 18, right: 18, top: 16, bottom: 14 }))
            .show(ctx, |ui| {
                ui.set_width(w - 36.0);
                ui.set_height(h - 30.0);
                super::label(ui, "Vessel settings", super::bold(18.0), t.text);
                let body_h = ui.available_height() - 44.0;
                let mut sa = egui::ScrollArea::vertical().max_height(body_h).auto_shrink([false, false]);
                if self.scroll_to_bottom {
                    sa = sa.vertical_scroll_offset(1e6);
                    self.scroll_to_bottom = false;
                }
                sa.show(ui, |ui| {
                    ui.set_width(ui.available_width() - 10.0);
                    ui.spacing_mut().item_spacing.y = 10.0;
                    self.panels(ui, t);
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Max), |ui| {
                    if Btn::new("Save").primary().min_width(80.0).show(ui, t).clicked() {
                        self.save();
                        out = Some(true);
                    }
                    if Btn::new("Cancel").min_width(80.0).show(ui, t).clicked() {
                        out = Some(false);
                    }
                });
            });
        show_stack(self, |d| &mut d.dialogs, ctx, t);
        if out.is_none() && !busy && ctx.input(|i| i.key_pressed(Key::Escape)) {
            out = Some(false);
        }
        if self.restart {
            out = Some(false);
        }
        out
    }

    fn panels(&mut self, ui: &mut egui::Ui, t: &Theme) {
        let ctx = ui.ctx().clone();
        let lw = 200.0;
        let fullw = f32::INFINITY;
        let combo_w = |ui: &egui::Ui| ui.available_width() - 4.0;

        panel_frame(t).show(ui, |ui| {
            ui.set_width(ui.available_width());
            h2(ui, t, "Appearance");
            form_row(ui, t, lw, "Theme", |ui| {
                egui::ComboBox::from_id_salt("theme").selected_text(&self.theme).width(combo_w(ui)).show_ui(ui, |ui| {
                    for (name, cols) in THEMES {
                        ui.horizontal(|ui| {
                            let (rect, _) = ui.allocate_exact_size(vec2(28.0, 16.0), egui::Sense::hover());
                            let bg = super::parse_color(cols[0]).unwrap();
                            let border = super::parse_color(cols[3]).unwrap();
                            let acc = super::parse_color(cols[6]).unwrap();
                            ui.painter().rect(rect, 4.0, bg, egui::Stroke::new(1.0, border), egui::StrokeKind::Inside);
                            ui.painter().circle_filled(egui::pos2(rect.min.x + 19.0, rect.center().y), 4.0, acc);
                            ui.selectable_value(&mut self.theme, name.to_string(), *name);
                        });
                    }
                });
            });
            form_row(ui, t, lw, "Accent", |ui| {
                ui.horizontal(|ui| {
                    let theme = self.preview_theme();
                    let mut c = theme.accent;
                    let text = if self.accent.is_empty() { "Pick…".to_string() } else { format!("Pick…  {}", super::hex(c)) };
                    let r = Btn::new(text).kind(Kind::Primary).min_width(120.0).show(ui, t);
                    egui::Popup::menu(&r).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
                        if egui::color_picker::color_picker_color32(ui, &mut c, egui::color_picker::Alpha::Opaque) {
                            self.accent = super::hex(c);
                        }
                    });
                    self.accent_color = c;
                    if Btn::new("Theme's own").show(ui, t).clicked() {
                        self.accent = String::new();
                    }
                });
            });
            form_row(ui, t, lw, "Grid covers", |ui| {
                let label = |k: &str| { let mut c = k.chars(); c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default() };
                egui::ComboBox::from_id_salt("size").selected_text(label(&self.size)).width(combo_w(ui)).show_ui(ui, |ui| {
                    for (k, _) in SIZES {
                        ui.selectable_value(&mut self.size, k.to_string(), label(k));
                    }
                });
            });
            form_row(ui, t, lw, "", |ui| {
                let tip = format!("The X hides Vessel to the system tray (click its icon there to open it again, or right-click → Quit). {}",
                                  if self.tray_ok { "" } else { "No system tray was found, so the X quits for now." });
                check(ui, t, &mut self.close_to_tray, "Closing the window keeps Vessel running in the tray", true).on_hover_text(tip);
            });
        });

        panel_frame(t).show(ui, |ui| {
            ui.set_width(ui.available_width());
            h2(ui, t, "Running Windows programs");
            form_row(ui, t, lw, "New .exe files use", |ui| {
                egui::ComboBox::from_id_salt("winrunner").selected_text(runners::runner_name(&self.win_runner)).width(combo_w(ui))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.win_runner, "proton".into(), "Proton");
                        ui.selectable_value(&mut self.win_runner, "wine".into(), "Wine");
                    });
            });
            form_row(ui, t, lw, "Default Proton", |ui| {
                let cur = self.protons.iter().find(|(_, p)| *p == self.proton).map(|(n, _)| n.clone()).unwrap_or_default();
                egui::ComboBox::from_id_salt("proton").selected_text(cur).width(combo_w(ui)).show_ui(ui, |ui| {
                    for (n, p) in &self.protons {
                        ui.selectable_value(&mut self.proton, p.clone(), n);
                    }
                });
            });
            form_row(ui, t, lw, "", |ui| {
                ui.horizontal(|ui| {
                    let idle = self.proton_job.is_none();
                    for src in PROTON_SOURCES {
                        let mut b = Btn::new(format!("Get the newest {}", src.label)).enabled(idle);
                        if src.key == "dw" {
                            b = b.tip("DWProton (An Anime Team, built on Proton-CachyOS): fixes for anime games");
                        }
                        if b.show(ui, t).clicked() {
                            self.proton_note = "Looking for the newest release…".into();
                            let key = src.key.to_string();
                            let k2 = key.clone();
                            self.proton_job = Some((key, Job::start(&ctx, move |report| net::install_proton(&k2, report))));
                        }
                    }
                    if let Some((_, job)) = &self.proton_job {
                        let frac = match &job.last {
                            Some(Progress::Unpack(_)) => None,
                            _ => job.fraction(),
                        };
                        progress(ui, t, frac, ui.available_width().max(40.0));
                    }
                });
                muted_wrap(ui, t, &self.proton_note);
            });
            form_row(ui, t, lw, "Default Wine", |ui| {
                let cur = self.wines.iter().find(|(_, p)| *p == self.wine).map(|(n, _)| n.clone()).unwrap_or_default();
                egui::ComboBox::from_id_salt("wine").selected_text(cur).width(combo_w(ui)).show_ui(ui, |ui| {
                    for (n, p) in &self.wines {
                        ui.selectable_value(&mut self.wine, p.clone(), n);
                    }
                });
            });
            let mut browse = false;
            form_row(ui, t, lw, "New prefixes go in", |ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    browse = Btn::new("Browse…").show(ui, t).clicked();
                    ui.add(field(&mut self.prefix_root, "").desired_width(ui.available_width()));
                });
            });
            if browse {
                let start = util::expanduser(&self.prefix_root);
                self.pick = Some(Pick::start(PickOpts::folder("New prefixes go in", &start), &ctx, |d: &mut SettingsDialog, p| d.prefix_root = p));
            }
            form_row(ui, t, lw, "Proton games as Wayland windows", |ui| {
                let opts = [("auto", "Automatic (on niri only)"), ("on", "Always"), ("off", "Never")];
                let cur = opts.iter().find(|(k, _)| *k == self.wayland).map(|(_, v)| *v).unwrap_or(opts[0].1);
                egui::ComboBox::from_id_salt("wayland").selected_text(cur).width(combo_w(ui)).show_ui(ui, |ui| {
                    for (k, v) in opts {
                        ui.selectable_value(&mut self.wayland, k.to_string(), v);
                    }
                }).response.on_hover_text(
                    "GE-Proton's Wayland mode (PROTON_ENABLE_WAYLAND=1), not for VR games. On niri X11 windows can vanish \
                     on a monitor above or left of the main one, so Automatic uses it there. Elsewhere X11 is better: a \
                     Wayland window can only be resized when the program allows it. For one game, put \
                     PROTON_ENABLE_WAYLAND=0 or 1 in its Environment.");
            });
            form_row(ui, t, lw, "", |ui| {
                check(ui, t, &mut self.exe_opener, "Open .exe files with Vessel (double-click one to run it)", true).on_hover_text(
                    "Vessel becomes the app for Windows programs: opening one from your file manager shows a small window \
                     to run it with Proton or Wine, and add it to the library if you like.");
                let umu = format!("Use umu-launcher for Proton{}", if self.has_umu { "" } else { " (not installed)" });
                check(ui, t, &mut self.use_umu, &umu, true).on_hover_text(
                    "umu runs Proton inside Steam's runtime, like Steam does. Without it, Proton runs directly, which works \
                     for most games. Games that ship Steam's DLL (steam_api.dll) always run directly, so they can reach the \
                     Steam client.");
                ui.horizontal(|ui| {
                    muted(ui, t, &self.wt_note);
                    if self.winetricks.is_none() && Btn::new("Download winetricks").enabled(self.wt_job.is_none()).show(ui, t).clicked() {
                        self.wt_note = "Downloading winetricks…".into();
                        self.wt_job = Some(Job::start(&ctx, |_| net::install_winetricks()));
                    }
                });
            });
        });

        panel_frame(t).show(ui, |ui| {
            ui.set_width(ui.available_width());
            h2(ui, t, "Content");
            form_row(ui, t, lw, "", |ui| {
                check(ui, t, &mut self.show_nsfw, "Show NSFW entries in Library and Favourites too", true);
                check(ui, t, &mut self.blur_nsfw, "Blur NSFW covers (point at one to see it)", true);
                check(ui, t, &mut self.hide_nsfw, "Hide the NSFW tab: only a key combination shows it", true);
            });
            form_row(ui, t, lw, "NSFW tab key", |ui| {
                ui.horizontal(|ui| {
                    let text = if self.capturing { "Press the keys…".to_string() } else { self.nsfw_key.clone() };
                    if Btn::new(text).min_width(160.0).tip("Shows (or hides again) the NSFW tab. Without “Hide the NSFW tab”, it jumps to it")
                        .show(ui, t).clicked() {
                        self.capturing = true;
                    }
                    if Btn::new("Reset").show(ui, t).clicked() {
                        self.nsfw_key = "Ctrl+Shift+N".into();
                        self.capturing = false;
                    }
                });
            });
            form_row(ui, t, lw, "NSFW password", |ui| {
                ui.horizontal(|ui| {
                    let on = store::settings().has_password();
                    muted(ui, t, if on { "On: the NSFW tab asks for it" } else { "Off" });
                    if Btn::new(if on { "Change…" } else { "Set a password…" }).show(ui, t).clicked() {
                        self.set_password();
                    }
                    if on && Btn::new("Remove").show(ui, t).clicked() {
                        self.ask_old_password(|_| store::save_settings(&[("nsfw_pass", Value::Object(Default::default()))]));
                    }
                });
            });
            let mut browse = false;
            form_row(ui, t, lw, "Dropped games go in", |ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    browse = Btn::new("Browse…").show(ui, t).clicked();
                    ui.add(field(&mut self.games_root, "").desired_width(ui.available_width()));
                });
            });
            if browse {
                let start = util::expanduser(&self.games_root);
                self.pick = Some(Pick::start(PickOpts::folder("Dropped games go in", &start), &ctx, |d: &mut SettingsDialog, p| d.games_root = p));
            }
        });

        panel_frame(t).show(ui, |ui| {
            ui.set_width(ui.available_width());
            h2(ui, t, "VR (WiVRn)");
            form_row(ui, t, lw, "VR launch options", |ui| {
                ui.add(field(&mut self.vr_options, "Optional, e.g. PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1 %command%").desired_width(fullw))
                    .on_hover_text("Added to the launch options of every game marked VR (Steam syntax)");
            });
            form_row(ui, t, lw, "", |ui| {
                let ok = self.has_wivrn || self.s.start_wivrn;
                check(ui, t, &mut self.start_wivrn, "Start the WiVRn server before a VR game if it isn't running", ok);
                muted_wrap(ui, t, &self.vr_note);
            });
        });

        panel_frame(t).show(ui, |ui| {
            ui.set_width(ui.available_width());
            h2(ui, t, "Covers");
            form_row(ui, t, lw, "SteamGridDB key", |ui| {
                ui.add(field(&mut self.sgdb, "Optional").password(true).desired_width(fullw));
            });
            form_row(ui, t, lw, "", |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    muted(ui, t, "For covers of games that aren't on Steam. Get a free key at ");
                    if ui.link("steamgriddb.com").clicked() {
                        runners::open_path("https://www.steamgriddb.com/profile/preferences/api");
                    }
                    muted(ui, t, " (Preferences → API).");
                });
            });
        });

        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let text = if self.update_ready {
                    "Restart Vessel".to_string()
                } else if let Some(i) = self.update_info.as_ref().filter(|i| i.newer && !i.url.is_empty() && crate::appimage().is_some()) {
                    format!("Update to {}", i.version)
                } else {
                    "Check for updates".to_string()
                };
                let idle = self.check_job.is_none() && self.update_job.is_none();
                if Btn::new(text).enabled(idle).show(ui, t).clicked() {
                    self.check_updates(&ctx);
                }
                muted_wrap(ui, t, &format!("Vessel {VERSION} · library in {}", store::conf_dir()));
            });
        });
        if let Some(job) = &self.update_job {
            progress(ui, t, job.fraction(), ui.available_width());
        }
        if !self.update_note.is_empty() {
            ui.horizontal_wrapped(|ui| {
                muted(ui, t, &self.update_note);
                if let Some(i) = &self.update_info {
                    if i.newer && ui.link("release page").clicked() {
                        runners::open_path(&i.page);
                    }
                }
            });
        }
    }
}
