//! library.json and settings.json, in the same format the Python Vessel wrote.

use crate::util::{self, join};
use serde_json::{Map, Value, json};
use std::fs;
use std::sync::Mutex;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn conf_dir() -> String {
    join(&util::config_home(), "vessel")
}
pub fn data_dir() -> String {
    join(&util::data_home(), "vessel")
}
pub fn library_file() -> String {
    join(&conf_dir(), "library.json")
}
pub fn settings_file() -> String {
    join(&conf_dir(), "settings.json")
}
pub fn covers_dir() -> String {
    join(&data_dir(), "covers")
}
pub fn icons_dir() -> String {
    join(&data_dir(), "icons")
}
pub fn logs_dir() -> String {
    join(&data_dir(), "logs")
}
pub fn runners_dir() -> String {
    join(&data_dir(), "runners")
}
pub fn tools_dir() -> String {
    join(&data_dir(), "tools")
}
pub fn menu_dir() -> String {
    join(&util::data_home(), "applications")
}

/// the library and settings are read, changed and written whole: one at a time (the launcher's thread
/// counts playtime while the window edits)
static LOCK: Mutex<()> = Mutex::new(());

pub fn read_json(path: &str) -> Option<Value> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn write_json(path: &str, value: &Value) -> std::io::Result<()> {
    fs::create_dir_all(util::dirname(path))?;
    let tmp = format!("{path}.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(value).unwrap_or_default())?;
    fs::rename(&tmp, path)
}

// ---- settings --------------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Settings {
    pub theme: String,
    pub accent: String,
    pub view: String,
    pub size: String,
    pub kind: String,
    pub tab: String,
    pub sort: String,
    pub proton: String,
    pub wine: String,
    pub windows_runner: String,
    pub prefix_root: String,
    pub use_umu: bool,
    pub show_nsfw: bool,
    pub blur_nsfw: bool,
    pub sgdb_key: String,
    pub vr_options: String,
    pub start_wivrn: bool,
    pub wayland: String,
    pub games_root: String,
    pub exe_opener: bool,
    pub close_to_tray: bool,
    pub nsfw_hidden: bool,
    pub nsfw_key: String,
    pub nsfw_pass: Map<String, Value>,
}

pub fn default_prefix_root() -> String {
    util::joins(&[&util::home(), "Games", "Vessel", "prefixes"])
}

pub fn default_games_root() -> String {
    join(&util::home(), "Games")
}

fn get_str(m: &Map<String, Value>, k: &str, d: &str) -> String {
    match m.get(k) {
        Some(Value::String(s)) => s.clone(),
        _ => d.to_string(),
    }
}

fn get_bool(m: &Map<String, Value>, k: &str, d: bool) -> bool {
    match m.get(k) {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0) != 0.0,
        _ => d,
    }
}

fn get_int(m: &Map<String, Value>, k: &str) -> i64 {
    match m.get(k) {
        Some(Value::Number(n)) => n.as_i64().unwrap_or_else(|| n.as_f64().unwrap_or(0.0) as i64),
        Some(Value::Bool(b)) => *b as i64,
        _ => 0,
    }
}

fn stored_settings() -> Map<String, Value> {
    match read_json(&settings_file()) {
        Some(Value::Object(m)) => m,
        _ => Map::new(),
    }
}

pub fn settings() -> Settings {
    let m = stored_settings();
    // Wayland was on or off; on was the default, kept for niri: it's Automatic now (False stays Never)
    let wayland = match m.get("wayland") {
        Some(Value::Bool(false)) => "off".to_string(),
        Some(Value::String(s)) if ["auto", "on", "off"].contains(&s.as_str()) => s.clone(),
        _ => "auto".to_string(),
    };
    Settings {
        theme: get_str(&m, "theme", "Midnight"),
        accent: get_str(&m, "accent", ""),
        view: get_str(&m, "view", "grid"),
        size: get_str(&m, "size", "medium"),
        kind: get_str(&m, "kind", "game"),
        tab: get_str(&m, "tab", "library"),
        sort: get_str(&m, "sort", "name"),
        proton: get_str(&m, "proton", ""),
        wine: get_str(&m, "wine", ""),
        windows_runner: get_str(&m, "windows_runner", "proton"),
        prefix_root: get_str(&m, "prefix_root", &default_prefix_root()),
        use_umu: get_bool(&m, "use_umu", true),
        show_nsfw: get_bool(&m, "show_nsfw", false),
        blur_nsfw: get_bool(&m, "blur_nsfw", true),
        sgdb_key: get_str(&m, "sgdb_key", ""),
        vr_options: get_str(&m, "vr_options", ""),
        start_wivrn: get_bool(&m, "start_wivrn", true),
        wayland,
        games_root: get_str(&m, "games_root", &default_games_root()),
        exe_opener: get_bool(&m, "exe_opener", false),
        close_to_tray: get_bool(&m, "close_to_tray", true),
        nsfw_hidden: get_bool(&m, "nsfw_hidden", false),
        nsfw_key: get_str(&m, "nsfw_key", "Ctrl+Shift+N"),
        nsfw_pass: match m.get("nsfw_pass") {
            Some(Value::Object(o)) => o.clone(),
            _ => Map::new(),
        },
    }
}

impl Settings {
    pub fn has_password(&self) -> bool {
        self.nsfw_pass.get("hash").and_then(|h| h.as_str()).is_some_and(|h| !h.is_empty())
    }
}

/// changes some settings in settings.json (the rest stay as they are)
pub fn save_settings(changes: &[(&str, Value)]) {
    let _g = LOCK.lock();
    let mut m = stored_settings();
    for (k, v) in changes {
        m.insert(k.to_string(), v.clone());
    }
    if let Err(e) = write_json(&settings_file(), &Value::Object(m)) {
        eprintln!("vessel: couldn't save the settings: {e}");
    }
}

/// Proton games as Wayland windows (GE-Proton's Wayland mode)? Automatic: on niri only, where X11
/// windows (through xwayland-satellite) got lost on monitors at negative positions. Elsewhere X11 is
/// better: a Wayland window can't be resized by the compositor unless the program says it's resizable
pub fn wayland_on(s: &Settings) -> bool {
    if std::env::var("WAYLAND_DISPLAY").map(|v| v.is_empty()).unwrap_or(true) {
        return false;
    }
    s.wayland == "on" || (s.wayland == "auto" && std::env::var("NIRI_SOCKET").is_ok_and(|v| !v.is_empty()))
}

// ---- NSFW password: a salted scrypt hash (PBKDF2 records from Pythons without scrypt still work) --

pub fn hash_password(pw: &str) -> Value {
    let mut salt = [0u8; 16];
    let _ = getrandom::fill(&mut salt);
    let mut out = [0u8; 32];
    let params = scrypt::Params::new(14, 8, 1).expect("scrypt parameters");
    scrypt::scrypt(pw.as_bytes(), &salt, &params, &mut out).expect("scrypt");
    json!({"kind": "scrypt", "salt": hex::encode(salt), "hash": hex::encode(out)})
}

pub fn password_ok(pw: &str, rec: &Map<String, Value>) -> bool {
    let Some(hash) = rec.get("hash").and_then(|h| h.as_str()).filter(|h| !h.is_empty()) else {
        return true;
    };
    let Some(salt) = rec.get("salt").and_then(|s| s.as_str()).and_then(|s| hex::decode(s).ok()) else {
        return false;
    };
    let mut out = [0u8; 32];
    if rec.get("kind").and_then(|k| k.as_str()) == Some("pbkdf2") {
        pbkdf2::pbkdf2_hmac::<sha2::Sha256>(pw.as_bytes(), &salt, 300_000, &mut out);
    } else {
        let params = scrypt::Params::new(14, 8, 1).expect("scrypt parameters");
        if scrypt::scrypt(pw.as_bytes(), &salt, &params, &mut out).is_err() {
            return false;
        }
    }
    let mine = hex::encode(out);
    // the same time whatever matches
    mine.len() == hash.len() && mine.bytes().zip(hash.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

// ---- library ---------------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub exe: String,
    pub args: String,
    pub cwd: String,
    pub runner: String,
    pub version: String,
    pub prefix: String,
    pub env: String,
    pub wrapper: String,
    pub mangohud: bool,
    pub gamemode: bool,
    pub favorite: bool,
    pub nsfw: bool,
    pub hidden: bool,
    pub vr: bool,
    pub eac: bool,
    pub battleye: bool,
    pub cover: String,
    pub icon: String,
    pub added: i64,
    pub last_played: i64,
    pub playtime: i64,
    /// keys this version doesn't know, kept as they are
    pub extra: Map<String, Value>,
}

const KNOWN: &[&str] = &["id", "name", "kind", "exe", "args", "cwd", "runner", "version", "prefix", "env", "wrapper",
    "mangohud", "gamemode", "favorite", "nsfw", "hidden", "vr", "eac", "battleye", "cover", "icon", "added",
    "last_played", "playtime"];

impl Entry {
    pub fn new() -> Self {
        Entry { kind: "game".into(), runner: "native".into(), ..Default::default() }
    }

    pub fn from_json(m: &Map<String, Value>) -> Self {
        Entry {
            id: get_str(m, "id", ""),
            name: get_str(m, "name", ""),
            kind: get_str(m, "kind", "game"),
            exe: get_str(m, "exe", ""),
            args: get_str(m, "args", ""),
            cwd: get_str(m, "cwd", ""),
            runner: get_str(m, "runner", "native"),
            version: get_str(m, "version", ""),
            prefix: get_str(m, "prefix", ""),
            env: get_str(m, "env", ""),
            wrapper: get_str(m, "wrapper", ""),
            mangohud: get_bool(m, "mangohud", false),
            gamemode: get_bool(m, "gamemode", false),
            favorite: get_bool(m, "favorite", false),
            nsfw: get_bool(m, "nsfw", false),
            hidden: get_bool(m, "hidden", false),
            vr: get_bool(m, "vr", false),
            eac: get_bool(m, "eac", false),
            battleye: get_bool(m, "battleye", false),
            cover: get_str(m, "cover", ""),
            icon: get_str(m, "icon", ""),
            added: get_int(m, "added"),
            last_played: get_int(m, "last_played"),
            playtime: get_int(m, "playtime"),
            extra: m.iter().filter(|(k, _)| !KNOWN.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect(),
        }
    }

    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        let s = |v: &String| Value::String(v.clone());
        m.insert("id".into(), s(&self.id));
        m.insert("name".into(), s(&self.name));
        m.insert("kind".into(), s(&self.kind));
        m.insert("exe".into(), s(&self.exe));
        m.insert("args".into(), s(&self.args));
        m.insert("cwd".into(), s(&self.cwd));
        m.insert("runner".into(), s(&self.runner));
        m.insert("version".into(), s(&self.version));
        m.insert("prefix".into(), s(&self.prefix));
        m.insert("env".into(), s(&self.env));
        m.insert("wrapper".into(), s(&self.wrapper));
        m.insert("mangohud".into(), self.mangohud.into());
        m.insert("gamemode".into(), self.gamemode.into());
        m.insert("favorite".into(), self.favorite.into());
        m.insert("nsfw".into(), self.nsfw.into());
        m.insert("hidden".into(), self.hidden.into());
        m.insert("vr".into(), self.vr.into());
        m.insert("eac".into(), self.eac.into());
        m.insert("battleye".into(), self.battleye.into());
        m.insert("cover".into(), s(&self.cover));
        m.insert("icon".into(), s(&self.icon));
        m.insert("added".into(), self.added.into());
        m.insert("last_played".into(), self.last_played.into());
        m.insert("playtime".into(), self.playtime.into());
        for (k, v) in &self.extra {
            m.insert(k.clone(), v.clone());
        }
        Value::Object(m)
    }
}

fn read_library() -> Vec<Entry> {
    match read_json(&library_file()) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_object())
            .filter(|m| m.get("id").and_then(|i| i.as_str()).is_some_and(|i| !i.is_empty()))
            .map(Entry::from_json)
            .collect(),
        _ => Vec::new(),
    }
}

fn write_library(lib: &[Entry]) {
    let v = Value::Array(lib.iter().map(|e| e.to_json()).collect());
    if let Err(e) = write_json(&library_file(), &v) {
        eprintln!("vessel: couldn't save the library: {e}");
    }
}

pub fn library() -> Vec<Entry> {
    let _g = LOCK.lock();
    read_library()
}

pub fn get_entry(eid: &str) -> Option<Entry> {
    library().into_iter().find(|e| e.id == eid)
}

/// adds the entry, or replaces the one with its id
pub fn put_entry(entry: &Entry) {
    let _g = LOCK.lock();
    let mut lib = read_library();
    match lib.iter_mut().find(|e| e.id == entry.id) {
        Some(e) => *e = entry.clone(),
        None => lib.push(entry.clone()),
    }
    write_library(&lib);
}

pub fn update_entry(eid: &str, change: impl FnOnce(&mut Entry)) -> Option<Entry> {
    let _g = LOCK.lock();
    let mut lib = read_library();
    let e = lib.iter_mut().find(|e| e.id == eid)?;
    change(e);
    let out = e.clone();
    write_library(&lib);
    Some(out)
}

pub fn remove_entry(eid: &str) {
    let _g = LOCK.lock();
    let lib: Vec<Entry> = read_library().into_iter().filter(|e| e.id != eid).collect();
    write_library(&lib);
}

pub fn add_playtime(eid: &str, seconds: f64) {
    let secs = seconds.max(0.0) as i64;
    update_entry(eid, |e| {
        e.playtime += secs;
        e.last_played = util::now_secs();
    });
}

pub fn remove_file(folder: &str, name: &str) {
    if !name.is_empty() {
        let _ = fs::remove_file(join(folder, name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwords() {
        let rec = hash_password("hunter2");
        let m = rec.as_object().unwrap();
        assert!(password_ok("hunter2", m));
        assert!(!password_ok("hunter3", m));
        assert!(password_ok("anything", &Map::new()));
    }

    #[test]
    fn entry_roundtrip() {
        let v = json!({"id": "abc", "name": "X", "playtime": 12.5, "custom": [1, 2]});
        let e = Entry::from_json(v.as_object().unwrap());
        assert_eq!(e.playtime, 12);
        assert_eq!(e.runner, "native");
        assert_eq!(e.to_json()["custom"], json!([1, 2]));
    }
}
