//! One Vessel at a time: starting it again (menu, dock, file manager) brings back the running one, which may
//! be hidden in the tray, and hands it the files. The same socket and message as the Python Vessel's.

use crate::ui::app::AppEvent;
use crate::util;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub fn socket_path() -> String {
    let dir = std::env::var("XDG_RUNTIME_DIR").ok().filter(|v| !v.is_empty())
        .unwrap_or_else(|| format!("/tmp/vessel-{}", unsafe { libc::getuid() }));
    util::join(&dir, "vessel.sock")
}

/// true when another Vessel is running: it was told to show itself (and to take `paths`)
pub fn hand_over(paths: &[String]) -> bool {
    let Ok(mut c) = UnixStream::connect(socket_path()) else { return false };
    let _ = c.set_write_timeout(Some(Duration::from_secs(1)));
    let msg = serde_json::json!({"show": true, "paths": paths});
    c.write_all(format!("{msg}\n").as_bytes()).is_ok()
}

/// listens for other starts; their messages arrive as AppEvent::Show
pub fn listen(tx: Sender<AppEvent>, wake: Arc<Mutex<Option<egui::Context>>>) {
    let path = socket_path();
    let _ = std::fs::create_dir_all(util::dirname(&path));
    // nobody answered on it (hand_over failed), so it's left from a crash
    let _ = std::fs::remove_file(&path);
    let Ok(listener) = UnixListener::bind(&path) else { return };
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut conn) = conn else { continue };
            let _ = conn.set_read_timeout(Some(Duration::from_secs(1)));
            let mut data = Vec::new();
            let mut buf = [0u8; 65536];
            while !data.ends_with(b"\n") && data.len() < 1 << 20 {
                match conn.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => data.extend_from_slice(&buf[..n]),
                }
            }
            let msg: serde_json::Value = serde_json::from_slice(&data).unwrap_or_default();
            let paths: Vec<String> = msg["paths"].as_array().map(|a| {
                a.iter().filter_map(|p| p.as_str()).filter(|p| util::exists(p)).map(String::from).collect()
            }).unwrap_or_default();
            if tx.send(AppEvent::Show(paths)).is_err() {
                return;
            }
            if let Some(c) = wake.lock().unwrap().as_ref() {
                c.request_repaint();
            }
        }
    });
}

static CLOSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// stops being the running Vessel (once: after a restart the socket is the new Vessel's)
pub fn close() {
    if !CLOSED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        let _ = std::fs::remove_file(socket_path());
    }
}
