//! Work on another thread, with progress for the window.

use crate::files::Progress;
use std::sync::mpsc::{Receiver, channel};

pub struct Job<T> {
    progress: Receiver<Progress>,
    result: Receiver<Result<T, String>>,
    /// the newest progress it reported
    pub last: Option<Progress>,
}

impl<T: Send + 'static> Job<T> {
    pub fn start(ctx: &egui::Context, f: impl FnOnce(&dyn Fn(Progress)) -> Result<T, String> + Send + 'static) -> Self {
        let (ptx, progress) = channel();
        let (rtx, result) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let c2 = ctx.clone();
            let report = move |p: Progress| {
                let _ = ptx.send(p);
                c2.request_repaint();
            };
            let r = f(&report);
            let _ = rtx.send(r);
            ctx.request_repaint();
        });
        Job { progress, result, last: None }
    }

    /// its result, once it's done
    pub fn poll(&mut self) -> Option<Result<T, String>> {
        while let Ok(p) = self.progress.try_recv() {
            self.last = Some(p);
        }
        match self.result.try_recv() {
            Ok(r) => Some(r),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(_) => Some(Err("it stopped".into())),
        }
    }

    /// how far it is, 0..1 (None: no amount known)
    pub fn fraction(&self) -> Option<f32> {
        match &self.last {
            Some(Progress::Count(_, done, total)) if *total > 0 => Some(*done as f32 / *total as f32),
            Some(Progress::Download(done, total, _)) if *total > 0 => Some(*done as f32 / *total as f32),
            _ => None,
        }
    }
}
