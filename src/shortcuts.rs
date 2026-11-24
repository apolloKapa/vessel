//! Menu and desktop shortcuts, opening .exe files from the file manager, and moving over from Hangar.

use crate::runners::{self, is_windows};
use crate::store::{self, Entry, Settings};
use crate::util::{self, isdir, join, realpath};
use md5::Digest;
use std::fs;
use std::os::unix::fs::PermissionsExt;

/// how to start this Vessel again: the AppImage, or this binary
pub fn self_command() -> Vec<String> {
    if let Some(a) = crate::appimage() {
        return vec![a];
    }
    vec![std::env::current_exe().map(|p| util::path_str(&p)).unwrap_or_else(|_| "vessel".into())]
}

// ---- opening programs from the file manager --------------------------------------------------

/// Windows programs' MIME types: Vessel can be the app that opens them (double-click an .exe and it runs)
pub const EXE_MIMES: &[&str] = &["application/x-ms-dos-executable", "application/x-msdownload", "application/x-msi",
    "application/x-ms-shortcut", "application/vnd.microsoft.portable-executable", "application/x-dosexec",
    "application/x-ms-ne-executable", "application/x-bat", "application/x-msdos-program"];

pub fn opener() -> String {
    join(&store::menu_dir(), "vessel-open.desktop")
}

/// the library entry that runs this file, if there is one
pub fn entry_for_path(path: &str) -> Option<Entry> {
    let real = realpath(path);
    store::library().into_iter().find(|e| {
        let exe = util::expanduser(&e.exe);
        !exe.is_empty() && realpath(&exe) == real
    })
}

/// an entry for running a file once: its library entry if it has one, else a new one (not saved) whose
/// prefix is the one it would get in the library, so its saves are there if you add it later
pub fn quick_entry(path: &str, s: &Settings) -> (Entry, bool) {
    if let Some(e) = entry_for_path(path) {
        return (e, true);
    }
    let digest = md5::Md5::digest(realpath(path).as_bytes());
    let mut e = Entry::new();
    e.id = format!("quick-{}", &hex::encode(digest)[..10]);
    e.name = util::guess_name(path);
    e.exe = path.to_string();
    e.runner = if is_windows(path) {
        if s.windows_runner.is_empty() { "proton".into() } else { s.windows_runner.clone() }
    } else {
        "native".into()
    };
    e.added = util::now_secs();
    (e, false)
}

/// Vessel opens Windows programs (or stops doing so): a hidden menu entry for their MIME types, made the
/// default for them
pub fn register_opener(on: bool) -> std::io::Result<()> {
    let path = opener();
    if on {
        fs::create_dir_all(store::menu_dir())?;
        let mut exec = self_command();
        exec.push("--open".into());
        fs::write(&path, format!(
            "[Desktop Entry]\nType=Application\nName=Vessel\nGenericName=Run with Vessel\n\
             Comment=Run this Windows program with Proton or Wine\nExec={} %f\n\
             Icon=vessel\nTerminal=false\nNoDisplay=true\nStartupWMClass=vessel\nMimeType={};\n",
            util::shlex_join(&exec), EXE_MIMES.join(";")))?;
        if runners::which("xdg-mime").is_some() {
            let mut cmd = vec!["xdg-mime", "default", "vessel-open.desktop"];
            cmd.extend(EXE_MIMES);
            runners::quiet(&cmd);
        }
    } else {
        let _ = fs::remove_file(&path);
        // take it back out of mimeapps.list, so the type falls back to whatever else opens it
        let mimeapps = join(&util::config_home(), "mimeapps.list");
        if let Ok(text) = fs::read_to_string(&mimeapps) {
            let lines: Vec<&str> = text.split_inclusive('\n').collect();
            let keep: Vec<&str> = lines.iter().copied().filter(|ln| {
                let key = ln.split('=').next().unwrap_or("").trim();
                !(EXE_MIMES.contains(&key) && ln.contains("vessel-open.desktop"))
            }).collect();
            if keep.len() != lines.len() {
                let _ = fs::write(&mimeapps, keep.concat());
            }
        }
    }
    if runners::which("update-desktop-database").is_some() {
        runners::quiet(&["update-desktop-database", &store::menu_dir()]);
    }
    Ok(())
}

// ---- menu and desktop shortcuts --------------------------------------------------------------

/// the user's desktop folder (XDG_DESKTOP_DIR from user-dirs.dirs, else ~/Desktop)
pub fn desktop_dir() -> String {
    let h = util::home();
    if let Ok(text) = fs::read_to_string(join(&util::config_home(), "user-dirs.dirs")) {
        for line in text.lines() {
            if let Some(v) = line.strip_prefix("XDG_DESKTOP_DIR=") {
                let path = v.trim().trim_matches('"').replace("$HOME", &h);
                if !path.is_empty() && util::abspath(&path) != util::abspath(&h) {
                    return path;
                }
            }
        }
    }
    join(&h, "Desktop")
}

/// the desktop file for this entry; found by its X-Vessel-Id so a renamed entry keeps its file
fn desktop_shortcut_path(e: &Entry) -> Option<String> {
    let folder = desktop_dir();
    for f in util::listdir(&folder) {
        if !f.ends_with(".desktop") {
            continue;
        }
        let path = join(&folder, &f);
        let Ok(bytes) = fs::read(&path) else { continue };
        let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]).into_owned();
        if head.contains(&format!("\nX-Vessel-Id={}\n", e.id)) || head.contains(&format!("\nX-Hangar-Id={}\n", e.id)) {
            return Some(path);
        }
    }
    None
}

pub fn shortcut_path(e: &Entry, desktop: bool) -> String {
    if desktop {
        return desktop_shortcut_path(e).unwrap_or_default();
    }
    join(&store::menu_dir(), &format!("vessel-{}.desktop", e.id))
}

pub fn has_shortcut(e: &Entry, desktop: bool) -> bool {
    let p = shortcut_path(e, desktop);
    !p.is_empty() && util::exists(&p)
}

fn shortcut_text(e: &Entry) -> String {
    let icon = if !e.icon.is_empty() {
        join(&store::icons_dir(), &e.icon)
    } else if !e.cover.is_empty() {
        join(&store::covers_dir(), &e.cover)
    } else {
        "applications-games".into()
    };
    let mut exec = self_command();
    exec.push("--launch".into());
    exec.push(e.id.clone());
    format!("[Desktop Entry]\nType=Application\nName={}\nComment=Run with Vessel\nExec={}\nIcon={icon}\n\
             Terminal=false\nCategories={}\nX-Vessel-Id={}\n",
            e.name.replace('\n', " "), util::shlex_join(&exec), if e.kind == "game" { "Game;" } else { "Utility;" }, e.id)
}

pub fn make_shortcut(e: &Entry, desktop: bool) -> std::io::Result<()> {
    if !desktop {
        fs::create_dir_all(store::menu_dir())?;
        return fs::write(shortcut_path(e, false), shortcut_text(e));
    }
    let folder = desktop_dir();
    fs::create_dir_all(&folder)?;
    let mut stem: String = e.name.replace('\n', " ").chars()
        .filter(|c| !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') && (*c as u32) >= 0x20)
        .collect();
    stem = stem.trim_matches(|c| c == ' ' || c == '.').to_string();
    if stem.is_empty() {
        stem = "Game".into();
    }
    let old = desktop_shortcut_path(e);
    let mut path = join(&folder, &format!("{stem}.desktop"));
    let mut n = 2;
    while util::exists(&path) && Some(&path) != old.as_ref() {
        path = join(&folder, &format!("{stem} ({n}).desktop"));
        n += 1;
    }
    fs::write(&path, shortcut_text(e))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
    if let Some(old) = old.filter(|o| *o != path) {
        let _ = fs::remove_file(old);
    }
    // Nautilus/Nemo only run desktop files marked trusted
    if runners::which("gio").is_some() {
        runners::quiet(&["gio", "set", &path, "metadata::trusted", "true"]);
    }
    Ok(())
}

pub fn remove_shortcut(e: &Entry, desktop: bool) {
    let p = shortcut_path(e, desktop);
    if !p.is_empty() {
        let _ = fs::remove_file(p);
    }
}

/// rewrite whichever shortcuts this entry already has (after a rename, new icon, kind change)
pub fn refresh_shortcuts(e: &Entry) {
    for desktop in [false, true] {
        if has_shortcut(e, desktop) {
            let _ = make_shortcut(e, desktop);
        }
    }
}

/// Vessel was called Hangar until 2.0.0: move ~/.config/hangar and ~/.local/share/hangar over (leaving a link
/// behind, so paths saved in launch options still work) and rewrite the shortcuts it made
pub fn migrate_from_hangar() {
    let mut moved = false;
    for new in [store::conf_dir(), store::data_dir()] {
        let old = join(&util::dirname(&new), "hangar");
        if isdir(&old) && !util::islink(&old) && !util::lexists(&new) {
            match fs::rename(&old, &new).and_then(|_| std::os::unix::fs::symlink(&new, &old)) {
                Ok(()) => moved = true,
                Err(ex) => eprintln!("vessel: couldn't move {old} to {new}: {ex}"),
            }
        }
    }
    if !moved {
        return;
    }
    let s = store::settings();
    let old_pfx = util::joins(&[&util::home(), "Games", "Hangar", "prefixes"]);
    let stored = store::read_json(&store::settings_file());
    let has_root = stored.as_ref().and_then(|v| v.as_object()).is_some_and(|m| m.contains_key("prefix_root"));
    if !has_root && isdir(&old_pfx) {
        // Hangar's default; Vessel's is ~/Games/Vessel/prefixes
        store::save_settings(&[("prefix_root", old_pfx.into())]);
    }
    for e in store::library() {
        let old_menu = join(&store::menu_dir(), &format!("hangar-{}.desktop", e.id));
        if util::exists(&old_menu) && fs::remove_file(&old_menu).is_ok() {
            let _ = make_shortcut(&e, false);
        }
        if desktop_shortcut_path(&e).is_some() {
            let _ = make_shortcut(&e, true);
        }
    }
    let old_opener = join(&store::menu_dir(), "hangar-open.desktop");
    if util::exists(&old_opener) {
        let _ = fs::remove_file(&old_opener);
        if s.exe_opener {
            let _ = register_opener(true);
        }
    }
}

pub fn display_path(path: &str) -> String {
    let h = util::home();
    match path.strip_prefix(&h) {
        Some(rest) => format!("~{rest}"),
        None => path.to_string(),
    }
}
