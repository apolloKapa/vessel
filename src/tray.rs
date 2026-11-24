//! The tray icon (StatusNotifierItem): click to show or hide Vessel, right-click to quit.

use crate::ui::app::AppEvent;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};

pub type Handle = ksni::blocking::Handle<VesselTray>;

/// where tray clicks go, and the window to wake (set up by main)
pub static TARGET: OnceLock<(Sender<AppEvent>, Arc<Mutex<Option<egui::Context>>>)> = OnceLock::new();

pub struct VesselTray;

fn send(ev: AppEvent) {
    if let Some((tx, wake)) = TARGET.get() {
        let _ = tx.send(ev);
        if let Some(c) = wake.lock().unwrap().as_ref() {
            c.request_repaint();
        }
    }
}

fn icon(size: u32) -> ksni::Icon {
    let img = crate::imaging::app_icon(size);
    let mut data = img.into_raw();
    for px in data.chunks_exact_mut(4) {
        px.rotate_right(1); // RGBA to ARGB
    }
    ksni::Icon { width: size as i32, height: size as i32, data }
}

impl ksni::Tray for VesselTray {
    fn id(&self) -> String {
        "vessel".into()
    }
    fn title(&self) -> String {
        "Vessel".into()
    }
    fn icon_name(&self) -> String {
        "vessel".into()
    }
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![icon(22), icon(32), icon(64)]
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip { title: "Vessel".into(), ..Default::default() }
    }
    fn activate(&mut self, _x: i32, _y: i32) {
        send(AppEvent::TrayActivate);
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::StandardItem;
        vec![
            StandardItem { label: "Open Vessel".into(), activate: Box::new(|_| send(AppEvent::TrayOpen)), ..Default::default() }.into(),
            ksni::MenuItem::Separator,
            StandardItem { label: "Quit Vessel".into(), activate: Box::new(|_| send(AppEvent::TrayQuit)), ..Default::default() }.into(),
        ]
    }
}

/// the tray icon, if the desktop has a tray
pub fn spawn() -> Option<Handle> {
    use ksni::blocking::TrayMethods;
    match VesselTray.spawn() {
        Ok(h) => Some(h),
        Err(e) => {
            eprintln!("vessel: no system tray ({e})");
            None
        }
    }
}
