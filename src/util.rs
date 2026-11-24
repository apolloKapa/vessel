//! Small helpers: paths, shell words, natural sorting, names from file names, time.

use regex::Regex;
use std::cmp::Ordering;
use std::fs;
use std::path::Path;
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

// ---- paths (as strings, like the library stores them) ------------------------------------------

pub fn home() -> String {
    std::env::var("HOME").ok().filter(|h| !h.is_empty()).unwrap_or_else(|| "/".into())
}

pub fn xdg(var: &str, fallback: &str) -> String {
    std::env::var(var).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| join(&home(), fallback))
}

pub fn config_home() -> String {
    xdg("XDG_CONFIG_HOME", ".config")
}

pub fn data_home() -> String {
    xdg("XDG_DATA_HOME", ".local/share")
}

/// os.path.join: an absolute second part replaces the first
pub fn join(a: &str, b: &str) -> String {
    if b.starts_with('/') || a.is_empty() {
        return b.to_string();
    }
    if a.ends_with('/') { format!("{a}{b}") } else { format!("{a}/{b}") }
}

pub fn joins(parts: &[&str]) -> String {
    let mut out = String::new();
    for p in parts {
        out = join(&out, p);
    }
    out
}

/// os.path.dirname
pub fn dirname(p: &str) -> String {
    match p.rfind('/') {
        Some(0) => "/".into(),
        Some(i) => {
            let head = &p[..i];
            let trimmed = head.trim_end_matches('/');
            if trimmed.is_empty() { "/".into() } else { trimmed.to_string() }
        }
        None => String::new(),
    }
}

/// os.path.basename
pub fn basename(p: &str) -> String {
    match p.rfind('/') {
        Some(i) => p[i + 1..].to_string(),
        None => p.to_string(),
    }
}

/// os.path.splitext(basename)[0]
pub fn stem(p: &str) -> String {
    let b = basename(p);
    let lead = b.len() - b.trim_start_matches('.').len();
    match b[lead..].rfind('.') {
        Some(i) => b[..lead + i].to_string(),
        None => b,
    }
}

pub fn expanduser(p: &str) -> String {
    if p == "~" {
        return home();
    }
    if let Some(rest) = p.strip_prefix("~/") {
        return join(&home(), rest);
    }
    p.to_string()
}

pub fn exists(p: &str) -> bool {
    !p.is_empty() && fs::metadata(p).is_ok()
}

pub fn lexists(p: &str) -> bool {
    !p.is_empty() && fs::symlink_metadata(p).is_ok()
}

pub fn isfile(p: &str) -> bool {
    !p.is_empty() && fs::metadata(p).map(|m| m.is_file()).unwrap_or(false)
}

pub fn isdir(p: &str) -> bool {
    !p.is_empty() && fs::metadata(p).map(|m| m.is_dir()).unwrap_or(false)
}

pub fn islink(p: &str) -> bool {
    fs::symlink_metadata(p).map(|m| m.file_type().is_symlink()).unwrap_or(false)
}

/// os.path.realpath: canonical when it exists, else made absolute as it is
pub fn realpath(p: &str) -> String {
    match fs::canonicalize(p) {
        Ok(r) => r.to_string_lossy().into_owned(),
        Err(_) => abspath(p),
    }
}

pub fn abspath(p: &str) -> String {
    let full = if p.starts_with('/') {
        p.to_string()
    } else {
        let cwd = std::env::current_dir().map(|c| c.to_string_lossy().into_owned()).unwrap_or_else(|_| "/".into());
        join(&cwd, p)
    };
    normpath(&full)
}

pub fn normpath(p: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in p.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            x => parts.push(x),
        }
    }
    format!("/{}", parts.join("/"))
}

pub fn is_executable(p: &str) -> bool {
    use std::ffi::CString;
    match CString::new(p) {
        Ok(c) => unsafe { libc::access(c.as_ptr(), libc::X_OK) == 0 },
        Err(_) => false,
    }
}

pub fn listdir(p: &str) -> Vec<String> {
    match fs::read_dir(p) {
        Ok(rd) => rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect(),
        Err(_) => Vec::new(),
    }
}

/// shutil.which over a given PATH
pub fn which_in(name: &str, path: &str) -> Option<String> {
    if name.contains('/') {
        return (isfile(name) && is_executable(name)).then(|| name.to_string());
    }
    for dir in path.split(':') {
        if dir.is_empty() {
            continue;
        }
        let cand = join(dir, name);
        if isfile(&cand) && is_executable(&cand) {
            return Some(cand);
        }
    }
    None
}

/// os.walk, top-down: `visit(dirpath, dirnames, filenames)` may prune `dirnames`; it returns false to stop.
/// Symlinked folders are listed in dirnames but not entered (followlinks=False).
pub fn walk(root: &str, mut visit: impl FnMut(&str, &mut Vec<String>, &[String]) -> bool) {
    let mut stack = vec![root.to_string()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        for ent in rd.flatten() {
            let name = ent.file_name().to_string_lossy().into_owned();
            let is_dir = fs::metadata(ent.path()).map(|m| m.is_dir()).unwrap_or(false);
            if is_dir { dirs.push(name) } else { files.push(name) }
        }
        if !visit(&dir, &mut dirs, &files) {
            return;
        }
        for d in dirs.iter().rev() {
            let full = join(&dir, d);
            if !islink(&full) {
                stack.push(full);
            }
        }
    }
}

// ---- shell words (Python's shlex, POSIX mode) ---------------------------------------------------

pub fn shlex_split(s: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' | '\r' | '\n' => {
                if in_word {
                    out.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            '\\' => {
                in_word = true;
                match chars.next() {
                    Some(n) => cur.push(n),
                    None => return Err("No escaped character".into()),
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(n) => cur.push(n),
                        None => return Err("No closing quotation".into()),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.peek() {
                            Some('"') | Some('\\') => cur.push(chars.next().unwrap()),
                            Some(_) => cur.push('\\'),
                            None => return Err("No closing quotation".into()),
                        },
                        Some(n) => cur.push(n),
                        None => return Err("No closing quotation".into()),
                    }
                }
            }
            _ => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        out.push(cur);
    }
    Ok(out)
}

pub fn shlex_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".into();
    }
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c)) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

pub fn shlex_join<S: AsRef<str>>(words: &[S]) -> String {
    words.iter().map(|w| shlex_quote(w.as_ref())).collect::<Vec<_>>().join(" ")
}

// ---- natural order ---------------------------------------------------------------------------

/// compares like Python's [int(t) if t.isdigit() else t.lower() for t in re.split(r"(\d+)", s)]
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let ka = natural_key(a);
    let kb = natural_key(b);
    for (x, y) in ka.iter().zip(kb.iter()) {
        let o = match (x, y) {
            (Chunk::Num(p), Chunk::Num(q)) => {
                let p = p.trim_start_matches('0');
                let q = q.trim_start_matches('0');
                p.len().cmp(&q.len()).then_with(|| p.cmp(q))
            }
            (Chunk::Text(p), Chunk::Text(q)) => p.cmp(q),
            (Chunk::Num(_), Chunk::Text(_)) => Ordering::Less,
            (Chunk::Text(_), Chunk::Num(_)) => Ordering::Greater,
        };
        if o != Ordering::Equal {
            return o;
        }
    }
    ka.len().cmp(&kb.len())
}

enum Chunk {
    Text(String),
    Num(String),
}

fn natural_key(s: &str) -> Vec<Chunk> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut digits = false;
    for c in s.chars() {
        let d = c.is_numeric();
        if d != digits {
            out.push(if digits { Chunk::Num(std::mem::take(&mut cur)) } else { Chunk::Text(std::mem::take(&mut cur).to_lowercase()) });
            digits = d;
        }
        cur.push(c);
    }
    out.push(if digits { Chunk::Num(cur) } else { Chunk::Text(cur.to_lowercase()) });
    if digits {
        out.push(Chunk::Text(String::new()));
    }
    out
}

// ---- names -----------------------------------------------------------------------------------

macro_rules! re {
    ($name:ident, $pat:expr) => {
        pub static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pat).unwrap());
    };
}

re!(NON_ALNUM, r"[^A-Za-z0-9]+");
re!(NOT_LOWER_ALNUM, r"[^a-z0-9]");
re!(SEPARATORS, r"[-_.]+");
re!(ARCH_WORDS, r"(?i)\b(x64|x86|x86 64|win64|win32|appimage|amd64|linux)\b");
re!(TRAILING_VERSION, r"(\s+v?\d+(\s+\d+)*)+\s*$");
re!(SPACES, r"\s+");
re!(ARCHIVE_SUFFIX, r"(?i)(\.part0*1)?\.(zip|rar|7z|tar|tgz|txz|tbz2|tar\.\w+|7z\.001)$");
re!(BRACKETS, r"[\[({].*?[\])}]");
re!(VERSION_TAIL, r"(?i)\b(v|ver|version|build)\s*\d[\d ]*\b.*$");
re!(RELEASE_WORDS, r"(?i)\b(x64|x86|win64|win32|windows|linux|portable|repack|gog|steamrip|multi\d*)\b");
re!(TRAILING_NUMBERS, r"(\s+\d+)+\s*$");
re!(BAD_DIR_CHARS, r#"[/\\:*?"<>|]+"#);

pub fn slug(name: &str) -> String {
    let s = NON_ALNUM.replace_all(name, "-").trim_matches('-').to_lowercase();
    if s.is_empty() { "game".into() } else { s }
}

fn squash(s: &str) -> String {
    NOT_LOWER_ALNUM.replace_all(&s.to_lowercase(), "").into_owned()
}

/// a space between a lower-case letter and the capital after it (CamelCase → Camel Case)
fn split_camel(s: &str) -> String {
    let mut out = String::new();
    let mut prev_lower = false;
    for c in s.chars() {
        if prev_lower && c.is_ascii_uppercase() {
            out.push(' ');
        }
        out.push(c);
        prev_lower = c.is_ascii_lowercase();
    }
    out
}

/// Python's str.islower(): has cased letters and none of them upper-case
fn is_lower(s: &str) -> bool {
    s.chars().any(|c| c.is_lowercase()) && !s.chars().any(|c| c.is_uppercase())
}

fn cap_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// a name from an executable's path: its file name, or its folder's for generic names
pub fn guess_name(path: &str) -> String {
    let base = stem(path);
    const GENERIC: &[&str] = &["game", "launcher", "start", "play", "setup", "run", "main", "app", "client", "win64",
        "bin", "binaries", "x64", "shipping"];
    let mut words = SEPARATORS.replace_all(&base, " ").trim().to_string();
    let mut parent = path.to_string();
    if GENERIC.contains(&squash(&base).as_str()) || base.to_lowercase().ends_with("-win64-shipping") {
        for _ in 0..4 {
            parent = dirname(&parent);
            let pname = basename(&parent);
            if !pname.is_empty() && !GENERIC.contains(&squash(&pname).as_str()) {
                words = SEPARATORS.replace_all(&pname, " ").trim().to_string();
                break;
            }
        }
    }
    let words = split_camel(&words);
    let words = ARCH_WORDS.replace_all(&words, "").into_owned();
    let words = TRAILING_VERSION.replace_all(words.trim(), "").into_owned();
    let mut words = SPACES.replace_all(&words, " ").trim().to_string();
    if words.is_empty() {
        words = base;
    }
    if is_lower(&words) { cap_first(&words) } else { words }
}

/// a game name from an archive's file name: without extensions, part numbers, [tags] and versions
pub fn archive_name(path: &str) -> String {
    let full = basename(path);
    let base = ARCHIVE_SUFFIX.replace(&full, "").into_owned();
    let base = BRACKETS.replace_all(&base, " ").into_owned();
    let base = SEPARATORS.replace_all(&base, " ").into_owned();
    let base = VERSION_TAIL.replace(&base, "").into_owned();
    let base = RELEASE_WORDS.replace_all(&base, "").into_owned();
    let base = TRAILING_NUMBERS.replace(base.trim(), "").into_owned();
    let mut base = SPACES.replace_all(&base, " ").trim().to_string();
    if base.is_empty() {
        base = full;
    }
    if is_lower(&base) { cap_first(&base) } else { base }
}

pub fn unique_dir(parent: &str, name: &str) -> String {
    let mut name = BAD_DIR_CHARS.replace_all(name, " ").trim().to_string();
    if name.is_empty() {
        name = "Game".into();
    }
    let mut path = join(parent, &name);
    let mut n = 2;
    while lexists(&path) {
        path = join(parent, &format!("{name} ({n})"));
        n += 1;
    }
    path
}

// ---- time ------------------------------------------------------------------------------------

pub fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

pub fn now_secs() -> i64 {
    now() as i64
}

fn local_tm(ts: i64) -> libc::tm {
    let t: libc::time_t = ts as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    tm
}

pub fn strftime(fmt: &str, ts: i64) -> String {
    let tm = local_tm(ts);
    let mut buf = vec![0u8; 128];
    let cfmt = std::ffi::CString::new(fmt).unwrap_or_default();
    let n = unsafe { libc::strftime(buf.as_mut_ptr() as *mut libc::c_char, buf.len(), cfmt.as_ptr(), &tm) };
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

/// time.ctime()
pub fn ctime(ts: i64) -> String {
    let tm = local_tm(ts);
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{} {} {:2} {:02}:{:02}:{:02} {}", DAYS[tm.tm_wday as usize % 7], MONTHS[tm.tm_mon as usize % 12],
            tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec, tm.tm_year + 1900)
}

fn local_midnight(ts: i64) -> i64 {
    let mut tm = local_tm(ts);
    tm.tm_hour = 0;
    tm.tm_min = 0;
    tm.tm_sec = 0;
    tm.tm_isdst = -1;
    unsafe { libc::mktime(&mut tm) as i64 }
}

pub fn human_time(secs: i64) -> String {
    if secs < 60 {
        return if secs > 0 { "under a minute".into() } else { "never".into() };
    }
    let (h, m) = ((secs / 60) / 60, (secs / 60) % 60);
    if h > 0 { format!("{h}h {m}m") } else { format!("{m}m") }
}

pub fn human_when(ts: i64) -> String {
    if ts == 0 {
        return "never".into();
    }
    let days = (local_midnight(now_secs()) - local_midnight(ts)).div_euclid(86400);
    match days {
        d if d <= 0 => "today".into(),
        1 => "yesterday".into(),
        d if d < 30 => format!("{d} days ago"),
        _ => strftime("%d %b %Y", ts),
    }
}

pub fn random_hex(n: usize) -> String {
    let mut buf = vec![0u8; n.div_ceil(2)];
    let _ = getrandom::fill(&mut buf);
    let mut s = hex::encode(buf);
    s.truncate(n);
    s
}

pub fn new_id() -> String {
    random_hex(12)
}

pub fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(guess_name("/g/Hollow Knight/hollow_knight.exe"), "Hollow knight");
        assert_eq!(guess_name("/g/Some Game/Binaries/Win64/Game-Win64-Shipping.exe"), "Some Game");
        assert_eq!(guess_name("/x/SuperMeatBoy.exe"), "Super Meat Boy");
        assert_eq!(guess_name("/x/game_v1.2.exe"), "Game");
        assert_eq!(archive_name("/d/Cool_Game-v1.2.3 [GOG].part1.rar"), "Cool Game");
        assert_eq!(archive_name("/d/hades.zip"), "Hades");
    }

    #[test]
    fn shell() {
        assert_eq!(shlex_split(r#"VAR=1 "a b" 'c d' e\ f %command%"#).unwrap(),
                   vec!["VAR=1", "a b", "c d", "e f", "%command%"]);
        assert!(shlex_split("'oops").is_err());
        assert_eq!(shlex_join(&["a b", "c", "it's"]), r#"'a b' c 'it'"'"'s'"#);
    }

    #[test]
    fn natural() {
        let mut v = vec!["GE-Proton10-9", "GE-Proton10-10", "GE-Proton9-27"];
        v.sort_by(|a, b| natural_cmp(b, a));
        assert_eq!(v, vec!["GE-Proton10-10", "GE-Proton10-9", "GE-Proton9-27"]);
    }

    #[test]
    fn paths() {
        assert_eq!(dirname("/a/b/c.exe"), "/a/b");
        assert_eq!(dirname("/a"), "/");
        assert_eq!(basename("/a/b/c.exe"), "c.exe");
        assert_eq!(stem("/a/b/c.tar.gz"), "c.tar");
        assert_eq!(join("/a", "/b"), "/b");
    }
}

re!(NUMBERS, r"\d+");
