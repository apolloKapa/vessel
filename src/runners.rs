//! Finding Proton and Wine, building the command that runs an entry, and what's running in a prefix.

use crate::store::{self, Entry, Settings};
use crate::util::{self, basename, dirname, isdir, isfile, join, joins, realpath};
use regex::Regex;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::{LazyLock, Mutex};

pub type Env = BTreeMap<String, String>;

pub const WINDOWS_EXT: &[&str] = &[".exe", ".msi", ".bat", ".cmd", ".lnk"];

pub fn steam_roots() -> Vec<String> {
    let h = util::home();
    vec![join(&h, ".steam/root"), join(&h, ".local/share/Steam"), join(&h, ".steam/steam"),
         join(&h, ".var/app/com.valvesoftware.Steam/.local/share/Steam")]
}

/// the environment for anything Vessel starts: without the AppImage's own paths
pub fn host_env() -> Env {
    let mut env: Env = std::env::vars().collect();
    for key in ["APPDIR", "APPIMAGE", "ARGV0", "OWD", "VESSEL_BSDTAR"] {
        env.remove(key);
    }
    // Vessel's own tools (a downloaded winetricks) come last, after the system's
    let path = env.get("PATH").cloned().unwrap_or_else(|| "/usr/bin:/bin".into());
    env.insert("PATH".into(), format!("{path}:{}", join(&store::tools_dir(), "bin")));
    env
}

pub fn which(name: &str) -> Option<String> {
    util::which_in(name, &host_env()["PATH"])
}

/// a command run quietly to its end: its output, or None
pub fn output(cmd: &[&str], timeout_secs: u64) -> Option<(i32, String)> {
    let mut child = Command::new(cmd[0])
        .args(&cmd[1..])
        .env_clear()
        .envs(host_env())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut out = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut out, &mut s);
        s
    });
    let end = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    let code = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st.code().unwrap_or(-1),
            Ok(None) if std::time::Instant::now() < end => std::thread::sleep(std::time::Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    Some((code, reader.join().unwrap_or_default()))
}

/// runs a command quietly and waits: its exit code (None if it can't start)
pub fn quiet(cmd: &[&str]) -> Option<i32> {
    Command::new(cmd[0])
        .args(&cmd[1..])
        .env_clear()
        .envs(host_env())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()
        .map(|s| s.code().unwrap_or(-1))
}

/// starts a command in its own session, not waited for
pub fn detach(cmd: &[String], env: &Env) -> std::io::Result<Child> {
    let mut c = Command::new(&cmd[0]);
    c.args(&cmd[1..]).env_clear().envs(env).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    unsafe {
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    c.spawn()
}

pub fn open_path(path: &str) {
    let _ = detach(&["xdg-open".into(), path.into()], &host_env()).map(reap);
}

/// waits for a detached child in the background, so it doesn't stay a zombie
pub fn reap(mut child: Child) {
    std::thread::spawn(move || {
        let _ = child.wait();
    });
}

pub fn notify(title: &str, body: &str) {
    let _ = quiet(&["notify-send", "-a", "Vessel", title, body]);
}

pub fn steam_root() -> Option<String> {
    steam_roots().into_iter().find(|r| isdir(&join(r, "steamapps"))).map(|r| realpath(&r))
}

static VDF_PATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""path"\s*"([^"]+)""#).unwrap());

pub fn steam_libraries() -> Vec<String> {
    let mut libs: Vec<String> = Vec::new();
    for root in steam_roots() {
        let Ok(text) = fs::read_to_string(joins(&[&root, "steamapps", "libraryfolders.vdf"])) else { continue };
        for c in VDF_PATH.captures_iter(&text) {
            let p = realpath(&c[1].replace("\\\\", "\\"));
            if !libs.contains(&p) {
                libs.push(p);
            }
        }
    }
    libs
}

/// True when a Windows game ships the Steamworks DLL (steam_api.dll / steam_api64.dll) beside its .exe or
/// in a Unity/Unreal plugin folder: many quit at once when Steam can't be reached
pub fn uses_steamworks(exe: &str) -> bool {
    if !isfile(exe) {
        return false;
    }
    let root = dirname(exe);
    let base = root.trim_end_matches('/').split('/').count();
    let mut found = false;
    util::walk(&root, |folder, dirs, files| {
        if files.iter().any(|f| matches!(f.to_lowercase().as_str(), "steam_api.dll" | "steam_api64.dll")) {
            found = true;
            return false;
        }
        if folder.trim_end_matches('/').split('/').count() - base >= 4 {
            dirs.clear();
        }
        true
    });
    found
}

pub const ARCHES: &[&str] = &["x86_64", "aarch64"];

pub fn machine() -> String {
    let m = std::env::consts::ARCH.to_lowercase();
    match m.as_str() {
        "amd64" => "x86_64".into(),
        "arm64" => "aarch64".into(),
        _ => m,
    }
}

/// the CPU a Proton build/archive name says it's for ("" when it doesn't say)
pub fn arch_of(name: &str) -> &'static str {
    let low = name.to_lowercase();
    ARCHES.iter().find(|a| low.contains(**a)).copied().unwrap_or("")
}

pub fn proton_label(name: &str) -> String {
    let mut n = name.to_string();
    for a in ARCHES {
        n = n.replace(&format!("-{a}"), "");
    }
    n
}

/// where a downloaded GE-Proton goes: Steam's compatibilitytools.d, so Steam can use it too
pub fn proton_install_dir() -> String {
    match steam_root() {
        Some(root) => join(&root, "compatibilitytools.d"),
        None => join(&store::runners_dir(), "proton"),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Version {
    pub name: String,
    pub path: String,
}

/// every Proton on this PC, newest GE-Proton first
pub fn proton_versions() -> Vec<Version> {
    let h = util::home();
    let mut dirs: Vec<String> = steam_roots().iter().map(|r| join(r, "compatibilitytools.d")).collect();
    dirs.extend(steam_libraries().iter().map(|l| joins(&[l, "steamapps", "common"])));
    dirs.push(join(&h, ".config/heroic/tools/proton"));
    dirs.push(join(&store::runners_dir(), "proton"));
    dirs.push("/usr/share/steam/compatibilitytools.d".into());
    let me = machine();
    let mut found: Vec<(String, Version)> = Vec::new();
    for d in dirs {
        for n in util::listdir(&d) {
            let p = join(&d, &n);
            let real = realpath(&p);
            let arch = arch_of(&n);
            // an ARM build on an x86 PC can't run (Exec format error)
            if isfile(&join(&p, "proton")) && !found.iter().any(|(r, _)| *r == real) && (arch.is_empty() || arch == me) {
                found.push((real, Version { name: proton_label(&n), path: p }));
            }
        }
    }
    let (mut ge, mut rest): (Vec<Version>, Vec<Version>) =
        found.into_iter().map(|(_, v)| v).partition(|v| v.name.to_lowercase().starts_with("ge-proton"));
    ge.sort_by(|a, b| util::natural_cmp(&b.name, &a.name));
    rest.sort_by(|a, b| util::natural_cmp(&b.name, &a.name));
    ge.extend(rest);
    ge
}

static WINE_LABEL: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// every Wine on this PC: the system's, then Lutris/Heroic/Bottles/Vessel runners
pub fn wine_versions() -> Vec<Version> {
    let mut out = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    if let Some(system) = which("wine") {
        let label = {
            let cached = WINE_LABEL.lock().unwrap().get(&system).cloned();
            match cached {
                Some(l) => l,
                None => {
                    let ver = output(&[&system, "--version"], 5).map(|(_, o)| o.trim().to_string()).unwrap_or_default();
                    let l = if ver.is_empty() { "System Wine".to_string() } else { format!("System Wine ({ver})") };
                    WINE_LABEL.lock().unwrap().insert(system.clone(), l.clone());
                    l
                }
            }
        };
        seen.push(realpath(&system));
        out.push(Version { name: label, path: system });
    }
    let h = util::home();
    for d in [join(&h, ".local/share/lutris/runners/wine"), join(&h, ".config/heroic/tools/wine"),
              join(&h, ".local/share/bottles/runners"), join(&h, ".var/app/com.usebottles.bottles/data/bottles/runners"),
              join(&store::runners_dir(), "wine")] {
        let mut names = util::listdir(&d);
        names.sort_by(|a, b| util::natural_cmp(b, a));
        for n in names {
            for b in ["wine", "wine64"] {
                let p = joins(&[&d, &n, "bin", b]);
                if isfile(&p) && !seen.contains(&realpath(&p)) {
                    seen.push(realpath(&p));
                    out.push(Version { name: n.clone(), path: p });
                    break;
                }
            }
        }
    }
    out
}

pub fn default_proton(s: &Settings) -> String {
    let vs = proton_versions();
    if !s.proton.is_empty() && vs.iter().any(|v| v.path == s.proton) {
        return s.proton.clone();
    }
    vs.first().map(|v| v.path.clone()).unwrap_or_default()
}

pub fn default_wine(s: &Settings) -> String {
    let vs = wine_versions();
    if !s.wine.is_empty() && vs.iter().any(|v| v.path == s.wine) {
        return s.wine.clone();
    }
    vs.first().map(|v| v.path.clone()).unwrap_or_default()
}

/// the Proton folder or wine binary this entry runs with ("" if none)
pub fn runner_path(e: &Entry, s: &Settings) -> String {
    match e.runner.as_str() {
        "proton" => {
            let arch = arch_of(&basename(&e.version));
            if !e.version.is_empty() && isfile(&join(&e.version, "proton")) && (arch.is_empty() || arch == machine()) {
                return e.version.clone();
            }
            default_proton(s)
        }
        "wine" => {
            if !e.version.is_empty() && isfile(&e.version) {
                return e.version.clone();
            }
            default_wine(s)
        }
        _ => String::new(),
    }
}

pub fn runner_name(r: &str) -> &'static str {
    match r {
        "proton" => "Proton",
        "wine" => "Wine",
        _ => "Native",
    }
}

pub fn runner_label(e: &Entry, s: &Settings) -> String {
    if e.runner == "native" {
        return "Native".into();
    }
    let p = runner_path(e, s);
    if p.is_empty() {
        return format!("{} (missing)", runner_name(&e.runner));
    }
    if e.runner == "proton" {
        return proton_label(&basename(&p));
    }
    wine_versions().into_iter().find(|v| v.path == p).map(|v| v.name).unwrap_or_else(|| "Wine".into())
}

pub fn proton_bin(proton: &str, name: &str) -> String {
    for sub in ["files", "dist"] {
        let p = joins(&[proton, sub, "bin", name]);
        if isfile(&p) {
            return p;
        }
    }
    String::new()
}

pub fn prefix_of(e: &Entry, s: &Settings) -> String {
    if !e.prefix.is_empty() {
        util::expanduser(&e.prefix)
    } else {
        join(&util::expanduser(&s.prefix_root), &util::slug(&e.name))
    }
}

/// the WINEPREFIX itself: <prefix>/pfx (Proton's layout), or the folder if it's a plain wine prefix
pub fn wine_prefix(e: &Entry, s: &Settings) -> String {
    let p = prefix_of(e, s);
    if isdir(&join(&p, "drive_c")) && !isdir(&join(&p, "pfx")) { p } else { join(&p, "pfx") }
}

pub fn is_windows(path: &str) -> bool {
    let low = path.to_lowercase();
    WINDOWS_EXT.iter().any(|x| low.ends_with(x))
}

fn windows_cmd(target: &str) -> Vec<String> {
    let low = target.to_lowercase();
    if low.ends_with(".msi") {
        vec!["msiexec".into(), "/i".into(), target.into()]
    } else if low.ends_with(".bat") || low.ends_with(".cmd") {
        vec!["cmd".into(), "/c".into(), target.into()]
    } else if low.ends_with(".lnk") {
        vec!["start".into(), "/unix".into(), target.into()]
    } else {
        vec![target.into()]
    }
}

#[derive(Debug)]
pub struct LaunchError(pub String);

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<std::io::Error> for LaunchError {
    fn from(e: std::io::Error) -> Self {
        LaunchError(e.to_string())
    }
}

static ENV_TOKEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*=").unwrap());

pub struct LaunchOptions {
    pub env: Vec<(String, String)>,
    pub pre: Vec<String>,
    pub args: Vec<String>,
}

/// Steam-style launch options: 'VAR=1 OTHER=2 gamescope -f -- %command% -vr' puts VAR/OTHER in the
/// environment, gamescope in front of the game and -vr after it. Without %command%, leading VAR=value words
/// are the environment and the rest are arguments, so plain '-windowed -nointro' still works.
pub fn parse_launch_options(text: &str) -> Result<LaunchOptions, LaunchError> {
    let mut words = util::shlex_split(text).map_err(|e| LaunchError(format!("The launch options don't parse: {e}")))?;
    let mut env = Vec::new();
    let take_env = |words: &mut Vec<String>, env: &mut Vec<(String, String)>| {
        while !words.is_empty() && ENV_TOKEN.is_match(&words[0]) {
            let w = words.remove(0);
            let (k, v) = w.split_once('=').unwrap();
            env.push((k.to_string(), v.to_string()));
        }
    };
    if let Some(i) = words.iter().position(|w| w == "%command%") {
        let args: Vec<String> = words[i + 1..].iter().filter(|w| *w != "%command%").cloned().collect();
        let mut before: Vec<String> = words[..i].to_vec();
        take_env(&mut before, &mut env);
        Ok(LaunchOptions { env, pre: before, args })
    } else {
        take_env(&mut words, &mut env);
        Ok(LaunchOptions { env, pre: vec![], args: words })
    }
}

// ---- VR ------------------------------------------------------------------------------------------

fn xrizer_dirs() -> Vec<String> {
    let h = util::home();
    vec!["/opt/xrizer".into(), "/usr/lib/xrizer".into(), "/usr/share/xrizer".into(), join(&h, ".local/share/xrizer"),
         "/opt/opencomposite".into(), join(&h, ".local/share/OpenComposite")]
}

pub fn wivrn_json() -> Vec<String> {
    vec!["/usr/share/openxr/1/openxr_wivrn.json".into(), "/usr/local/share/openxr/1/openxr_wivrn.json".into(),
         join(&util::home(), ".local/share/openxr/1/openxr_wivrn.json")]
}

pub fn xrizer_dir() -> String {
    xrizer_dirs().into_iter().find(|d| isfile(&join(d, "bin/linux64/vrclient.so"))).unwrap_or_default()
}

pub fn wivrn_running() -> bool {
    quiet(&["pgrep", "-x", "wivrn-server"]) == Some(0)
}

/// starts the WiVRn server (its systemd user unit, or the binary); true if it was started
pub fn start_wivrn() -> bool {
    let Some(server) = which("wivrn-server") else { return false };
    if wivrn_running() {
        return false;
    }
    let has_unit = quiet(&["systemctl", "--user", "cat", "wivrn.service"]) == Some(0);
    if has_unit && quiet(&["systemctl", "--user", "start", "wivrn.service"]) == Some(0) {
        return true;
    }
    detach(&[server], &host_env()).map(reap).is_ok()
}

fn runtime_dir() -> String {
    std::env::var("XDG_RUNTIME_DIR").ok().filter(|v| !v.is_empty())
        .unwrap_or_else(|| format!("/run/user/{}", unsafe { libc::getuid() }))
}

/// environment for VR games: WiVRn's OpenXR runtime and xrizer for OpenVR games, as a fallback when WiVRn
/// hasn't set them as active (it does when a headset connects), and what Steam's container needs (umu)
fn vr_env() -> Vec<(String, String)> {
    let mut env = Vec::new();
    let ch = util::config_home();
    if !isfile(&join(&ch, "openxr/1/active_runtime.json")) {
        if let Some(wj) = wivrn_json().into_iter().find(|j| isfile(j)) {
            env.push(("XR_RUNTIME_JSON".into(), wj));
        }
    }
    if !isfile(&join(&ch, "openvr/openvrpaths.vrpath")) {
        let xr = xrizer_dir();
        if !xr.is_empty() {
            env.push(("VR_OVERRIDE".into(), xr));
        }
    }
    env.push(("PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES".into(), "1".into()));
    env.push(("PRESSURE_VESSEL_FILESYSTEMS_RW".into(), join(&runtime_dir(), "wivrn/comp_ipc")));
    env
}

const VR_LIBS: &[&str] = &["openvr_api.dll", "openxr_loader.dll", "xrsdkopenvr.dll", "unityopenxr.dll",
    "libopenvr_api.so", "libopenxr_loader.so", "vrclient.dll", "openvr_api.so"];

/// True when the program's folder ships OpenVR/OpenXR libraries
pub fn looks_vr(exe: &str) -> bool {
    let folder = dirname(exe);
    if !isdir(&folder) || folder == util::home() || folder == "/" {
        return false;
    }
    let base_depth = folder.matches('/').count();
    let mut seen = 0;
    let mut found = false;
    util::walk(&folder, |dirpath, dirs, files| {
        if dirpath.matches('/').count() - base_depth >= 4 {
            dirs.clear();
        }
        for f in files {
            seen += 1;
            if VR_LIBS.contains(&f.to_lowercase().as_str()) {
                found = true;
                return false;
            }
        }
        seen <= 30000
    });
    found
}

// ---- anti-cheat ----------------------------------------------------------------------------------

/// Steam's free anti-cheat runtimes: Proton's Wine loads the Linux side of EAC/BattlEye from the folder in
/// PROTON_EAC_RUNTIME / PROTON_BATTLEYE_RUNTIME
pub struct AntiCheat {
    pub key: &'static str,
    pub label: &'static str,
    pub var: &'static str,
    pub appid: u32,
    pub steam_dir: &'static str,
    pub heroic_dir: &'static str,
}

pub const ANTICHEATS: &[AntiCheat] = &[
    AntiCheat { key: "eac", label: "EasyAntiCheat", var: "PROTON_EAC_RUNTIME", appid: 1826330,
                steam_dir: "Proton EasyAntiCheat Runtime", heroic_dir: "eac_runtime" },
    AntiCheat { key: "battleye", label: "BattlEye", var: "PROTON_BATTLEYE_RUNTIME", appid: 1161040,
                steam_dir: "Proton BattlEye Runtime", heroic_dir: "battleye_runtime" },
];

/// the installed runtime folder ("" if none): Steam's, then Heroic's
pub fn anticheat_runtime(ac: &AntiCheat) -> String {
    let h = util::home();
    let mut dirs: Vec<String> = steam_libraries().iter().map(|l| joins(&[l, "steamapps", "common", ac.steam_dir])).collect();
    for heroic in [join(&h, ".config/heroic"), join(&h, ".var/app/com.heroicgameslauncher.hgl/config/heroic")] {
        dirs.push(joins(&[&heroic, "tools", "runtimes", ac.heroic_dir]));
    }
    dirs.into_iter().find(|d| isdir(d) && !util::listdir(d).is_empty()).unwrap_or_default()
}

/// (eac, battleye): what the program's folder ships
pub fn looks_anticheat(exe: &str) -> (bool, bool) {
    let folder = dirname(exe);
    let (mut eac, mut be) = (false, false);
    if !isdir(&folder) || folder == util::home() || folder == "/" {
        return (eac, be);
    }
    let base_depth = folder.matches('/').count();
    let mut seen = 0;
    util::walk(&folder, |dirpath, dirs, files| {
        if dirpath.matches('/').count() - base_depth >= 3 {
            dirs.clear();
        }
        for n in dirs.iter().chain(files.iter()) {
            let low = n.to_lowercase();
            seen += 1;
            if low == "easyanticheat" || low.starts_with("easyanticheat_") || low.starts_with("start_protected_game") {
                eac = true;
            }
            if low == "battleye" || low.starts_with("beservice") || low.starts_with("beclient") {
                be = true;
            }
        }
        !(seen > 5000 || (eac && be))
    });
    (eac, be)
}

fn entry_has(e: &Entry, key: &str) -> bool {
    match key {
        "eac" => e.eac,
        "battleye" => e.battleye,
        _ => false,
    }
}

fn anticheat_env(e: &Entry, env: &Env) -> Result<Vec<(String, String)>, LaunchError> {
    let mut out = Vec::new();
    let mut shared = Vec::new();
    for ac in ANTICHEATS {
        if !entry_has(e, ac.key) {
            continue;
        }
        let path = anticheat_runtime(ac);
        if path.is_empty() {
            return Err(LaunchError(format!(
                "{} needs the Proton {} Runtime. It's free: install it from Steam (search \"Proton {} Runtime\" with \
                 Tools shown in the library, or open steam://install/{}), or turn {} off in the game's settings.",
                e.name, ac.label, ac.label, ac.appid, ac.label)));
        }
        out.push((ac.var.to_string(), path.clone()));
        shared.push(path);
    }
    if !shared.is_empty() {
        // umu's Steam Runtime container only sees home by default; a Steam library elsewhere must be shared
        let old = env.get("PRESSURE_VESSEL_FILESYSTEMS_RO").cloned().unwrap_or_default();
        let mut all: Vec<String> = old.split(':').filter(|p| !p.is_empty()).map(String::from).collect();
        all.extend(shared);
        out.push(("PRESSURE_VESSEL_FILESYSTEMS_RO".into(), all.join(":")));
    }
    Ok(out)
}

/// the name of the monitor that has focus (niri), or ""
fn focused_output() -> String {
    if std::env::var("NIRI_SOCKET").map(|v| v.is_empty()).unwrap_or(true) || which("niri").is_none() {
        return String::new();
    }
    match output(&["niri", "msg", "-j", "focused-output"], 2) {
        Some((0, out)) => serde_json::from_str::<serde_json::Value>(&out).ok()
            .and_then(|v| v.get("name").and_then(|n| n.as_str()).map(String::from))
            .unwrap_or_default(),
        _ => String::new(),
    }
}

pub struct Launch {
    pub cmd: Vec<String>,
    pub env: Env,
    pub cwd: String,
}

/// the command, environment and working folder to run the entry, or `target` inside its prefix
pub fn build_command(e: &Entry, s: &Settings, target: Option<&str>, extra: Option<&[String]>, tool: bool)
                     -> Result<Launch, LaunchError> {
    let mut env = host_env();
    let exe = util::expanduser(target.unwrap_or(&e.exe)).trim().to_string();
    if exe.is_empty() {
        return Err(LaunchError("No program set. Edit it and pick what to run.".into()));
    }
    let mut opts = parse_launch_options(&e.args)?;
    if e.vr {
        let vr = parse_launch_options(&s.vr_options)?;
        let mut env2 = vr.env;
        env2.extend(opts.env);
        opts.env = env2;
        let mut pre = vr.pre;
        pre.extend(opts.pre);
        opts.pre = pre;
        let mut args = vr.args;
        args.extend(opts.args);
        opts.args = args;
    }
    let args: Vec<String> = match extra {
        Some(x) => x.to_vec(),
        None => opts.args.clone(),
    };
    let is_file = isfile(&exe);
    let cwd = if !e.cwd.is_empty() { util::expanduser(&e.cwd) } else if is_file { dirname(&exe) } else { util::home() };
    let mut pre: Vec<String> = Vec::new();
    let mut cmd: Vec<String>;
    match e.runner.as_str() {
        "native" => {
            if is_file {
                if exe.ends_with(".sh") && !util::is_executable(&exe) {
                    cmd = vec!["sh".into(), exe.clone()];
                } else {
                    if !util::is_executable(&exe) {
                        use std::os::unix::fs::PermissionsExt;
                        let ok = fs::metadata(&exe).and_then(|m| {
                            let mut p = m.permissions();
                            p.set_mode(p.mode() | 0o111);
                            fs::set_permissions(&exe, p)
                        });
                        if ok.is_err() {
                            return Err(LaunchError(format!("{} isn't executable and can't be made so.", basename(&exe))));
                        }
                    }
                    cmd = vec![exe.clone()];
                }
            } else {
                cmd = util::shlex_split(&exe).unwrap_or_default();
                if cmd.is_empty() || !(isfile(&cmd[0]) || util::which_in(&cmd[0], &env["PATH"]).is_some()) {
                    return Err(LaunchError(format!("Can't find {exe}")));
                }
            }
            cmd.extend(args);
            if e.mangohud && which("mangohud").is_some() {
                pre.push("mangohud".into());
            }
        }
        "proton" => {
            let proton = runner_path(e, s);
            if proton.is_empty() {
                return Err(LaunchError("No Proton found. Get GE-Proton in Settings.".into()));
            }
            if target.is_none() && !is_file {
                return Err(LaunchError(format!("{exe} doesn't exist.")));
            }
            let prefix = prefix_of(e, s);
            let mut run = windows_cmd(&exe);
            run.extend(args);
            let mut umu = if s.use_umu { which("umu-run") } else { None };
            if umu.is_some() && steam_root().is_some() && uses_steamworks(&exe) {
                // umu-run blanks STEAM_COMPAT_CLIENT_INSTALL_PATH, so Proton never puts its Steam bridge
                // (steamclient64.dll) into the prefix and SteamAPI_Init fails: such games open and close again
                // (Granny: Legacy). Plain Proton with the Steam folder set reaches the Steam client you're running
                umu = None;
            }
            if let Some(umu) = umu {
                env.insert("WINEPREFIX".into(), join(&prefix, "pfx"));
                env.insert("PROTONPATH".into(), proton.clone());
                env.insert("GAMEID".into(), "umu-0".into());
                env.insert("STORE".into(), "none".into());
                cmd = vec![umu];
                cmd.extend(run);
            } else {
                env.insert("STEAM_COMPAT_DATA_PATH".into(), prefix.clone());
                env.insert("STEAM_COMPAT_CLIENT_INSTALL_PATH".into(), steam_root().unwrap_or_else(|| prefix.clone()));
                env.insert("STEAM_COMPAT_INSTALL_PATH".into(), if isdir(&cwd) { cwd.clone() } else { prefix.clone() });
                env.insert("SteamAppId".into(), "0".into());
                env.insert("SteamGameId".into(), "0".into());
                env.insert("STEAM_COMPAT_APP_ID".into(), "0".into());
                cmd = vec![join(&proton, "proton"), if tool { "run" } else { "waitforexitandrun" }.into()];
                cmd.extend(run);
            }
            if e.mangohud {
                env.insert("MANGOHUD".into(), "1".into());
            }
            for (k, v) in anticheat_env(e, &env)? {
                env.insert(k, v);
            }
            if store::wayland_on(s) && !e.vr {
                // a native Wayland window: X11 windows get lost on monitors at negative positions (niri). Not for
                // VR games: Proton's OpenVR/DXVK bridge needs X11 (D3D11 fails to create its device otherwise)
                env.insert("PROTON_ENABLE_WAYLAND".into(), "1".into());
                // Wine takes the topmost monitor as the main one, so fullscreen games jumped to it: make the main
                // one the monitor you launch from (GE-Proton's Wayland driver reads this; the game's own env wins)
                let focused = focused_output();
                if !focused.is_empty() {
                    env.entry("WAYLANDDRV_PRIMARY_MONITOR".into()).or_insert(focused);
                }
            }
        }
        "wine" => {
            let wine = runner_path(e, s);
            if wine.is_empty() {
                return Err(LaunchError("No Wine found. Install wine, or pick Proton instead.".into()));
            }
            if target.is_none() && !is_file {
                return Err(LaunchError(format!("{exe} doesn't exist.")));
            }
            env.insert("WINEPREFIX".into(), wine_prefix(e, s));
            let path = format!("{}:{}", dirname(&wine), env["PATH"]);
            env.insert("PATH".into(), path);
            env.entry("WINEDEBUG".into()).or_insert_with(|| "-all".into());
            cmd = vec![wine.clone()];
            cmd.extend(windows_cmd(&exe));
            cmd.extend(args);
            if e.mangohud {
                env.insert("MANGOHUD".into(), "1".into());
            }
            // only Proton-based Wine builds (GE/soda) read these
            for (k, v) in anticheat_env(e, &env)? {
                env.insert(k, v);
            }
        }
        other => return Err(LaunchError(format!("Unknown runner {other}"))),
    }
    if e.gamemode && which("gamemoderun").is_some() {
        pre.insert(0, "gamemoderun".into());
    }
    if extra.is_none() {
        let mut p = opts.pre.clone();
        p.extend(pre);
        pre = p;
    }
    if !e.wrapper.trim().is_empty() {
        let mut w = util::shlex_split(&e.wrapper).map_err(|x| LaunchError(format!("The command prefix doesn't parse: {x}")))?;
        w.extend(pre);
        pre = w;
    }
    if e.vr {
        for (k, v) in vr_env() {
            env.insert(k, v);
        }
    }
    for line in e.env.lines() {
        if line.contains('=') && !line.trim().starts_with('#') {
            let (k, v) = line.split_once('=').unwrap();
            if !k.trim().is_empty() {
                env.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }
    for (k, v) in opts.env {
        env.insert(k, v);
    }
    pre.extend(cmd);
    let cwd = if isdir(&cwd) { cwd } else { util::home() };
    Ok(Launch { cmd: pre, env, cwd })
}

pub fn start_process(e: &Entry, s: &Settings, target: Option<&str>, extra: Option<&[String]>, tool: bool,
                     log_name: Option<&str>) -> Result<Child, LaunchError> {
    let l = build_command(e, s, target, extra, tool)?;
    if e.runner == "proton" {
        fs::create_dir_all(prefix_of(e, s))?;
    } else if e.runner == "wine" {
        fs::create_dir_all(&l.env["WINEPREFIX"])?;
    }
    fs::create_dir_all(store::logs_dir())?;
    let path = join(&store::logs_dir(), &format!("{}.log", log_name.unwrap_or(&e.id)));
    let mut log = fs::File::create(&path)?;
    write!(log, "# {}\n# {}\n# in {}\n\n", util::ctime(util::now_secs()), util::shlex_join(&l.cmd), l.cwd)?;
    log.flush()?;
    let err = log.try_clone()?;
    let mut c = Command::new(&l.cmd[0]);
    c.args(&l.cmd[1..]).env_clear().envs(&l.env).current_dir(&l.cwd)
        .stdin(Stdio::null()).stdout(Stdio::from(log)).stderr(Stdio::from(err));
    unsafe {
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    c.spawn().map_err(|x| LaunchError(format!("{}: {x}", l.cmd[0])))
}

/// (environment, wineserver) for wine tools (winetricks, wineserver -k) on the entry's prefix
pub fn wine_tool_env(e: &Entry, s: &Settings) -> (Env, String) {
    let mut env = host_env();
    let path = runner_path(e, s);
    let (wine, server);
    if e.runner == "proton" {
        wine = proton_bin(&path, "wine");
        server = proton_bin(&path, "wineserver");
        env.insert("WINEPREFIX".into(), join(&prefix_of(e, s), "pfx"));
    } else {
        wine = path.clone();
        let beside = join(&dirname(&path), "wineserver");
        server = if isfile(&beside) { beside } else { which("wineserver").unwrap_or_default() };
        env.insert("WINEPREFIX".into(), wine_prefix(e, s));
    }
    if !wine.is_empty() {
        env.insert("WINE".into(), wine.clone());
        let p = format!("{}:{}", dirname(&wine), env["PATH"]);
        env.insert("PATH".into(), p);
    }
    if !server.is_empty() {
        env.insert("WINESERVER".into(), server.clone());
    }
    (env, server)
}

pub fn kill_prefix(e: &Entry, s: &Settings) {
    if e.runner == "native" {
        return;
    }
    let (env, server) = wine_tool_env(e, s);
    if !server.is_empty() && isdir(&env["WINEPREFIX"]) {
        let _ = detach(&[server, "-k".into()], &env).map(reap);
    }
}

fn environ(pid: &str) -> Vec<Vec<u8>> {
    match fs::read(format!("/proc/{pid}/environ")) {
        Ok(b) => b.split(|c| *c == 0).map(|x| x.to_vec()).collect(),
        Err(_) => Vec::new(),
    }
}

/// does a process environment point into the prefix folder `root` (realpath, ending in /)?
fn in_prefix(env: &[Vec<u8>], root: &str) -> bool {
    for kv in env {
        for key in [&b"WINEPREFIX="[..], &b"STEAM_COMPAT_DATA_PATH="[..]] {
            if let Some(v) = kv.strip_prefix(key) {
                let v = String::from_utf8_lossy(v);
                if !v.is_empty() && format!("{}/", realpath(&v).trim_end_matches('/')).starts_with(root) {
                    return true;
                }
            }
        }
    }
    false
}

fn prefix_root(e: &Entry, s: &Settings) -> String {
    format!("{}/", realpath(&prefix_of(e, s)).trim_end_matches('/'))
}

fn pids() -> Vec<String> {
    util::listdir("/proc").into_iter().filter(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit())).collect()
}

/// pids of what's running in the entry's prefix: its Windows programs, wineserver, Proton's script (from
/// their environment). Proton's waitforexitandrun waits for all of it to end first, so a run that crashed or
/// hung there kept every new start from loading
pub fn prefix_procs(e: &Entry, s: &Settings) -> Vec<i32> {
    if e.runner == "native" {
        return Vec::new();
    }
    let root = prefix_root(e, s);
    let me = std::process::id().to_string();
    pids().into_iter().filter(|d| *d != me && in_prefix(&environ(d), &root)).filter_map(|d| d.parse().ok()).collect()
}

/// stops everything in the entry's prefix (politely, then for good), waiting up to `wait` seconds
pub fn stop_prefix(e: &Entry, s: &Settings, wait: f64) {
    kill_prefix(e, s);
    for pid in prefix_procs(e, s) {
        unsafe { libc::kill(pid, libc::SIGTERM) };
    }
    let end = util::now() + wait;
    while util::now() < end && !prefix_procs(e, s).is_empty() {
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    for pid in prefix_procs(e, s) {
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }
}

/// the pids of the windows on screen, from the compositor (Hyprland, niri); None when it can't say
fn window_pids() -> Option<Vec<i64>> {
    let set = |k: &str| std::env::var(k).is_ok_and(|v| !v.is_empty());
    let (cmd, key): (&[&str], &str) = if set("HYPRLAND_INSTANCE_SIGNATURE") && which("hyprctl").is_some() {
        (&["hyprctl", "clients", "-j"], "pid")
    } else if set("NIRI_SOCKET") && which("niri").is_some() {
        (&["niri", "msg", "-j", "windows"], "pid")
    } else {
        return None;
    };
    let (_, out) = output(cmd, 2)?;
    let v: serde_json::Value = serde_json::from_str(if out.trim().is_empty() { "[]" } else { &out }).ok()?;
    Some(v.as_array()?.iter().map(|c| c.get(key).and_then(|p| p.as_i64()).unwrap_or(0)).collect())
}

fn parents() -> HashMap<i64, i64> {
    let mut out = HashMap::new();
    for d in pids() {
        if let Ok(stat) = fs::read_to_string(format!("/proc/{d}/stat")) {
            if let Some(rest) = stat.rsplit_once(')').map(|x| x.1) {
                if let (Ok(pid), Some(Ok(pp))) = (d.parse::<i64>(), rest.split_whitespace().nth(1).map(|x| x.parse::<i64>())) {
                    out.insert(pid, pp);
                }
            }
        }
    }
    out
}

/// Some(true) once the entry's program shows a window (one of its prefix's, or a child of root_pid);
/// None when the compositor can't be asked
pub fn has_window(e: &Entry, root_pid: Option<u32>, s: &Settings) -> Option<bool> {
    let pids = window_pids()?;
    if e.runner != "native" {
        let root = prefix_root(e, s);
        if pids.iter().any(|p| *p != 0 && in_prefix(&environ(&p.to_string()), &root)) {
            return Some(true);
        }
    }
    if let Some(root_pid) = root_pid {
        let par = parents();
        for &p0 in &pids {
            let mut p = p0;
            let mut seen = 0;
            while p != 0 && p != 1 && seen < 64 {
                if p == root_pid as i64 {
                    return Some(true);
                }
                p = par.get(&p).copied().unwrap_or(0);
                seen += 1;
            }
        }
    }
    Some(false)
}

pub fn kill_group(pid: u32, sig: i32) {
    unsafe { libc::killpg(pid as i32, sig) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_options() {
        let o = parse_launch_options("VAR=1 OTHER=2 gamescope -f -- %command% -vr").unwrap();
        assert_eq!(o.env, vec![("VAR".into(), "1".into()), ("OTHER".into(), "2".into())]);
        assert_eq!(o.pre, vec!["gamescope", "-f", "--"]);
        assert_eq!(o.args, vec!["-vr"]);
        let o = parse_launch_options("A=b -windowed -nointro").unwrap();
        assert_eq!(o.env.len(), 1);
        assert_eq!(o.args, vec!["-windowed", "-nointro"]);
    }
}
