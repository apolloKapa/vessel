//! The pictures of entries as textures: made on worker threads (decoding covers is slow), cached, and
//! drawn with their text on top.

use crate::imaging::{self, Overlay};
use crate::store::{self, Entry};
use crate::util::join;
use egui::{Color32, Rect, TextureHandle, TextureOptions, pos2};
use image::RgbaImage;
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Shape {
    /// portrait, the cover first
    Cover,
    /// square, the icon first
    Icon,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Key {
    pub id: String,
    pub name: String,
    pub cover: String,
    pub icon: String,
    pub w: u32,
    pub h: u32,
    pub shape: Shape,
    pub blur: bool,
}

enum Slot {
    Loading,
    Ready(TextureHandle, Overlay),
}

struct Job {
    key: Key,
}

struct Done {
    key: Key,
    image: RgbaImage,
    overlay: Overlay,
}

pub struct Pictures {
    cache: HashMap<Key, Slot>,
    jobs: Sender<Job>,
    done: Receiver<Done>,
    ctx: Arc<Mutex<Option<egui::Context>>>,
}

/// the picture of an entry at w × h pixels
pub fn make(key: &Key) -> (RgbaImage, Overlay) {
    let cover = if key.cover.is_empty() { None } else { imaging::load(&join(&store::covers_dir(), &key.cover)) };
    let icon = if key.icon.is_empty() { None } else { imaging::load(&join(&store::icons_dir(), &key.icon)) };
    let (img, overlay) = match (key.shape, &icon, &cover) {
        (Shape::Icon, Some(ic), _) => (imaging::fit(ic, key.w, key.h, true), Overlay::None),
        (_, _, Some(c)) => (imaging::cover(c, key.w, key.h), Overlay::None),
        _ => imaging::placeholder(&key.name, key.w, key.h, icon.as_ref()),
    };
    if key.blur { (imaging::blurred(&img), Overlay::Nsfw) } else { (img, overlay) }
}

pub fn to_color_image(img: &RgbaImage) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw())
}

impl Pictures {
    pub fn new() -> Self {
        let (jobs, rx) = channel::<Job>();
        let (tx, done) = channel::<Done>();
        let rx = Arc::new(Mutex::new(rx));
        let ctx: Arc<Mutex<Option<egui::Context>>> = Default::default();
        for _ in 0..3 {
            let rx = rx.clone();
            let tx = tx.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || loop {
                let job = match rx.lock().unwrap().recv() {
                    Ok(j) => j,
                    Err(_) => return,
                };
                let (image, overlay) = make(&job.key);
                if tx.send(Done { key: job.key, image, overlay }).is_err() {
                    return;
                }
                if let Some(c) = ctx.lock().unwrap().as_ref() {
                    c.request_repaint();
                }
            });
        }
        Pictures { cache: HashMap::new(), jobs, done, ctx }
    }

    /// a new window (or a new theme) needs new textures
    pub fn reset(&mut self, ctx: &egui::Context) {
        self.cache.clear();
        *self.ctx.lock().unwrap() = Some(ctx.clone());
        while self.done.try_recv().is_ok() {}
    }

    /// forgets the pictures of one entry (its cover or icon changed)
    pub fn forget(&mut self, id: &str) {
        self.cache.retain(|k, _| k.id != id);
    }

    fn collect(&mut self, ctx: &egui::Context) {
        while let Ok(d) = self.done.try_recv() {
            if let Some(Slot::Loading) = self.cache.get(&d.key) {
                let name = format!("pic-{}-{}x{}", d.key.id, d.key.w, d.key.h);
                let tex = ctx.load_texture(name, to_color_image(&d.image), TextureOptions::LINEAR);
                self.cache.insert(d.key, Slot::Ready(tex, d.overlay));
            }
        }
    }

    /// draws the entry's picture into `rect` with rounded corners (a plain box while it loads)
    pub fn paint(&mut self, ui: &egui::Ui, e: &Entry, rect: Rect, shape: Shape, blur: bool, radius: f32, loading: Color32) {
        let ctx = ui.ctx().clone();
        self.collect(&ctx);
        let ppp = ctx.pixels_per_point();
        let key = Key {
            id: e.id.clone(),
            name: e.name.clone(),
            cover: e.cover.clone(),
            icon: e.icon.clone(),
            w: (rect.width() * ppp).round().max(1.0) as u32,
            h: (rect.height() * ppp).round().max(1.0) as u32,
            shape,
            blur,
        };
        match self.cache.get(&key) {
            Some(Slot::Ready(tex, overlay)) => {
                let overlay = *overlay;
                egui::Image::from_texture((tex.id(), rect.size())).corner_radius(radius).paint_at(ui, rect);
                paint_overlay(ui, rect, overlay, &e.name);
            }
            Some(Slot::Loading) => {
                ui.painter().rect_filled(rect, radius, loading);
            }
            None => {
                ui.painter().rect_filled(rect, radius, loading);
                self.cache.insert(key.clone(), Slot::Loading);
                let _ = self.jobs.send(Job { key });
            }
        }
    }
}

pub fn initials(name: &str) -> String {
    let words: Vec<&str> = crate::util::NON_ALNUM.split(name).filter(|w| !w.is_empty()).take(2).collect();
    let s: String = words.iter().filter_map(|w| w.chars().next()).collect::<String>().to_uppercase();
    if s.is_empty() { "?".into() } else { s }
}

/// the text a picture needs: initials, the name under the icon, or "NSFW"
pub fn paint_overlay(ui: &egui::Ui, rect: Rect, overlay: Overlay, name: &str) {
    let (w, h) = (rect.width(), rect.height());
    let p = ui.painter();
    match overlay {
        Overlay::None => {}
        Overlay::Initials => {
            p.text(rect.center(), egui::Align2::CENTER_CENTER, initials(name), super::bold((w.min(h) * 0.34).max(6.0)),
                   Color32::from_rgba_unmultiplied(255, 255, 255, 225));
        }
        Overlay::NameUnder { icon_bottom } => {
            let font = super::bold((w * 0.085).max(11.0));
            let g = super::elided(ui, name, font, Color32::from_rgba_unmultiplied(255, 255, 255, 235), w - 20.0);
            let x = rect.center().x - g.size().x / 2.0;
            p.galley(pos2(x, rect.min.y + icon_bottom * h + 8.0), g, Color32::WHITE);
        }
        Overlay::Nsfw => {
            p.text(rect.center(), egui::Align2::CENTER_CENTER, "NSFW", super::bold((w.min(h) / 7.0).max(11.0)),
                   Color32::from_rgba_unmultiplied(255, 255, 255, 230));
        }
    }
}

/// a picture that isn't an entry's (a cover being picked, the edit window's preview): made right away
pub struct Still {
    pub tex: TextureHandle,
    pub overlay: Overlay,
}

impl Still {
    pub fn new(ctx: &egui::Context, name: &str, img: &RgbaImage, overlay: Overlay) -> Still {
        Still { tex: ctx.load_texture(name, to_color_image(img), TextureOptions::LINEAR), overlay }
    }

    pub fn paint(&self, ui: &egui::Ui, rect: Rect, radius: f32, name: &str) {
        egui::Image::from_texture((self.tex.id(), rect.size())).corner_radius(radius).paint_at(ui, rect);
        paint_overlay(ui, rect, self.overlay, name);
    }
}

pub fn app_icon_texture(ctx: &egui::Context, size: u32) -> TextureHandle {
    ctx.load_texture("vessel-icon", to_color_image(&imaging::app_icon(size)), TextureOptions::LINEAR)
}
