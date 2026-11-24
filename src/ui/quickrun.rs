//! Opening a program with Vessel (from the file manager, or `vessel FILE.exe`): its icon and name, what runs it,
//! and Run. It can go into the library too; a program already there runs as set up. Run turns the window into
//! a loading screen until the program's window shows.

use super::dialogs::{Dialog, show_stack};
use super::pics::Still;
use super::{Btn, Theme, check, muted, muted_wrap};
use crate::files;
use crate::imaging::{self, Overlay};
use crate::runners::{self, is_windows};
use crate::shortcuts;
use crate::store::{self, Entry, Settings};
use crate::task::Job;
use crate::util::{self, dirname, join};
use egui::{Align, Key, Layout, vec2};
use std::process::Child;
use std::sync::mpsc::{Receiver, channel};

struct Loading {
    e: Entry,
    first: bool,
    t0: f64,
    proc: Child,
    log_name: String,
    ready_at: f64,
    checked: f64,
    check: Option<Receiver<Option<bool>>>,
    title: String,
    text: String,
    failed: bool,
}

pub struct QuickRun {
    path: String,
    s: Settings,
    e: Entry,
    known: bool,
    name: String,
    runners: Vec<(String, String, String)>,
    choice: usize,
    mangohud: bool,
    gamemode: bool,
    keep: bool,
    installer: bool,
    icon_img: image::RgbaImage,
    icon: Option<Still>,
    loading: Option<Loading>,
    dialogs: Vec<Dialog<QuickRun>>,
    stopping: Option<Job<String>>,
    t: Theme,
    started: bool,
    ctx: Option<egui::Context>,
    /// Install was clicked: the main window runs the installer
    pub install: bool,
}

impl QuickRun {
    pub fn new(path: &str) -> Self {
        let s = store::settings();
        let (e, known) = shortcuts::quick_entry(path, &s);
        let icon_img = files::exe_icon(path).filter(|_| is_windows(path)).and_then(|d| imaging::decode(&d))
            .map(|img| imaging::fit(&img, 144, 144, true))
            .unwrap_or_else(|| imaging::placeholder(&e.name, 144, 144, None).0);
        let mut runners_list = Vec::new();
        if is_windows(path) {
            for v in runners::proton_versions() {
                runners_list.push((format!("Proton: {}", v.name), "proton".into(), v.path));
            }
            for v in runners::wine_versions() {
                runners_list.push((format!("Wine: {}", v.name), "wine".into(), v.path));
            }
        }
        runners_list.push(("Native (run it as a Linux program)".into(), "native".into(), String::new()));
        // what it's set to (its library entry), else the default Proton/Wine
        let want_v = if !e.version.is_empty() {
            e.version.clone()
        } else {
            match e.runner.as_str() {
                "proton" => runners::default_proton(&s),
                "wine" => runners::default_wine(&s),
                _ => String::new(),
            }
        };
        let choice = runners_list.iter().position(|(_, r, v)| *r == e.runner && (want_v.is_empty() || *v == want_v || r == "native"))
            .unwrap_or(0);
        let installer = is_windows(path) && files::is_installer(path) && !known;
        QuickRun {
            path: path.to_string(),
            name: e.name.clone(),
            mangohud: e.mangohud,
            gamemode: e.gamemode,
            t: super::current_theme(&s),
            s,
            e,
            known,
            runners: runners_list,
            choice,
            keep: false,
            installer,
            icon_img,
            icon: None,
            loading: None,
            dialogs: vec![],
            stopping: None,
            started: false,
            ctx: None,
            install: false,
        }
    }

    /// the window's height for what it shows (fixed: tiling compositors float fixed-size windows)
    pub fn height(&self) -> f32 {
        let mut h = 22.0 + 76.0 + 14.0 + 3.0 * 32.0 + 14.0 + 34.0 + 22.0;
        if self.known {
            h += 4.0;
        }
        if self.installer {
            h += 46.0;
        }
        h
    }

    pub fn title(&self) -> String {
        format!("Run {}", util::basename(&self.path))
    }

    fn chosen(&self) -> Entry {
        let mut e = self.e.clone();
        if !self.known {
            let (_, r, v) = &self.runners[self.choice];
            e.name = if self.name.trim().is_empty() { self.e.name.clone() } else { self.name.trim().to_string() };
            e.runner = r.clone();
            e.version = v.clone();
            e.mangohud = self.mangohud;
            e.gamemode = self.gamemode;
        }
        e
    }

    fn run(&mut self, ctx: &egui::Context) {
        let e = self.chosen();
        if !runners::prefix_procs(&e, &self.s).is_empty() {
            let name = e.name.clone();
            self.dialogs.push(Dialog::new(format!("{name} is still running"), format!("{name} is still running, or what's left of its last run is stuck."))
                .info("It can't start again until that ends: Proton waits for it, so it would never load. Stop it and start again?")
                .buttons(&[("Cancel", super::Kind::Normal), ("Stop it and start", super::Kind::Primary)], Some(1))
                .then(move |q: &mut QuickRun, ans| {
                    if ans.button == Some(1) {
                        let (e2, s) = (e.clone(), q.s.clone());
                        if let Some(ctx) = q.ctx.clone() {
                            q.stopping = Some(Job::start(&ctx, move |_| {
                                runners::stop_prefix(&e2, &s, 4.0);
                                Ok(String::new())
                            }));
                        }
                    }
                }));
            self.ctx = Some(ctx.clone());
            return;
        }
        self.start(e);
    }

    fn start(&mut self, mut e: Entry) {
        let first = e.runner != "native" && !util::isdir(&join(&runners::wine_prefix(&e, &self.s), "drive_c"));
        let started = if self.known || self.keep {
            if !self.known {
                e.id = util::new_id();
                e.icon = if is_windows(&self.path) { imaging::save_exe_icon(&e.id, &self.path) } else { String::new() };
                store::put_entry(&e);
            }
            // through --launch: it keeps the playtime, like a menu shortcut
            let mut cmd = shortcuts::self_command();
            cmd.push("--launch".into());
            cmd.push(e.id.clone());
            let env: runners::Env = std::env::vars().collect();
            runners::detach(&cmd, &env).map(|c| (c, e.id.clone())).map_err(|x| x.to_string())
        } else {
            runners::start_process(&e, &self.s, None, None, false, Some("quick")).map(|c| (c, "quick".to_string())).map_err(|x| x.0)
        };
        match started {
            Err(ex) => self.dialogs.push(Dialog::new("Can't run it", ex)),
            Ok((proc, log_name)) => {
                let proton = e.runner == "proton";
                self.loading = Some(Loading {
                    title: format!("Starting {}", e.name),
                    text: if proton { "Starting Proton…".into() } else { "Starting…".into() },
                    e,
                    first,
                    t0: util::now(),
                    proc,
                    log_name,
                    ready_at: 0.0,
                    checked: 0.0,
                    check: None,
                    failed: false,
                });
            }
        }
    }

    /// is its window there yet, did it end? (and what to say meanwhile); true: close the window
    fn tick(&mut self) -> bool {
        let s = self.s.clone();
        let Some(l) = &mut self.loading else { return false };
        if l.failed {
            return false;
        }
        let now = util::now();
        let el = (now - l.t0) as i64;
        if l.ready_at > 0.0 {
            // its window is up: "ready" for a moment, then gone
            return now - l.ready_at > 0.8;
        }
        if let Ok(Some(status)) = l.proc.try_wait() {
            use std::os::unix::process::ExitStatusExt;
            let code = status.code().unwrap_or_else(|| -status.signal().unwrap_or(0));
            if code != 0 && now - l.t0 < 60.0 {
                // it ended right away: say so, with its log
                l.failed = true;
                l.title = format!("{} closed right away", l.e.name);
                l.text = format!("Exit code {code}. Its log says what went wrong.");
                return false;
            }
            return true;
        }
        if let Some(rx) = &l.check {
            if let Ok(shown) = rx.try_recv() {
                l.check = None;
                // (its window is up; or this compositor can't tell: some seconds of starting, then done)
                if (shown == Some(true) || (shown.is_none() && now - l.t0 > 8.0)) && now - l.t0 > 1.2 {
                    l.ready_at = now;
                    l.title = format!("{} is ready", l.e.name);
                    l.text = format!("Started in {} s", el.max(1));
                    return false;
                }
            }
        }
        if now - l.t0 > 180.0 {
            return true;
        }
        if l.check.is_none() && now - l.checked >= 0.7 {
            l.checked = now;
            let (tx, rx) = channel();
            let (e, pid) = (l.e.clone(), l.proc.id());
            std::thread::spawn(move || {
                let _ = tx.send(runners::has_window(&e, Some(pid), &s));
            });
            l.check = Some(rx);
        }
        let what = if l.e.runner != "native" {
            if el < 3 { "Starting Proton…" } else if l.first { "Setting up its prefix (the first start takes about a minute)…" } else { "Waiting for its window…" }
        } else {
            "Waiting for its window…"
        };
        l.text = format!("{what}  {el} s");
        false
    }

    fn frame(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let t = self.t;
        if !self.started {
            self.started = true;
            super::install_fonts(&ctx);
            super::apply_theme(&ctx, &t);
            self.icon = Some(Still::new(&ctx, "quick-icon", &self.icon_img, Overlay::None));
        }
        self.ctx = Some(ctx.clone());
        if let Some(job) = &mut self.stopping {
            if job.poll().is_some() {
                self.stopping = None;
                let e = self.chosen();
                self.start(e);
            }
        }
        if self.loading.is_some() {
            if self.tick() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ctx.request_repaint();
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(t.bg).inner_margin(egui::Margin { left: 22, right: 22, top: 22, bottom: 18 }))
            .show(ui, |ui| {
                if self.loading.is_some() {
                    self.loading_page(ui);
                } else {
                    self.form(ui);
                }
            });
        show_stack(self, |q| &mut q.dialogs, &ctx, &t);
    }

    fn form(&mut self, ui: &mut egui::Ui) {
        let t = self.t;
        let ctx = ui.ctx().clone();
        ui.spacing_mut().item_spacing.y = 14.0;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;
            let (rect, _) = ui.allocate_exact_size(vec2(72.0, 72.0), egui::Sense::hover());
            if let Some(i) = &self.icon {
                i.paint(ui, rect, 14.0, "");
            }
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.add(egui::TextEdit::singleline(&mut self.name).font(super::bold(18.0)).frame(egui::Frame::NONE)
                    .interactive(!self.known).desired_width(f32::INFINITY));
                muted_wrap(ui, &t, &shortcuts::display_path(&self.path));
                if self.known {
                    muted(ui, &t, "In your library: runs with its own settings");
                }
            });
        });
        super::form_row(ui, &t, 70.0, "Run with", |ui| {
            ui.add_enabled_ui(!self.known, |ui| {
                egui::ComboBox::from_id_salt("quick-runner").selected_text(&self.runners[self.choice].0)
                    .width(ui.available_width() - 4.0).show_ui(ui, |ui| {
                        for (i, (label, _, _)) in self.runners.iter().enumerate() {
                            ui.selectable_value(&mut self.choice, i, label);
                        }
                    });
            });
        });
        super::form_row(ui, &t, 70.0, "", |ui| {
            ui.horizontal(|ui| {
                check(ui, &t, &mut self.mangohud, "MangoHud", !self.known);
                check(ui, &t, &mut self.gamemode, "GameMode", !self.known);
            });
            if !self.known {
                check(ui, &t, &mut self.keep, "Add it to my library", true);
            }
        });
        if self.installer {
            muted_wrap(ui, &t, "This looks like an installer: Install runs it in a new prefix and adds the game it installs to your library.");
        }
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.horizontal(|ui| {
                if Btn::new("Open folder").show(ui, &t).clicked() {
                    runners::open_path(&dirname(&self.path));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let enter = self.dialogs.is_empty() && ui.input(|i| i.key_pressed(Key::Enter));
                    if Btn::new("Run").primary().min_width(80.0).show(ui, &t).clicked() || enter {
                        self.run(&ctx);
                    }
                    if self.installer && Btn::new("Install").show(ui, &t).clicked() {
                        self.install = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    if Btn::new("Cancel").show(ui, &t).clicked() || (self.dialogs.is_empty() && ui.input(|i| i.key_pressed(Key::Escape))) {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            });
        });
    }

    fn loading_page(&mut self, ui: &mut egui::Ui) {
        let t = self.t;
        let ctx = ui.ctx().clone();
        let l = self.loading.as_ref().unwrap();
        let time = ui.input(|i| i.time) as f32;
        let avail = ui.available_size();
        ui.vertical_centered(|ui| {
            ui.add_space(((avail.y - 96.0 - 90.0) / 2.0).max(0.0));
            let (rect, _) = ui.allocate_exact_size(vec2(96.0, 96.0), egui::Sense::hover());
            // the program's icon with a ring turning round it; an arc that grows and shrinks as it turns
            let w = (96.0f32 / 26.0).max(3.0);
            let r = (96.0 - w) / 2.0 - 1.0;
            let done = l.ready_at > 0.0 || l.failed;
            ui.painter().circle_stroke(rect.center(), r, egui::Stroke::new(w, super::alpha(t.accent, 0.16)));
            if done {
                ui.painter().circle_stroke(rect.center(), r, egui::Stroke::new(w, t.accent));
            } else {
                let angle = (time * 300.0) % 360.0;
                let span = 70.0 + 60.0 * (1.0 + (angle * 2.0).to_radians().sin()) / 2.0;
                super::arc(ui.painter(), rect.center(), r, angle * 2.0, span, egui::Stroke::new(w, t.accent));
            }
            if let Some(i) = &self.icon {
                let inner = egui::Rect::from_center_size(rect.center(), vec2(54.0, 54.0));
                i.paint(ui, inner, 10.0, "");
            }
            ui.add_space(6.0);
            super::label(ui, &l.title, super::bold(20.0), t.text);
            muted_wrap(ui, &t, &l.text);
        });
        let log = join(&store::logs_dir(), &format!("{}.log", l.log_name));
        let failed = l.failed;
        ui.with_layout(Layout::bottom_up(Align::Max), |ui| {
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if failed {
                        if Btn::new("Close").show(ui, &t).clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        if Btn::new("Show log").show(ui, &t).clicked() {
                            runners::open_path(&log);
                        }
                    } else if Btn::new("Hide").tip("Close this window; the program keeps starting").show(ui, &t).clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            });
        });
    }
}

pub struct Wrapper(pub std::rc::Rc<std::cell::RefCell<QuickRun>>);

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
    use super::*;

    #[test]
    fn quickrun_screens() {
        let (Ok(dir), Ok(exe)) = (std::env::var("VESSEL_SCREENS"), std::env::var("VESSEL_QUICK_EXE")) else { return };
        let q = std::rc::Rc::new(std::cell::RefCell::new(QuickRun::new(&exe)));
        let h = q.borrow().height();
        let q2 = q.clone();
        let mut harness = egui_kittest::Harness::builder().with_accessibility_check(false).with_size(vec2(500.0, h)).build_ui(move |ui| q2.borrow_mut().frame(ui));
        harness.run_steps(4);
        harness.render().unwrap().save(format!("{dir}/q1-form.png")).unwrap();
        let e = q.borrow().chosen();
        q.borrow_mut().loading = Some(Loading {
            title: format!("Starting {}", e.name), text: "Setting up its prefix (the first start takes about a minute)…  7 s".into(),
            first: true, t0: util::now() + 1e6, proc: std::process::Command::new("sleep").arg("5").spawn().unwrap(),
            log_name: "quick".into(), ready_at: 0.0, checked: util::now() + 1e6, check: None, failed: false, e,
        });
        harness.run_steps(3);
        harness.render().unwrap().save(format!("{dir}/q2-loading.png")).unwrap();
    }

    #[test]
    fn cover_search_online() {
        if std::env::var("VESSEL_NET").is_err() {
            return;
        }
        let c = crate::net::cover_candidates("Hades", "").unwrap();
        assert!(!c.is_empty());
        let data = crate::net::http_get(&c[0].thumb, &[], 15).unwrap();
        assert!(imaging::decode(&data).is_some());
        println!("{} covers, first {} ({} bytes)", c.len(), c[0].label, data.len());
    }
}
