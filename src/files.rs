//! Archives, folders, installers and what's inside Windows programs (icons, version info).

use crate::runners;
use crate::store::Entry;
use crate::util::{self, basename, dirname, isdir, isfile, join};
use regex::Regex;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::process::{Command, Stdio};
use std::sync::{LazyLock, Mutex};

/// progress of a long job, for a progress bar
#[derive(Clone, Debug)]
pub enum Progress {
    /// (stage, done, total): unpacking files, copying items
    #[allow(dead_code)]
    Count(&'static str, u64, u64),
    /// downloading (done bytes, total bytes, what)
    Download(u64, u64, String),
    /// unpacking a download (what)
    Unpack(String),
}

pub type Report<'a> = &'a dyn Fn(Progress);

static ARCHIVE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\.(zip|rar|7z|tar|tgz|txz|tbz2|tar\.(gz|xz|bz2|zst|lz4)|7z\.001)$").unwrap());
static LATER_PART_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(\.part0*([2-9]|\d{2,})\.rar|\.r\d\d|\.z\d\d)$").unwrap());
static SEVENZ_PART: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\.7z\.(\d{3})$").unwrap());
pub const IMAGE_EXT: &[&str] = &[".png", ".jpg", ".jpeg", ".webp", ".bmp"];
static SKIP_PROGRAMS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?i)unins|uninstall|crash|redist|vc_?redist|dxsetup|dxwebsetup|directx|dotnet|ue4prereq|prereq|physx|",
        r"easyanticheat|battleye|notification_helper|qtwebengineprocess|cefsharp|subprocess|helper|reporter|",
        r"vcrun|oalinst|xnafx|7z(a|g|fm)?\.exe|unitycrashhandler|winrar|unrar|dosbox_?debug"
    )).unwrap()
});
static INSTALLER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(setup|install|installer)([-_ .][^/]*)?\.exe$|[-_ ]setup[^/]*\.exe$|^setup_.*\.exe$").unwrap()
});

pub fn is_image(path: &str) -> bool {
    let low = path.to_lowercase();
    IMAGE_EXT.iter().any(|x| low.ends_with(x))
}

/// a later part of a split archive (.part2.rar, .r01, .7z.002, .z01): the first part unpacks them all
pub fn is_later_part(path: &str) -> bool {
    LATER_PART_RE.is_match(path) || SEVENZ_PART.captures(path).is_some_and(|c| &c[1] != "001")
}

pub fn is_archive(path: &str) -> bool {
    ARCHIVE_RE.is_match(path) && !is_later_part(path)
}

// an installer's own messages in its string table: two different ones are needed (a game's own strings can
// say "installer", French for "install": PlayRDR2.exe's "veuillez réinstaller le jeu")
const INSTALLER_PHRASES: &[&str] = &[r"\bInstalling\b", r"setup wizard", r"installation wizard", r"license agreement",
    r"destination (folder|directory|location)", r"select (a|the) (location|folder) (to|where)",
    r"\bready to install\b", r"\binstallation (is )?complete"];
static INSTALLER_STRINGS: LazyLock<Vec<Regex>> =
    LazyLock::new(|| INSTALLER_PHRASES.iter().map(|p| Regex::new(&format!("(?i){p}")).unwrap()).collect());
static INSTALLER_TEXT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(setup|installer|install|installation)\b").unwrap());
static VERSION_FIELDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:FileDescription|ProductName|InternalName|OriginalFilename)\x00+([^\x00]+)").unwrap()
});
// what installer builders leave in the file: NSIS's manifest name, Inno's data block, InstallShield's name
const INSTALLER_MARKS: &[&[u8]] = &[b"Nullsoft.NSIS.exehead", b"Inno Setup Setup Data", b"InstallShield"];

pub fn installer_phrase_count(text: &str) -> usize {
    INSTALLER_STRINGS.iter().filter(|r| r.is_match(text)).count()
}

// ---- reading Windows programs ------------------------------------------------------------------

struct Pe {
    f: fs::File,
    sections: Vec<(u32, u32, u32)>,
    names: Vec<String>,
    res_base: Option<u64>,
}

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

fn bad() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, "bad PE")
}

impl Pe {
    fn open(path: &str) -> std::io::Result<Option<Pe>> {
        let mut f = fs::File::open(path)?;
        let mut head = [0u8; 64];
        if f.read(&mut head)? < 64 || &head[..2] != b"MZ" {
            return Ok(None);
        }
        let pe = u32_at(&head, 0x3C).ok_or_else(bad)? as u64;
        f.seek(SeekFrom::Start(pe))?;
        let mut hdr = vec![0u8; 24 + 240 + 40 * 96];
        let n = read_up_to(&mut f, &mut hdr)?;
        hdr.truncate(n);
        if hdr.len() < 24 || &hdr[..4] != b"PE\0\0" {
            return Ok(None);
        }
        let nsec = u16_at(&hdr, 6).ok_or_else(bad)? as usize;
        let opt_size = u16_at(&hdr, 20).ok_or_else(bad)? as usize;
        let magic = u16_at(&hdr, 24).ok_or_else(bad)?;
        let dd = 24 + if magic == 0x10B { 96 } else { 112 };
        let res_rva = u32_at(&hdr, dd + 16).ok_or_else(bad)?;
        let mut sections = Vec::new();
        let mut names = Vec::new();
        for i in 0..nsec.min(96) {
            let o = 24 + opt_size + 40 * i;
            if o + 40 > hdr.len() {
                break;
            }
            let raw = &hdr[o..o + 8];
            let end = raw.iter().position(|c| *c == 0).unwrap_or(8);
            names.push(raw[..end].iter().map(|c| *c as char).collect());
            let vsize = u32_at(&hdr, o + 8).unwrap();
            let vaddr = u32_at(&hdr, o + 12).unwrap();
            let rawsize = u32_at(&hdr, o + 16).unwrap();
            let rawptr = u32_at(&hdr, o + 20).unwrap();
            sections.push((vaddr, vsize.max(rawsize), rawptr));
        }
        let mut pe = Pe { f, sections, names, res_base: None };
        if res_rva != 0 {
            pe.res_base = Some(pe.off_of(res_rva)?);
        }
        Ok(Some(pe))
    }

    fn at(&mut self, off: u64, n: usize) -> std::io::Result<Vec<u8>> {
        self.f.seek(SeekFrom::Start(off))?;
        let mut b = vec![0u8; n];
        self.f.read_exact(&mut b)?;
        Ok(b)
    }

    fn off_of(&self, rva: u32) -> std::io::Result<u64> {
        for &(va, size, raw) in &self.sections {
            if va <= rva && (rva as u64) < va as u64 + size as u64 {
                return Ok(raw as u64 + (rva - va) as u64);
            }
        }
        Err(bad())
    }

    /// a resource directory's entries: (name or id, offset)
    fn entries(&mut self, rel: u32, cap: usize) -> std::io::Result<Vec<(u32, u32)>> {
        let base = self.res_base.ok_or_else(bad)?;
        let b = self.at(base + rel as u64 + 12, 4)?;
        let count = (u16_at(&b, 0).unwrap() as usize + u16_at(&b, 2).unwrap() as usize).min(cap);
        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            let e = self.at(base + rel as u64 + 16 + 8 * i as u64, 8)?;
            out.push((u32_at(&e, 0).unwrap(), u32_at(&e, 4).unwrap()));
        }
        Ok(out)
    }

    /// (rva, size) of a data entry
    fn data_entry(&mut self, off: u32) -> std::io::Result<(u32, u32)> {
        let base = self.res_base.ok_or_else(bad)?;
        let b = self.at(base + off as u64, 8)?;
        Ok((u32_at(&b, 0).unwrap(), u32_at(&b, 4).unwrap()))
    }

    fn leaves(&mut self, node: u32, depth: u32, out: &mut Vec<(u32, u32)>) -> std::io::Result<()> {
        if out.len() >= 64 {
            return Ok(());
        }
        if node & 0x8000_0000 != 0 {
            if depth < 3 {
                for (_, sub) in self.entries(node & 0x7FFF_FFFF, 512)? {
                    self.leaves(sub, depth + 1, out)?;
                }
            }
        } else {
            out.push(self.data_entry(node)?);
        }
        Ok(())
    }

    fn read_rva(&mut self, rva: u32, size: u32) -> std::io::Result<Vec<u8>> {
        let off = self.off_of(rva)?;
        self.at(off, size as usize)
    }
}

fn read_up_to(f: &mut fs::File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

fn utf16(b: &[u8]) -> String {
    let units: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    String::from_utf16_lossy(&units).replace('\u{FFFD}', "")
}

/// (section names, version-info text, string-table text) of a Windows .exe
fn pe_sections_and_version(path: &str) -> std::io::Result<(Vec<String>, String, String)> {
    let Some(mut pe) = Pe::open(path)? else { return Ok((vec![], String::new(), String::new())) };
    let (mut text, mut strings) = (String::new(), String::new());
    if pe.res_base.is_some() {
        // read on demand: installers keep their whole payload in .rsrc
        for (name, off) in pe.entries(0, 512)? {
            if name == 16 && off & 0x8000_0000 != 0 {
                // RT_VERSION: the first language's is enough
                let mut l = Vec::new();
                pe.leaves(off, 0, &mut l)?;
                if let Some(&(rva, size)) = l.first() {
                    text = utf16(&pe.read_rva(rva, size.min(64 << 10))?);
                }
            } else if name == 6 && off & 0x8000_0000 != 0 {
                // RT_STRING: every language, capped
                let mut l = Vec::new();
                pe.leaves(off, 0, &mut l)?;
                let mut got: Vec<String> = Vec::new();
                let mut total = 0;
                for (rva, size) in l {
                    let s = utf16(&pe.read_rva(rva, size.min(64 << 10))?);
                    total += s.chars().count();
                    got.push(s);
                    if total > 256 << 10 {
                        break;
                    }
                }
                strings = got.join("\n");
            }
        }
    }
    Ok((pe.names, text, strings))
}

static INSTALLER_CACHE: LazyLock<Mutex<HashMap<(String, u64, i64), bool>>> = LazyLock::new(Default::default);

/// true when the .exe says it's an installer: WiX Burn's section, version info calling it a setup or
/// installer ("Rockstar Games Launcher Installer"), or an installer builder's marks
pub fn installer_by_content(path: &str) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(st) = fs::metadata(path) else { return false };
    let key = (path.to_string(), st.size(), st.mtime());
    if let Some(v) = INSTALLER_CACHE.lock().unwrap().get(&key) {
        return *v;
    }
    let found = (|| -> std::io::Result<bool> {
        let (names, text, strings) = pe_sections_and_version(path)?;
        if names.iter().any(|n| n.to_lowercase() == ".wixburn") {
            return Ok(true);
        }
        if VERSION_FIELDS.captures_iter(&text).any(|c| INSTALLER_TEXT_RE.is_match(&c[1]))
            || installer_phrase_count(&strings) >= 2 {
            return Ok(true);
        }
        let mut f = fs::File::open(path)?;
        let mut blob = vec![0u8; 2 << 20];
        let n = read_up_to(&mut f, &mut blob)?;
        blob.truncate(n);
        Ok(INSTALLER_MARKS.iter().any(|m| blob.windows(m.len()).any(|w| w == *m)))
    })()
    .unwrap_or(false);
    INSTALLER_CACHE.lock().unwrap().insert(key, found);
    found
}

/// an installer by its name (setup.exe, install_x.exe, x-setup.exe) or, for a real .exe file, by its contents
pub fn is_installer(path: &str) -> bool {
    if INSTALLER_RE.is_match(&basename(path)) {
        return true;
    }
    path.to_lowercase().ends_with(".exe") && path.starts_with('/') && isfile(path) && installer_by_content(path)
}

/// the biggest icon inside a Windows .exe, as PNG or ICO bytes (None if it has none)
pub fn exe_icon(path: &str) -> Option<Vec<u8>> {
    (|| -> std::io::Result<Option<Vec<u8>>> {
        let Some(mut pe) = Pe::open(path)? else { return Ok(None) };
        if pe.res_base.is_none() {
            return Ok(None);
        }
        let types: HashMap<u32, u32> = pe.entries(0, 4096)?.into_iter().collect();
        let (Some(&icons_off), Some(&groups_off)) = (types.get(&3), types.get(&14)) else { return Ok(None) };
        if icons_off & 0x8000_0000 == 0 || groups_off & 0x8000_0000 == 0 {
            return Ok(None);
        }
        let mut icons = HashMap::new();
        for (name, off) in pe.entries(icons_off & 0x7FFF_FFFF, 4096)? {
            if name & 0x8000_0000 == 0 {
                icons.insert(name, off);
            }
        }
        let groups = pe.entries(groups_off & 0x7FFF_FFFF, 4096)?;
        let Some(&(_, first_group)) = groups.first() else { return Ok(None) };
        // walk down subdirectories to the first data entry
        let first_data = |pe: &mut Pe, mut off: u32| -> std::io::Result<Option<Vec<u8>>> {
            for _ in 0..4 {
                if off & 0x8000_0000 != 0 {
                    let sub = pe.entries(off & 0x7FFF_FFFF, 4096)?;
                    match sub.first() {
                        Some(&(_, o)) => off = o,
                        None => return Ok(None),
                    }
                } else {
                    let (rva, size) = pe.data_entry(off)?;
                    if size > 32 * 1024 * 1024 {
                        return Ok(None);
                    }
                    return Ok(Some(pe.read_rva(rva, size)?));
                }
            }
            Ok(None)
        };
        let Some(grp) = first_data(&mut pe, first_group)? else { return Ok(None) };
        if grp.len() < 6 {
            return Ok(None);
        }
        let count = u16_at(&grp, 4).unwrap() as usize;
        let mut best: Option<((u32, u16), Vec<u8>, u32)> = None;
        for i in 0..count {
            let Some(rec) = grp.get(6 + 14 * i..20 + 14 * i) else { break };
            let (w, h) = (rec[0], rec[1]);
            let bits = u16_at(rec, 6).unwrap();
            let iid = u16_at(rec, 12).unwrap() as u32;
            let score = ((if w == 0 { 256 } else { w as u32 }) * (if h == 0 { 256 } else { h as u32 }), bits);
            if icons.contains_key(&iid) && best.as_ref().is_none_or(|b| score > b.0) {
                best = Some((score, rec.to_vec(), iid));
            }
        }
        let Some((_, rec, iid)) = best else { return Ok(None) };
        let Some(data) = first_data(&mut pe, icons[&iid])? else { return Ok(None) };
        if data.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Ok(Some(data));
        }
        // an ICO file around the bitmap: its header, then the group record with the image's offset
        let mut ico = vec![0, 0, 1, 0, 1, 0];
        ico.extend_from_slice(&rec[..4]);
        ico.extend_from_slice(&rec[4..8]);
        ico.extend_from_slice(&(data.len() as u32).to_le_bytes());
        ico.extend_from_slice(&22u32.to_le_bytes());
        ico.extend_from_slice(&data);
        Ok(Some(ico))
    })()
    .ok()
    .flatten()
}

// ---- archives and folders ------------------------------------------------------------------------

pub fn bsdtar() -> Option<String> {
    match std::env::var("VESSEL_BSDTAR") {
        Ok(p) if isfile(&p) => Some(p),
        _ => runners::which("bsdtar"),
    }
}

fn empty_dir(dest: &str) {
    for item in util::listdir(dest) {
        let path = join(dest, &item);
        if isdir(&path) && !util::islink(&path) {
            let _ = fs::remove_dir_all(&path);
        } else {
            let _ = fs::remove_file(&path);
        }
    }
}

/// unpacks an archive into dest, with whichever tool can: unrar/7z/bsdtar/unzip/tar
pub fn extract(archive: &str, dest: &str, report: Report) -> Result<String, String> {
    let env = runners::host_env();
    let low = archive.to_lowercase();
    let bt = bsdtar();
    let s = |x: &str| x.to_string();
    let seven = vec![s("7z"), s("x"), s("-y"), format!("-o{dest}"), s(archive)];
    let mut tools: Vec<Vec<String>> = Vec::new();
    if low.ends_with(".7z.001") {
        tools.push(seven);
    } else if low.ends_with(".rar") {
        tools.push(vec![s("unrar"), s("x"), s("-o+"), s("-y"), s("-idq"), s(archive), format!("{dest}/")]);
        tools.push(seven);
        if let Some(bt) = &bt {
            tools.push(vec![bt.clone(), s("-xvf"), s(archive), s("-C"), s(dest)]);
        }
    } else {
        if let Some(bt) = &bt {
            tools.push(vec![bt.clone(), s("-xvf"), s(archive), s("-C"), s(dest)]);
        }
        tools.push(seven);
        if low.ends_with(".zip") {
            tools.push(vec![s("unzip"), s("-o"), s("-q"), s(archive), s("-d"), s(dest)]);
        } else {
            tools.push(vec![s("tar"), s("-xf"), s(archive), s("-C"), s(dest)]);
        }
    }
    let mut errors: Vec<String> = Vec::new();
    for cmd in tools {
        let exe = if cmd[0].starts_with('/') { Some(cmd[0].clone()) } else { util::which_in(&cmd[0], &env["PATH"]) };
        let Some(exe) = exe else { continue };
        fs::create_dir_all(dest).map_err(|e| e.to_string())?;
        let mut total = 0u64;
        if basename(&exe) == "bsdtar" {
            if let Ok(out) = Command::new(&exe).args(["-tf", archive]).env_clear().envs(&env).stdin(Stdio::null())
                .stderr(Stdio::null()).output() {
                total = out.stdout.iter().filter(|c| **c == b'\n').count() as u64;
            }
        }
        report(Progress::Count("unpack", 0, total));
        // its output and errors in one stream (bsdtar -v lists the files on stderr)
        let Ok((reader, writer)) = std::io::pipe() else { continue };
        let Ok(w2) = writer.try_clone() else { continue };
        let mut c = Command::new(&exe);
        c.args(&cmd[1..]).env_clear().envs(&env).stdin(Stdio::null()).stdout(w2).stderr(writer);
        let child = c.spawn();
        drop(c);
        let Ok(mut child) = child else { continue };
        let mut tail: Vec<String> = Vec::new();
        let mut done = 0u64;
        for line in BufReader::new(reader).split(b'\n').map_while(Result::ok) {
            done += 1;
            tail.push(String::from_utf8_lossy(&line).trim().to_string());
            if tail.len() > 6 {
                tail.remove(0);
            }
            if total > 0 && done % 20 == 0 {
                report(Progress::Count("unpack", done.min(total), total));
            }
        }
        let status = child.wait().map_err(|e| e.to_string())?;
        if status.success() {
            return Ok(dest.to_string());
        }
        let msg: Vec<&String> = tail.iter().filter(|x| !x.is_empty()).collect();
        let msg = if msg.is_empty() {
            format!("exit {}", status.code().unwrap_or(-1))
        } else {
            msg.iter().map(|x| x.as_str()).collect::<Vec<_>>().join(" / ")
        };
        errors.push(format!("{}: {msg}", basename(&exe)));
        // start the next tool on an empty folder
        empty_dir(dest);
    }
    if errors.is_empty() {
        let need = if low.ends_with(".7z.001") { "7z" } else { "bsdtar, unrar or 7z" };
        return Err(format!("nothing here can unpack it; install {need}"));
    }
    Err(errors.join("; "))
}

/// an archive with one folder inside: its contents move up into `folder`
pub fn flatten(folder: &str) -> std::io::Result<()> {
    let items: Vec<String> = util::listdir(folder).into_iter().filter(|i| i != "__MACOSX").collect();
    if items.len() != 1 || !isdir(&join(folder, &items[0])) {
        return Ok(());
    }
    let tmp = join(folder, &format!(".vessel-{}", util::random_hex(6)));
    fs::rename(join(folder, &items[0]), &tmp)?;
    for item in util::listdir(&tmp) {
        move_path(&join(&tmp, &item), &join(folder, &item))?;
    }
    fs::remove_dir(&tmp)?;
    let _ = fs::remove_dir_all(join(folder, "__MACOSX"));
    Ok(())
}

pub fn is_native_program(path: &str) -> bool {
    let low = path.to_lowercase();
    if low.ends_with(".x86_64") || low.ends_with(".x86") || low.ends_with(".appimage") {
        return true;
    }
    if low.ends_with(".sh") {
        return ["start.sh", "run.sh", "launch.sh", "play.sh"].contains(&basename(&low).as_str()) || util::is_executable(path);
    }
    if basename(path).contains('.') || !util::is_executable(path) {
        return false;
    }
    let mut magic = [0u8; 4];
    fs::File::open(path).and_then(|mut f| f.read_exact(&mut magic)).is_ok() && &magic == b"\x7fELF"
}

const SKIP_DIRS: &[&str] = &["windows", "__macosx", "_commonredist", "redist", "directx", "dotnetfx", "vcredist",
    "support", "temp", "appdata", "internet explorer", "windows nt", "windows media player", "common files", "msbuild"];

/// likely main programs in a folder, best first
pub fn find_programs(folder: &str, newer_than: f64, max_depth: usize) -> Vec<String> {
    use std::os::unix::fs::MetadataExt;
    let want = util::NOT_LOWER_ALNUM.replace_all(&basename(folder.trim_end_matches('/')).to_lowercase(), "").into_owned();
    let root_depth = folder.trim_end_matches('/').matches('/').count();
    let mut found: Vec<(f64, String)> = Vec::new();
    util::walk(folder, |dirpath, dirs, files| {
        let depth = dirpath.matches('/').count().saturating_sub(root_depth);
        if depth >= max_depth {
            dirs.clear();
        }
        dirs.retain(|d| !SKIP_DIRS.contains(&d.to_lowercase().as_str()));
        for fname in files {
            let path = join(dirpath, fname);
            let win = fname.to_lowercase().ends_with(".exe");
            if !(win || is_native_program(&path)) || SKIP_PROGRAMS.is_match(fname) {
                continue;
            }
            let Ok(st) = fs::metadata(&path) else { continue };
            if newer_than > 0.0 && (st.ctime() as f64) < newer_than {
                continue;
            }
            let name = util::NOT_LOWER_ALNUM.replace_all(&util::stem(fname).to_lowercase(), "").into_owned();
            let mut score = -(depth as f64) * 12.0 + (st.size().min(300 << 20) as f64) / ((6u64 << 20) as f64);
            if !want.is_empty() && !name.is_empty() && (want.contains(&name) || name.contains(&want)) {
                score += 40.0;
            }
            if is_installer(fname) {
                score -= 60.0;
            }
            if name.contains("launcher") {
                score -= 5.0;
            }
            found.push((score, path));
        }
        true
    });
    found.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    found.into_iter().map(|(_, p)| p).collect()
}

/// shutil.copy2: the file, its permissions and times
fn copy_file(src: &str, dst: &str) -> std::io::Result<()> {
    fs::copy(src, dst)?;
    if let Ok(m) = fs::metadata(src) {
        let times = fs::FileTimes::new().set_modified(m.modified()?).set_accessed(m.accessed()?);
        if let Ok(f) = fs::OpenOptions::new().write(true).open(dst) {
            let _ = f.set_times(times);
        }
    }
    Ok(())
}

/// shutil.copytree(symlinks=True, dirs_exist_ok=True)
pub fn copy_tree(src: &str, dst: &str) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for ent in fs::read_dir(src)? {
        let ent = ent?;
        let name = ent.file_name().to_string_lossy().into_owned();
        let (s, d) = (join(src, &name), join(dst, &name));
        let ft = ent.file_type()?;
        if ft.is_symlink() {
            let target = fs::read_link(&s)?;
            if util::lexists(&d) {
                let _ = fs::remove_file(&d);
            }
            std::os::unix::fs::symlink(target, &d)?;
        } else if ft.is_dir() {
            copy_tree(&s, &d)?;
        } else {
            copy_file(&s, &d)?;
        }
    }
    Ok(())
}

/// shutil.move: a rename, or a copy and delete across drives
pub fn move_path(src: &str, dst: &str) -> std::io::Result<()> {
    let dst = if isdir(dst) { join(dst, &basename(src.trim_end_matches('/'))) } else { dst.to_string() };
    if fs::rename(src, &dst).is_ok() {
        return Ok(());
    }
    if isdir(src) && !util::islink(src) {
        copy_tree(src, &dst)?;
        fs::remove_dir_all(src)
    } else if util::islink(src) {
        std::os::unix::fs::symlink(fs::read_link(src)?, &dst)?;
        fs::remove_file(src)
    } else {
        copy_file(src, &dst)?;
        fs::remove_file(src)
    }
}

/// copies (or moves) files and folders into dest, unpacking archives there if `unpack`
pub fn copy_into(paths: &[String], dest: &str, mv: bool, unpack: bool, report: Report) -> Result<String, String> {
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    for (i, src) in paths.iter().enumerate() {
        report(Progress::Count("copy", i as u64, paths.len() as u64));
        if unpack && isfile(src) && is_archive(src) {
            extract(src, dest, report)?;
            continue;
        }
        let target = join(dest, &basename(src.trim_end_matches('/')));
        let r = if mv {
            if isdir(&target) && isdir(src) {
                copy_tree(src, &target).and_then(|_| fs::remove_dir_all(src))
            } else {
                move_path(src, &target)
            }
        } else if isdir(src) {
            copy_tree(src, &target)
        } else {
            copy_file(src, &target)
        };
        r.map_err(|e| format!("{}: {e}", basename(src)))?;
    }
    Ok(dest.to_string())
}

pub fn game_folder(e: &Entry) -> String {
    if !e.cwd.is_empty() && isdir(&util::expanduser(&e.cwd)) {
        return util::expanduser(&e.cwd);
    }
    let exe = util::expanduser(&e.exe);
    if isfile(&exe) { dirname(&exe) } else { String::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archives() {
        assert!(is_archive("/a/x.zip"));
        assert!(is_archive("/a/x.part1.rar"));
        assert!(!is_archive("/a/x.part2.rar"));
        assert!(is_archive("/a/x.7z.001"));
        assert!(!is_archive("/a/x.7z.002"));
        assert!(is_archive("/a/x.tar.zst"));
        assert!(INSTALLER_RE.is_match("setup.exe"));
        assert!(INSTALLER_RE.is_match("Game-Setup.exe"));
        assert!(!INSTALLER_RE.is_match("game.exe"));
    }
}
