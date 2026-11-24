//! Searching the Steam store (and SteamGridDB) for covers, and picking one.

use super::pics::Still;
use super::{Btn, Theme, field};
use crate::imaging::{self, Overlay};
use crate::net::{self, CoverCandidate};
use egui::{Id, Key, Rect, Sense, Stroke, StrokeKind, vec2};
use std::sync::mpsc::{Receiver, channel};

enum Msg {
    Found(CoverCandidate, String, Vec<u8>),
    Done(Result<usize, String>),
    Full(Vec<u8>),
}

pub struct CoverSearch {
    term: String,
    note: String,
    results: Vec<(CoverCandidate, String, Vec<u8>, Option<Still>)>,
    rx: Option<Receiver<Msg>>,
    fetching: bool,
    /// the picked cover's bytes, once it's chosen
    pub picked: Option<Vec<u8>>,
    focus: bool,
}

impl CoverSearch {
    pub fn new(ctx: &egui::Context, term: &str) -> Self {
        let key = crate::store::settings().sgdb_key;
        let mut c = CoverSearch {
            term: term.to_string(),
            note: if key.is_empty() {
                "Covers from the Steam store. Add a SteamGridDB key in Settings for games that aren't on Steam.".into()
            } else {
                "Covers from the Steam store and SteamGridDB".into()
            },
            results: vec![],
            rx: None,
            fetching: false,
            picked: None,
            focus: true,
        };
        if !term.trim().is_empty() {
            c.search(ctx);
        }
        c
    }

    fn search(&mut self, ctx: &egui::Context) {
        let term = self.term.trim().to_string();
        if term.is_empty() {
            return;
        }
        self.results.clear();
        self.note = "Searching…".into();
        let key = crate::store::settings().sgdb_key;
        let (tx, rx) = channel();
        self.rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let r = net::cover_candidates(&term, &key).map(|cands| {
                let mut found = 0;
                for c in cands {
                    for url in [c.thumb.clone(), c.fallback.clone()] {
                        if url.is_empty() {
                            continue;
                        }
                        let Ok(data) = net::http_get(&url, &[], 15) else { continue };
                        if tx.send(Msg::Found(c.clone(), url, data)).is_err() {
                            return found;
                        }
                        ctx.request_repaint();
                        found += 1;
                        break;
                    }
                }
                found
            });
            let _ = tx.send(Msg::Done(r));
            ctx.request_repaint();
        });
    }

    fn pick(&mut self, ctx: &egui::Context, i: usize) {
        let (c, got, data, _) = &self.results[i];
        if *got == c.url {
            self.picked = Some(data.clone());
            return;
        }
        // the search shows the small picture: get the big one (or keep the small one if that fails)
        let (url, data) = (c.url.clone(), data.clone());
        let (tx, rx) = channel();
        self.rx = Some(rx);
        self.fetching = true;
        self.note = "Getting the full-size cover…".into();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let full = net::http_get(&url, &[], 30).ok().filter(|d| imaging::decode(d).is_some()).unwrap_or(data);
            let _ = tx.send(Msg::Full(full));
            ctx.request_repaint();
        });
    }

    /// draws it: Some(true) once a cover is picked, Some(false) when closed
    pub fn show(&mut self, ctx: &egui::Context, t: &Theme) -> Option<bool> {
        if let Some(rx) = &self.rx {
            while let Ok(m) = rx.try_recv() {
                match m {
                    Msg::Found(c, url, data) => {
                        let still = imaging::decode(&data).map(|img| {
                            let img = imaging::cover(&img, 272, 412);
                            Still::new(ctx, "cover-result", &img, Overlay::None)
                        });
                        if still.is_some() {
                            self.results.push((c, url, data, still));
                        }
                    }
                    Msg::Done(Err(e)) => self.note = format!("Search failed: {e}"),
                    Msg::Done(Ok(0)) => {
                        self.note = "Nothing found. Try a shorter or different name, or choose an image file.".into()
                    }
                    Msg::Done(Ok(_)) => self.note = "Click a cover to use it.".into(),
                    Msg::Full(data) => {
                        self.picked = Some(data);
                        return Some(true);
                    }
                }
            }
        }
        let mut out = None;
        let size = ctx.content_rect().size();
        let w = 820f32.min(size.x - 40.0);
        let h = 600f32.min(size.y - 40.0);
        let mut search = false;
        let mut clicked = None;
        egui::Modal::new(Id::unique("cover-search"))
            .frame(super::panel_frame(t).inner_margin(18))
            .show(ctx, |ui| {
                ui.set_width(w - 36.0);
                ui.set_height(h - 36.0);
                super::h2(ui, t, "Find a cover");
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if Btn::new("Close").show(ui, t).clicked() {
                            out = Some(false);
                        }
                        if Btn::new("Search").primary().show(ui, t).clicked() {
                            search = true;
                        }
                        let r = ui.add(field(&mut self.term, "Game name").desired_width(ui.available_width()));
                        if self.focus {
                            r.request_focus();
                            self.focus = false;
                        }
                        if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                            search = true;
                        }
                    });
                });
                super::muted_wrap(ui, t, &self.note);
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    let cols = (((ui.available_width() - 10.0) / 156.0) as usize).max(1);
                    for (row, chunk) in self.results.chunks(cols).enumerate() {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 12.0;
                            for (col, (c, _, _, still)) in chunk.iter().enumerate() {
                                let (rect, resp) = ui.allocate_exact_size(vec2(144.0, 214.0), Sense::click());
                                let inner = Rect::from_center_size(rect.center(), vec2(136.0, 206.0));
                                if let Some(s) = still {
                                    s.paint(ui, inner, 10.0, "");
                                }
                                if resp.hovered() {
                                    ui.painter().rect_stroke(rect, 12.0, Stroke::new(2.0, t.accent), StrokeKind::Inside);
                                }
                                let resp = resp.on_hover_text(&c.label).on_hover_cursor(egui::CursorIcon::PointingHand);
                                if resp.clicked() && !self.fetching {
                                    clicked = Some(row * cols + col);
                                }
                            }
                        });
                        ui.add_space(4.0);
                    }
                });
            });
        if search {
            self.search(ctx);
        }
        if let Some(i) = clicked {
            self.pick(ctx, i);
            if self.picked.is_some() {
                return Some(true);
            }
        }
        if out.is_none() && ctx.input(|i| i.key_pressed(Key::Escape)) {
            out = Some(false);
        }
        out
    }
}
