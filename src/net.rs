//! Downloads: covers, Proton builds, winetricks, Vessel's own updates.

use crate::files::{Progress, Report};
use crate::runners::{self, arch_of, machine, proton_label};
use crate::store::{self, VERSION};
use crate::util::{self, join};
use serde_json::Value;
use sha2::Digest;
use std::fs;
use std::io::{Read, Write};
use std::time::Duration;

pub const GE_RELEASES: &str = "https://api.github.com/repos/GloriousEggroll/proton-ge-custom/releases/latest";
pub const VESSEL_RELEASES: &str = "https://api.github.com/repos/apolloKapa/vessel/releases/latest";
pub const VESSEL_PAGE: &str = "https://github.com/apolloKapa/vessel/releases/latest";
pub const DW_RELEASES: &str = "https://dawn.wine/api/v1/repos/dawn-winery/dwproton/releases/latest";
pub const WINETRICKS_URL: &str = "https://raw.githubusercontent.com/Winetricks/winetricks/master/src/winetricks";

/// a Proton Vessel can download: (key, label, latest-release API, archive suffix)
pub struct ProtonSource {
    pub key: &'static str,
    pub label: &'static str,
    pub api: &'static str,
    pub suffix: &'static str,
}

pub const PROTON_SOURCES: &[ProtonSource] = &[
    ProtonSource { key: "ge", label: "GE-Proton", api: GE_RELEASES, suffix: ".tar.gz" },
    ProtonSource { key: "dw", label: "DWProton", api: DW_RELEASES, suffix: ".tar.xz" },
];

fn agent(timeout: u64) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(timeout)))
        .user_agent(format!("Vessel/{VERSION}"))
        .build()
        .into()
}

fn call(url: &str, headers: &[(&str, &str)], timeout: u64) -> Result<ureq::http::Response<ureq::Body>, String> {
    let mut req = agent(timeout).get(url);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    req.call().map_err(|e| match e {
        ureq::Error::StatusCode(code) => format!("HTTP Error {code}"),
        other => other.to_string(),
    })
}

pub fn http_get(url: &str, headers: &[(&str, &str)], timeout: u64) -> Result<Vec<u8>, String> {
    let mut r = call(url, headers, timeout)?;
    r.body_mut().with_config().limit(64 << 20).read_to_vec().map_err(|e| e.to_string())
}

pub fn http_json(url: &str, headers: &[(&str, &str)]) -> Result<Value, String> {
    let data = http_get(url, headers, 20)?;
    serde_json::from_slice(&data).map_err(|e| e.to_string())
}

/// urllib.parse.quote: everything but letters, digits, _.-~ and / percent-encoded
pub fn quote(s: &str, plus: bool) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => out.push(b as char),
            b'/' if !plus => out.push('/'),
            b' ' if plus => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[derive(Clone, Debug)]
pub struct CoverCandidate {
    pub label: String,
    pub url: String,
    pub thumb: String,
    pub fallback: String,
}

/// possible covers for a name, from SteamGridDB (with a key) and the Steam store
pub fn cover_candidates(term: &str, key: &str) -> Result<Vec<CoverCandidate>, String> {
    let mut out = Vec::new();
    if !key.is_empty() {
        let auth = format!("Bearer {key}");
        let h = [("Authorization", auth.as_str())];
        let sgdb = (|| -> Result<(), String> {
            let games = http_json(&format!("https://www.steamgriddb.com/api/v2/search/autocomplete/{}", quote(term, false)), &h)?;
            for g in games["data"].as_array().cloned().unwrap_or_default().iter().take(3) {
                let grids = http_json(&format!("https://www.steamgriddb.com/api/v2/grids/game/{}?dimensions=600x900,342x482,660x930&types=static",
                                               g["id"]), &h)?;
                for gr in grids["data"].as_array().cloned().unwrap_or_default().iter().take(8) {
                    let url = gr["url"].as_str().unwrap_or("").to_string();
                    let thumb = gr["thumb"].as_str().filter(|t| !t.is_empty()).map(String::from).unwrap_or_else(|| url.clone());
                    out.push(CoverCandidate { label: g["name"].as_str().unwrap_or("").into(), url, thumb, fallback: String::new() });
                }
            }
            Ok(())
        })();
        if let Err(e) = sgdb {
            eprintln!("vessel: SteamGridDB search failed: {e}");
        }
    }
    let q = format!("term={}&l=english&cc=US", quote(term, true));
    let items = http_json(&format!("https://store.steampowered.com/api/storesearch/?{q}"), &[])?;
    for it in items["items"].as_array().cloned().unwrap_or_default().iter().take(10) {
        let id = &it["id"];
        let url = format!("https://shared.steamstatic.com/store_item_assets/steam/apps/{id}/library_600x900_2x.jpg");
        out.push(CoverCandidate {
            label: it["name"].as_str().unwrap_or("").into(),
            url: url.clone(),
            thumb: url,
            fallback: format!("https://shared.steamstatic.com/store_item_assets/steam/apps/{id}/header.jpg"),
        });
    }
    Ok(out)
}

/// streams a download into `part`, reporting progress; its size and SHA-512
fn download(url: &str, part: &str, size_hint: u64, label: &str, report: Report) -> Result<(u64, String), String> {
    let mut r = call(url, &[], 30)?;
    let total = r.body().content_length().filter(|n| *n > 0).unwrap_or(size_hint);
    let mut body = r.body_mut().as_reader();
    let mut f = fs::File::create(part).map_err(|e| e.to_string())?;
    let mut digest = sha2::Sha512::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut done = 0u64;
    loop {
        let n = body.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        f.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        digest.update(&buf[..n]);
        done += n as u64;
        report(Progress::Download(done, total, label.to_string()));
    }
    f.flush().map_err(|e| e.to_string())?;
    Ok((done, hex::encode(digest.finalize())))
}

/// downloads the newest release of a Proton into compatibilitytools.d; returns its name
pub fn install_proton(source: &str, report: Report) -> Result<String, String> {
    let src = PROTON_SOURCES.iter().find(|p| p.key == source).ok_or("unknown Proton")?;
    let rel = http_json(src.api, &[])?;
    let assets = rel["assets"].as_array().cloned().unwrap_or_default();
    let name = |a: &Value| a["name"].as_str().unwrap_or("").to_string();
    let archives: Vec<&Value> = assets.iter().filter(|a| name(a).ends_with(src.suffix)).collect();
    // releases can carry builds for several CPUs (GE-Proton11-12 lists aarch64 first): take this PC's, or one
    // that names no CPU (older releases were x86_64 only)
    let me = machine();
    let asset = archives.iter().find(|a| arch_of(&name(a)) == me)
        .or_else(|| archives.iter().find(|a| arch_of(&name(a)).is_empty() && me == "x86_64"))
        .ok_or_else(|| format!("the release has no {} for {me}", src.suffix))?;
    let aname = name(asset);
    let folder = aname[..aname.len() - src.suffix.len()].to_string();
    let tag = proton_label(&folder);
    let dest = runners::proton_install_dir();
    if util::isfile(&util::joins(&[&dest, &folder, "proton"])) {
        return Ok(tag);
    }
    let sums = assets.iter().find(|a| name(a) == format!("{folder}.sha512sum"))
        .or_else(|| assets.iter().find(|a| name(a).ends_with(".sha512sum") && arch_of(&name(a)).is_empty()));
    let mut expected = String::new();
    if let Some(s) = sums {
        let data = http_get(s["browser_download_url"].as_str().unwrap_or(""), &[], 20)?;
        expected = String::from_utf8_lossy(&data).split_whitespace().next().unwrap_or("").to_lowercase();
    }
    fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
    let part = join(&dest, &format!(".{aname}.part"));
    let result = (|| {
        let url = asset["browser_download_url"].as_str().unwrap_or("");
        let (_, sha) = download(url, &part, asset["size"].as_u64().unwrap_or(0), &tag, report)?;
        if !expected.is_empty() && sha != expected {
            return Err("the download is damaged (checksum mismatch); try again".to_string());
        }
        report(Progress::Unpack(tag.clone()));
        let out = std::process::Command::new("tar").args(["-xf", &part, "-C", &dest]).env_clear().envs(runners::host_env())
            .stdout(std::process::Stdio::null()).output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(format!("tar: {}", String::from_utf8_lossy(&out.stderr).trim()));
        }
        Ok(tag.clone())
    })();
    let _ = fs::remove_file(&part);
    result
}

fn version_tuple(v: &str) -> Vec<u64> {
    crate::util::NUMBERS.find_iter(v).take(3).filter_map(|m| m.as_str().parse().ok()).collect()
}

#[derive(Clone, Debug, Default)]
pub struct UpdateInfo {
    pub version: String,
    pub newer: bool,
    pub url: String,
    pub size: u64,
    pub page: String,
}

/// the newest Vessel release on GitHub
pub fn check_update() -> Result<UpdateInfo, String> {
    let rel = http_json(VESSEL_RELEASES, &[])?;
    let ver = rel["tag_name"].as_str().unwrap_or("").trim_start_matches('v').to_string();
    let asset = rel["assets"].as_array().and_then(|a| a.iter().find(|x| x["name"].as_str().unwrap_or("").ends_with(".AppImage")).cloned());
    Ok(UpdateInfo {
        newer: version_tuple(&ver) > version_tuple(VERSION),
        url: asset.as_ref().and_then(|a| a["browser_download_url"].as_str()).unwrap_or("").into(),
        size: asset.as_ref().and_then(|a| a["size"].as_u64()).unwrap_or(0),
        page: rel["html_url"].as_str().filter(|p| !p.is_empty()).unwrap_or(VESSEL_PAGE).into(),
        version: ver,
    })
}

/// downloads the release's AppImage over the running one (beside it first, then swapped in); its version
pub fn install_update(info: &UpdateInfo, report: Report) -> Result<String, String> {
    let appimage = crate::appimage().filter(|a| util::isfile(a)).ok_or("Vessel isn't running from its AppImage")?;
    let folder = util::dirname(&appimage);
    let probe = join(&folder, &format!(".vessel-write-{}", util::random_hex(6)));
    if fs::write(&probe, b"").is_err() {
        return Err(format!("can't write to {folder}"));
    }
    let _ = fs::remove_file(&probe);
    let part = join(&folder, &format!(".{}.part", util::basename(&appimage)));
    let result = (|| {
        let (got, _) = download(&info.url, &part, info.size, &info.version, report)?;
        let mut magic = [0u8; 4];
        fs::File::open(&part).and_then(|mut f| f.read_exact(&mut magic)).map_err(|e| e.to_string())?;
        if &magic != b"\x7fELF" {
            return Err("the download isn't an AppImage".to_string());
        }
        if info.size > 0 && got != info.size {
            return Err("the download is incomplete; try again".to_string());
        }
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&part, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
        // a new file, so the running copy keeps working until restart
        fs::rename(&part, &appimage).map_err(|e| e.to_string())?;
        Ok(info.version.clone())
    })();
    let _ = fs::remove_file(&part);
    result
}

pub fn install_winetricks() -> Result<String, String> {
    let data = http_get(WINETRICKS_URL, &[], 60)?;
    if !data.starts_with(b"#!") {
        return Err("unexpected download".into());
    }
    let bin = join(&store::tools_dir(), "bin");
    fs::create_dir_all(&bin).map_err(|e| e.to_string())?;
    let path = join(&bin, "winetricks");
    fs::write(&path, data).map_err(|e| e.to_string())?;
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    Ok(path)
}
