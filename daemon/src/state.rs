//! The daemon's view of each account, and how onedrive events change it.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::parse::Event;

pub(crate) const RECENT_MAX: usize = 100;

pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Mode {
    Monitor,
    SignIn,
}

#[derive(Default)]
pub(crate) struct Account {
    pub(crate) dir: PathBuf,
    pub(crate) sync_dir: String,
    pub(crate) status: &'static str,
    pub(crate) error: String,
    pub(crate) pid: Option<u32>,
    pub(crate) running: bool,
    pub(crate) stopping: bool,
    pub(crate) paused: bool,
    pub(crate) paused_until: u64,
    pub(crate) pause_gen: u64,
    pub(crate) needs_auth: bool,
    pub(crate) auth_dir: Option<PathBuf>,
    pub(crate) auth_url: String,
    pub(crate) resync_next: bool,
    /// crashes since the last completed sync, for the restart backoff
    pub(crate) restarts: usize,
    pub(crate) retry_at: u64,
    pub(crate) planned: u64,
    pub(crate) transferred: u64,
    pub(crate) current: String,
    pub(crate) percent: u32,
    pub(crate) quota_used: u64,
    pub(crate) quota_total: u64,
    pub(crate) excluded: BTreeSet<String>,
}

#[derive(Default)]
pub(crate) struct State {
    pub(crate) accounts: BTreeMap<String, Account>,
    pub(crate) recent: VecDeque<Value>,
}

impl State {
    pub(crate) fn apply(&mut self, name: &str, event: Event) {
        let a = self.accounts.get_mut(name).expect("known account");
        match event {
            Event::Transfer { action, path, ok } => {
                if ok && ["upload", "download", "conflict"].contains(&action) {
                    a.transferred += 1;
                }
                a.current.clear();
                self.recent.push_front(json!({"account": name, "action": action, "path": path, "ok": ok, "time": now()}));
                self.recent.truncate(RECENT_MAX);
                return;
            }
            Event::Planned(n) => {
                a.planned += n;
                return;
            }
            Event::Progress { path, percent } => {
                (a.current, a.percent) = if percent < 100 {
                    (path, percent)
                } else {
                    (String::new(), 0)
                };
                return;
            }
            _ => {}
        }
        // lines printed while onedrive shuts down must not overwrite "paused"
        if a.paused {
            return;
        }
        match event {
            Event::Syncing => a.status = "syncing",
            Event::Synced => {
                a.status = "synced";
                a.error.clear();
                a.restarts = 0;
                a.reset_progress();
            }
            Event::SyncedWithFailures => {
                a.status = "error";
                a.error = "Some items couldn't be synced".into();
                a.reset_progress();
            }
            Event::SignedOut(msg) => {
                a.needs_auth = true;
                a.error = msg;
            }
            Event::Error(msg) => {
                a.status = "error";
                a.error = msg;
            }
            Event::Transfer { .. }
            | Event::Planned(_)
            | Event::Progress { .. }
            | Event::AuthWaiting => {}
        }
    }
}

impl Account {
    pub(crate) fn reset_progress(&mut self) {
        (self.planned, self.transferred, self.current, self.percent) = (0, 0, String::new(), 0);
    }
}

#[cfg(test)]
#[path = "state_test.rs"]
mod tests;
