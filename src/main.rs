//! Vessel: a launcher for the games and apps you have outside Steam.
//!
//! Add anything you can run: a Windows .exe (run through Proton, GE-Proton by default, or Wine), a native
//! binary, an AppImage, a script or a plain command such as `flatpak run ...`. Each Windows program gets its own
//! prefix. Everything Vessel knows is in ~/.config/vessel (library.json, settings.json) and
//! ~/.local/share/vessel (covers, icons, logs, prefixes of its own runners).

mod files;
mod imaging;
mod instance;
mod launcher;
mod net;
mod runners;
mod shortcuts;
mod store;
mod task;
mod tray;
mod ui;
mod util;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use store::VERSION;

const HELP: &str = "\
Vessel: a launcher for the games and apps you have outside Steam.

  vessel                 open the window
  vessel --launch ID     run one entry without the window (used by menu shortcuts)
  vessel FILE.exe        open a Windows program (or --open FILE): a small window to run it, pick
                         Proton/Wine and optionally add it to the library, like PortProton
  vessel --run FILE      run a program right away with the default runner, no window
  vessel --check         list the Proton/Wine versions and tools Vessel finds

VESSEL_WAYLAND=1 opens the window as a native Wayland window (files can't be dropped on it then).";

static APPIMAGE: OnceLock<Option<String>> = OnceLock::new();

/// the AppImage Vessel runs from (read before anything can change the environment)
pub fn appimage() -> Option<String> {
    APPIMAGE.get_or_init(|| std::env::var("APPIMAGE").ok().filter(|a| !a.is_empty())).clone()
}

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_sigint(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

/// waits for a game started from the command line; Ctrl+C stops it (and is counted)
fn wait_child(mut child: std::process::Child) -> i32 {
    unsafe { libc::signal(libc::SIGINT, on_sigint as *const () as libc::sighandler_t) };
    let mut sent = false;
    loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                use std::os::unix::process::ExitStatusExt;
                return st.code().unwrap_or_else(|| 128 + st.signal().unwrap_or(0));
            }
            Ok(None) => {}
            Err(_) => return 1,
        }
        if INTERRUPTED.load(Ordering::SeqCst) && !sent {
            sent = true;
            runners::kill_group(child.id(), libc::SIGTERM);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

/// a menu shortcut / --run for something whose prefix is busy: it's open already, or its last run is stuck (and
/// Proton would wait for that forever). Says so instead of starting nothing; true: busy
fn cli_busy(e: &store::Entry) -> bool {
    if runners::prefix_procs(e, &store::settings()).is_empty() {
        return false;
    }
    let msg = "It's open already, or its last run is stuck. Open it from Vessel to stop that and start again.";
    eprintln!("vessel: {} is still running. {msg}", e.name);
    runners::notify(&format!("{} is still running", e.name), msg);
    true
}

fn cli_launch(eid: &str) -> i32 {
    let Some(e) = store::get_entry(eid) else {
        eprintln!("vessel: no entry {eid}");
        return 1;
    };
    if cli_busy(&e) {
        return 1;
    }
    let s = store::settings();
    if e.vr && s.start_wivrn {
        runners::start_wivrn();
    }
    let child = match runners::start_process(&e, &s, None, None, false, None) {
        Ok(c) => c,
        Err(ex) => {
            eprintln!("vessel: can't start {}: {ex}", e.name);
            runners::notify(&format!("Can't start {}", e.name), &ex.0);
            return 1;
        }
    };
    let start = util::now();
    store::update_entry(eid, |x| x.last_played = start as i64);
    let code = wait_child(child);
    store::add_playtime(eid, util::now() - start);
    code
}

/// `vessel --run FILE`: runs it right away (its library entry if it has one), no window
fn cli_run(path: &str) -> i32 {
    if !util::isfile(path) {
        eprintln!("vessel: no such file {path}");
        return 1;
    }
    let s = store::settings();
    let (e, known) = shortcuts::quick_entry(path, &s);
    if known {
        return cli_launch(&e.id);
    }
    if cli_busy(&e) {
        return 1;
    }
    match runners::start_process(&e, &s, None, None, false, Some("quick")) {
        Ok(c) => wait_child(c),
        Err(ex) => {
            eprintln!("vessel: can't run {path}: {ex}");
            runners::notify(&format!("Can't run {}", util::basename(path)), &ex.0);
            1
        }
    }
}

/// prints what Vessel finds on this PC, and whether downloads work
fn cli_check() -> i32 {
    let s = store::settings();
    println!("Vessel {VERSION}");
    let pv = runners::proton_versions();
    let names: Vec<String> = pv.iter().map(|v| v.name.clone()).collect();
    let dp = runners::default_proton(&s);
    println!("Proton ({}): {}\n  default: {}", pv.len(), if names.is_empty() { "none".into() } else { names.join(", ") },
             if dp.is_empty() { "-".into() } else { dp });
    let wv = runners::wine_versions();
    let names: Vec<String> = wv.iter().map(|v| v.name.clone()).collect();
    println!("Wine ({}): {}", wv.len(), if names.is_empty() { "none".into() } else { names.join(", ") });
    let xr = runners::xrizer_dir();
    println!("VR: WiVRn {}, xrizer {}",
             if runners::wivrn_running() { "running" } else if runners::which("wivrn-server").is_some() { "installed" } else { "not installed" },
             if xr.is_empty() { "not found".into() } else { xr });
    for tool in ["umu-run", "winetricks", "mangohud", "gamemoderun", "gamescope", "bsdtar"] {
        let found = if tool == "bsdtar" { files::bsdtar() } else { runners::which(tool) };
        println!("{tool}: {}", found.unwrap_or_else(|| "not found".into()));
    }
    match net::http_get("https://api.github.com/", &[], 10) {
        Ok(_) => {
            println!("downloads: ok");
            0
        }
        Err(e) => {
            println!("downloads: FAILED ({e})");
            1
        }
    }
}

fn native_options(title: &str, size: [f32; 2], min: [f32; 2], fixed: bool) -> eframe::NativeOptions {
    let icon = imaging::app_icon(128);
    let mut vp = egui::ViewportBuilder::default()
        .with_title(title)
        .with_app_id("vessel")
        .with_inner_size(size)
        .with_min_inner_size(min)
        .with_drag_and_drop(true)
        .with_icon(egui::IconData { rgba: icon.into_raw(), width: 128, height: 128 });
    if fixed {
        vp = vp.with_max_inner_size(size).with_resizable(false);
    }
    let mut o = eframe::NativeOptions { viewport: vp, ..Default::default() };
    o.run_and_return = true;
    o.persist_window = false;
    // winit only takes dropped files through X11 (XWayland); VESSEL_WAYLAND=1 keeps it a Wayland window
    let wayland = std::env::var("VESSEL_WAYLAND").is_ok_and(|v| v == "1");
    if !wayland && std::env::var("DISPLAY").is_ok_and(|d| !d.is_empty()) {
        o.event_loop_builder = Some(Box::new(|b| {
            use winit::platform::x11::EventLoopBuilderExtX11;
            b.with_x11();
        }));
    }
    o
}

fn run_window(paths: Vec<String>, installer: Option<String>) {
    let (tx, rx) = std::sync::mpsc::channel();
    let wake: Arc<Mutex<Option<egui::Context>>> = Default::default();
    let _ = tray::TARGET.set((tx.clone(), wake.clone()));
    instance::listen(tx, wake.clone());
    let (launcher, lrx) = launcher::Launcher::new();
    let app = Rc::new(RefCell::new(ui::app::VesselApp::new(launcher, lrx, rx, wake.clone())));
    app.borrow_mut().pending_paths = paths;
    app.borrow_mut().pending_installer = installer;
    loop {
        app.borrow_mut().new_window = true;
        let a = app.clone();
        let r = eframe::run_native("Vessel", native_options("Vessel", [1100.0, 720.0], [700.0, 460.0], false),
                                   Box::new(move |_cc| Ok(Box::new(ui::app::Wrapper(a)))));
        *wake.lock().unwrap() = None;
        if let Err(e) = r {
            eprintln!("vessel: the window couldn't open: {e}");
            break;
        }
        let mut a = app.borrow_mut();
        if a.quit || !a.hidden {
            break;
        }
        // in the tray: the window comes back for a click on the icon or another start of Vessel
        a.lock_nsfw();
        let quit = match a.events.recv() {
            Ok(ui::app::AppEvent::Show(p)) => {
                a.pending_paths.extend(p);
                false
            }
            Ok(ui::app::AppEvent::TrayActivate) | Ok(ui::app::AppEvent::TrayOpen) => false,
            // with games running, the window opens to ask
            Ok(ui::app::AppEvent::TrayQuit) => a.launcher.running_ids().is_empty(),
            Err(_) => true,
        };
        if quit {
            break;
        }
    }
    instance::close();
}

fn main() {
    appimage();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let first = args.first().map(|s| s.as_str()).unwrap_or("");
    if !["-h", "--help", "-v", "--version", "--render-icon"].contains(&first) {
        shortcuts::migrate_from_hangar();
    }
    match first {
        "--check" => std::process::exit(cli_check()),
        "--launch" if args.len() >= 2 => std::process::exit(cli_launch(&args[1])),
        "--run" if args.len() >= 2 => std::process::exit(cli_run(&util::abspath(&util::expanduser(&args[1])))),
        "-h" | "--help" => {
            println!("{HELP}");
            return;
        }
        "-v" | "--version" => {
            println!("Vessel {VERSION}");
            return;
        }
        // for the AppImage build: the icon as a PNG
        "--render-icon" if args.len() >= 3 => {
            let size = args[1].parse().unwrap_or(256);
            let path = args[2].clone();
            std::process::exit(if imaging::app_icon(size).save(&path).is_ok() { 0 } else { 1 });
        }
        _ => {}
    }
    // keep the file manager's "open with" pointing at this copy (an AppImage that moved, a new version)
    if store::settings().exe_opener {
        let _ = shortcuts::register_opener(true);
    }
    let open = args.iter().any(|a| a == "--open");
    let paths: Vec<String> = args.iter().filter(|a| *a != "--open" && util::exists(a)).map(|a| util::abspath(a)).collect();
    // one program opened from the file manager (or `vessel FILE.exe`): the small run window, PortProton-style
    if paths.len() == 1 && util::isfile(&paths[0]) && !files::is_archive(&paths[0]) && (runners::is_windows(&paths[0]) || open) {
        let q = Rc::new(RefCell::new(ui::quickrun::QuickRun::new(&paths[0])));
        let (title, h) = (q.borrow().title(), q.borrow().height());
        let q2 = q.clone();
        if let Err(e) = eframe::run_native("Vessel", native_options(&title, [500.0, h], [500.0, h], true),
                                           Box::new(move |_cc| Ok(Box::new(ui::quickrun::Wrapper(q2))))) {
            eprintln!("vessel: the window couldn't open: {e}");
            std::process::exit(1);
        }
        if q.borrow().install {
            // the installer goes through the main window, which finds the installed game and adds it
            run_window(vec![], Some(paths[0].clone()));
        }
        return;
    }
    // one Vessel at a time: starting it again brings back the running one, which may be hidden in the tray,
    // and hands it the files
    if instance::hand_over(&paths) {
        return;
    }
    run_window(paths, None);
}

#[cfg(test)]
mod parity {
    //! compares with the Python Vessel: VESSEL_PARITY_IN (cases, JSON) → VESSEL_PARITY_OUT (results)
    use super::*;
    use serde_json::{Value, json};

    #[test]
    fn python_parity() {
        let Ok(input) = std::env::var("VESSEL_PARITY_IN") else { return };
        let cases: Value = serde_json::from_str(&std::fs::read_to_string(input).unwrap()).unwrap();
        let s = store::settings();
        let host = runners::host_env();
        let mut out = serde_json::Map::new();
        let mut names = serde_json::Map::new();
        for p in cases["paths"].as_array().unwrap() {
            let p = p.as_str().unwrap();
            let icon = files::exe_icon(p).map(|d| hex::encode(<md5::Md5 as md5::Digest>::digest(&d))).unwrap_or_default();
            let (eac, be) = runners::looks_anticheat(p);
            names.insert(p.into(), json!({
                "guess": util::guess_name(p), "archive": util::archive_name(p), "slug": util::slug(&util::guess_name(p)),
                "installer": files::is_installer(p), "is_archive": files::is_archive(p), "icon": icon,
                "vr": runners::looks_vr(p), "eac": eac, "be": be, "steamworks": runners::uses_steamworks(p),
                "native": files::is_native_program(p),
            }));
        }
        out.insert("paths".into(), Value::Object(names));
        let mut folders = serde_json::Map::new();
        for f in cases["folders"].as_array().unwrap() {
            let f = f.as_str().unwrap();
            folders.insert(f.into(), json!(files::find_programs(f, 0.0, 6).into_iter().take(3).collect::<Vec<_>>()));
        }
        out.insert("folders".into(), Value::Object(folders));
        let mut opts = serde_json::Map::new();
        for o in cases["options"].as_array().unwrap() {
            let o = o.as_str().unwrap();
            opts.insert(o.into(), match runners::parse_launch_options(o) {
                Ok(l) => json!([l.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect::<std::collections::BTreeMap<_, _>>(), l.pre, l.args]),
                Err(e) => json!(e.0),
            });
        }
        out.insert("options".into(), Value::Object(opts));
        let mut cmds = serde_json::Map::new();
        for e in store::library() {
            let r = match runners::build_command(&e, &s, None, None, false) {
                Ok(l) => {
                    let diff: std::collections::BTreeMap<String, String> = l.env.iter()
                        .filter(|(k, v)| host.get(*k) != Some(v) && *k != "PATH").map(|(k, v)| (k.clone(), v.clone())).collect();
                    json!({"cmd": l.cmd, "env": diff, "cwd": l.cwd})
                }
                Err(x) => json!(x.0),
            };
            cmds.insert(e.id.clone(), json!({"build": r, "label": runners::runner_label(&e, &s), "prefix": runners::prefix_of(&e, &s),
                                             "wineprefix": runners::wine_prefix(&e, &s)}));
        }
        out.insert("entries".into(), Value::Object(cmds));
        out.insert("protons".into(), json!(runners::proton_versions().iter().map(|v| [v.name.clone(), v.path.clone()]).collect::<Vec<_>>()));
        out.insert("wines".into(), json!(runners::wine_versions().iter().map(|v| [v.name.clone(), v.path.clone()]).collect::<Vec<_>>()));
        out.insert("libraries".into(), json!(runners::steam_libraries()));
        let mut sorted: Vec<String> = store::library().into_iter().map(|e| e.name).collect();
        sorted.sort_by(|a, b| util::natural_cmp(a, b));
        out.insert("sorted".into(), json!(sorted));
        out.insert("times".into(), json!([0, 30, 59, 60, 3599, 3600, 7322, 100000].iter().map(|t| util::human_time(*t)).collect::<Vec<_>>()));
        let now = util::now_secs();
        out.insert("whens".into(), json!([0, now, now - 86400, now - 5 * 86400, now - 40 * 86400].iter().map(|t| util::human_when(*t)).collect::<Vec<_>>()));
        out.insert("pw".into(), json!(cases["pw"].as_object().map(|r| store::password_ok("hunter2", r) && !store::password_ok("x", r))));
        std::fs::write(std::env::var("VESSEL_PARITY_OUT").unwrap(), serde_json::to_string_pretty(&Value::Object(out)).unwrap()).unwrap();
    }
}
