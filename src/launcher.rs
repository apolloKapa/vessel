//! What Vessel started: playtime, stopping, loading (until the window shows), quick crashes.

use crate::runners::{self, LaunchError};
use crate::store::{self, Entry};
use crate::util;
use std::collections::HashMap;
use std::process::Child;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub enum Event {
    /// its window showed up
    Started(String),
    /// it ended: (id, exit code, seconds it ran)
    Finished(String, i32, f64),
    /// something to redraw
    Changed,
}

struct Running {
    child: Child,
    start: f64,
}

#[derive(Default)]
struct State {
    running: HashMap<String, Running>,
    /// still loading (no window yet): when it was started
    starting: HashMap<String, f64>,
}

#[derive(Clone)]
pub struct Launcher {
    state: Arc<Mutex<State>>,
    events: Sender<Event>,
    wake: Arc<Mutex<Option<Box<dyn Fn() + Send>>>>,
}

impl Launcher {
    /// the launcher and where its events arrive
    pub fn new() -> (Launcher, Receiver<Event>) {
        let (tx, rx) = channel();
        let l = Launcher { state: Default::default(), events: tx, wake: Default::default() };
        let me = l.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(1));
            me.poll();
        });
        (l, rx)
    }

    /// called after each event, to wake the window
    pub fn on_event(&self, f: impl Fn() + Send + 'static) {
        *self.wake.lock().unwrap() = Some(Box::new(f));
    }

    fn send(&self, ev: Event) {
        let _ = self.events.send(ev);
        if let Some(w) = self.wake.lock().unwrap().as_ref() {
            w();
        }
    }

    pub fn is_running(&self, eid: &str) -> bool {
        self.state.lock().unwrap().running.contains_key(eid)
    }

    pub fn is_starting(&self, eid: &str) -> bool {
        self.state.lock().unwrap().starting.contains_key(eid)
    }

    pub fn any_starting(&self) -> bool {
        !self.state.lock().unwrap().starting.is_empty()
    }

    pub fn running_ids(&self) -> Vec<String> {
        self.state.lock().unwrap().running.keys().cloned().collect()
    }

    pub fn launch(&self, e: &Entry) -> Result<(), LaunchError> {
        if self.is_running(&e.id) {
            return Ok(());
        }
        let s = store::settings();
        let child = runners::start_process(e, &s, None, None, false, None)?;
        store::update_entry(&e.id, |x| x.last_played = util::now_secs());
        {
            let mut st = self.state.lock().unwrap();
            st.running.insert(e.id.clone(), Running { child, start: util::now() });
            st.starting.insert(e.id.clone(), util::now());
        }
        self.send(Event::Changed);
        Ok(())
    }

    pub fn stop(&self, e: &Entry) {
        let pid = match self.state.lock().unwrap().running.get(&e.id) {
            Some(r) => r.child.id(),
            None => return,
        };
        runners::kill_group(pid, libc::SIGTERM);
        runners::kill_prefix(e, &store::settings());
        let me = self.clone();
        let eid = e.id.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(5));
            let mut st = me.state.lock().unwrap();
            if let Some(r) = st.running.get_mut(&eid) {
                if r.child.id() == pid && matches!(r.child.try_wait(), Ok(None)) {
                    runners::kill_group(pid, libc::SIGKILL);
                }
            }
        });
    }

    fn poll(&self) {
        let mut changed = false;
        let mut events = Vec::new();
        // loading: done once its window shows (or after 3 minutes, or when the compositor can't tell)
        let starting: Vec<(String, f64, Option<u32>)> = {
            let st = self.state.lock().unwrap();
            st.starting.iter().map(|(k, t)| (k.clone(), *t, st.running.get(k).map(|r| r.child.id()))).collect()
        };
        if !starting.is_empty() {
            let s = store::settings();
            for (eid, t0, pid) in starting {
                let e = store::get_entry(&eid);
                let shown = e.as_ref().and_then(|e| runners::has_window(e, pid, &s));
                if e.is_none() || shown != Some(false) || util::now() - t0 > 180.0 || pid.is_none() {
                    self.state.lock().unwrap().starting.remove(&eid);
                    changed = true;
                    if shown == Some(true) {
                        events.push(Event::Started(eid));
                    }
                }
            }
        }
        let ended: Vec<(String, i32, f64)> = {
            let mut st = self.state.lock().unwrap();
            let mut out = Vec::new();
            for (eid, r) in st.running.iter_mut() {
                if let Ok(Some(status)) = r.child.try_wait() {
                    use std::os::unix::process::ExitStatusExt;
                    let code = status.code().unwrap_or_else(|| -status.signal().unwrap_or(0));
                    out.push((eid.clone(), code, util::now() - r.start));
                }
            }
            for (eid, _, _) in &out {
                st.running.remove(eid);
                st.starting.remove(eid);
            }
            out
        };
        for (eid, code, secs) in ended {
            store::add_playtime(&eid, secs);
            events.push(Event::Finished(eid, code, secs));
            changed = true;
        }
        for ev in events {
            self.send(ev);
        }
        if changed {
            self.send(Event::Changed);
        }
    }

    /// counts the time so far of everything still running (when Vessel closes)
    pub fn save_running(&self) {
        let mut st = self.state.lock().unwrap();
        for (eid, r) in st.running.drain() {
            store::add_playtime(&eid, util::now() - r.start);
        }
        st.starting.clear();
    }
}
