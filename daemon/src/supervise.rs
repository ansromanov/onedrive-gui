//! Runs and stops onedrive processes for each account and keeps the shared state.

use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use zbus::fdo;
use zbus::object_server::SignalEmitter;

use crate::PATH;
use crate::config::signed_in;
use crate::parse::{Event, parse, parse_quota};
use crate::state::{Mode, State, now};

/// Seconds to wait before restarting onedrive after it crashed, by attempt.
const RETRY_DELAYS: [u64; 5] = [10, 30, 60, 120, 300];

pub(crate) fn onedrive() -> Command {
    Command::new(std::env::var_os("ONEDRIVE_BIN").unwrap_or_else(|| "onedrive".into()))
}

pub(crate) fn terminate(pid: u32) {
    // SIGTERM only: onedrive finishes its current step and closes its database cleanly.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
}

#[derive(Clone)]
pub(crate) struct Daemon {
    pub(crate) state: Arc<Mutex<State>>,
    pub(crate) conn: zbus::Connection,
}

impl Daemon {
    pub(crate) fn update<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        let r = f(&mut self.state.lock().unwrap());
        let conn = self.conn.clone();
        tokio::spawn(async move {
            if let Ok(e) = SignalEmitter::new(&conn, PATH) {
                let _ = Daemon::changed(&e).await;
            }
        });
        r
    }

    pub(crate) fn start(&self, name: &str) {
        {
            let mut s = self.state.lock().unwrap();
            let a = s.accounts.get_mut(name).expect("known account");
            if a.running || a.needs_auth {
                return;
            }
            a.running = true;
            a.stopping = false;
        }
        tokio::spawn(self.clone().run(name.to_string(), Mode::Monitor));
    }

    pub(crate) async fn run(self, name: String, mode: Mode) {
        let (dir, resync, auth_dir) = {
            let mut s = self.state.lock().unwrap();
            let a = s.accounts.get_mut(&name).expect("known account");
            let resync = mode == Mode::Monitor && std::mem::take(&mut a.resync_next);
            (a.dir.clone(), resync, a.auth_dir.clone())
        };
        // a resync run must come first: onedrive refuses anything else until then
        if mode == Mode::Monitor && !resync && !self.query_quota(&name, &dir).await {
            return; // paused meanwhile
        }
        let mut cmd = onedrive();
        match (mode, &auth_dir) {
            (Mode::SignIn, Some(auth)) => {
                let files = format!(
                    "{}:{}",
                    auth.join("auth-url").display(),
                    auth.join("response-url").display()
                );
                cmd.arg("--confdir")
                    .arg(&dir)
                    .args(["--reauth", "--auth-files", &files]);
            }
            _ => {
                cmd.arg("--monitor").arg("--confdir").arg(&dir);
                if resync {
                    cmd.args(["--resync", "--resync-auth"]);
                }
            }
        }
        let spawned = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match spawned {
            Ok(c) => c,
            Err(e) => {
                self.update(|s| {
                    let a = s.accounts.get_mut(&name).expect("known account");
                    a.running = false;
                    a.resync_next |= resync;
                    a.status = "stopped";
                    a.error = format!("Cannot start onedrive: {e}");
                });
                if mode == Mode::Monitor {
                    self.schedule_retry(&name);
                }
                return;
            }
        };
        self.update(|s| {
            let a = s.accounts.get_mut(&name).expect("known account");
            a.pid = child.id();
            a.error.clear();
            if mode == Mode::SignIn {
                a.status = "signingin";
            } else if !a.paused {
                a.status = "starting";
            } else if let Some(pid) = a.pid {
                // paused while we were spawning
                a.stopping = true;
                terminate(pid);
            }
        });

        let mut out = BufReader::new(child.stdout.take().expect("piped")).split(b'\n');
        let mut err = BufReader::new(child.stderr.take().expect("piped")).split(b'\n');
        loop {
            let bytes = tokio::select! {
                Ok(Some(l)) = out.next_segment() => l,
                Ok(Some(l)) = err.next_segment() => l,
                else => break,
            };
            match parse(&String::from_utf8_lossy(&bytes)) {
                // onedrive prints this only after it has fully written the URL file
                Some(Event::AuthWaiting) => {
                    if let Some(url) = auth_dir
                        .as_ref()
                        .and_then(|d| std::fs::read_to_string(d.join("auth-url")).ok())
                    {
                        self.update(|s| {
                            s.accounts.get_mut(&name).expect("known account").auth_url =
                                url.trim().to_string()
                        });
                    }
                }
                Some(event) => self.update(|s| s.apply(&name, event)),
                None => {}
            }
        }
        let exit = child.wait().await;

        let (restart, retry) = self.update(|s| {
            let a = s.accounts.get_mut(&name).expect("known account");
            a.pid = None;
            a.running = false;
            a.reset_progress();
            if mode == Mode::SignIn {
                a.auth_url.clear();
                a.paused = false;
                a.stopping = false;
                if let Some(d) = a.auth_dir.take() {
                    let _ = std::fs::remove_dir_all(d);
                }
                a.needs_auth = !signed_in(&a.dir);
                if a.needs_auth {
                    a.status = "signedout";
                    if a.error.is_empty() {
                        a.error = "Sign-in didn't complete".into();
                    }
                } else {
                    a.error.clear();
                }
                return (!a.needs_auth, false);
            }
            let restart = !a.paused && a.stopping; // resumed while still stopping
            let mut retry = false;
            if a.paused {
                a.status = "paused";
            } else if a.needs_auth {
                a.status = "signedout";
            } else if !restart {
                a.status = "stopped";
                if a.error.is_empty() {
                    a.error = match &exit {
                        Ok(st) => format!("onedrive stopped unexpectedly ({st})"),
                        Err(e) => format!("onedrive stopped unexpectedly ({e})"),
                    };
                }
                // crashed: try again later; a clean exit on its own is left for the user to Retry
                retry = !exit.as_ref().is_ok_and(|st| st.success());
                // an interrupted resync has to be redone, plain starts would be refused
                a.resync_next |= resync;
            }
            a.stopping = false;
            (restart, retry)
        });
        if restart {
            self.start(&name);
        } else if retry {
            self.schedule_retry(&name);
        }
    }

    /// Restart onedrive after a growing delay, unless the user pauses, resumes or retries first.
    fn schedule_retry(&self, name: &str) {
        let (generation, delay) = self.update(|s| {
            let a = s.accounts.get_mut(name).expect("known account");
            let delay = RETRY_DELAYS[a.restarts.min(RETRY_DELAYS.len() - 1)];
            a.restarts += 1;
            a.retry_at = now() + delay;
            (a.pause_gen, delay)
        });
        let (d, name) = (self.clone(), name.to_string());
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(delay)).await;
            let due = {
                let mut s = d.state.lock().unwrap();
                let a = s.accounts.get_mut(&name).expect("known account");
                let due = a.pause_gen == generation && !a.running && !a.paused && !a.needs_auth;
                if due {
                    a.retry_at = 0;
                }
                due
            };
            if due {
                d.start(&name);
            }
        });
    }

    /// Read used/total storage with `onedrive --display-quota` (it can't run beside --monitor, which
    /// locks the database). Its pid is tracked so Pause stops it. Returns false if paused meanwhile.
    async fn query_quota(&self, name: &str, dir: &Path) -> bool {
        // shortcut: quota refreshes only when onedrive (re)starts; poll if it goes stale over long sessions
        let spawned = onedrive()
            .arg("--confdir")
            .arg(dir)
            .arg("--display-quota")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        if let Ok(mut child) = spawned {
            self.state
                .lock()
                .unwrap()
                .accounts
                .get_mut(name)
                .expect("known account")
                .pid = child.id();
            let mut lines = BufReader::new(child.stdout.take().expect("piped")).lines();
            let (mut used, mut total) = (0, 0);
            let read = async {
                while let Ok(Some(l)) = lines.next_line().await {
                    match parse_quota(l.trim()) {
                        Some(("Used", b)) => used = b,
                        Some(("Total", b)) => total = b,
                        _ => {}
                    }
                }
            };
            if tokio::time::timeout(Duration::from_secs(60), read)
                .await
                .is_err()
                && let Some(pid) = child.id()
            {
                terminate(pid);
            }
            let _ = child.wait().await;
            if total > 0 {
                self.update(|s| {
                    let a = s.accounts.get_mut(name).expect("known account");
                    (a.quota_used, a.quota_total) = (used, total);
                });
            }
        }
        let mut s = self.state.lock().unwrap();
        let a = s.accounts.get_mut(name).expect("known account");
        a.pid = None;
        a.stopping = false;
        if a.paused {
            a.running = false;
            a.status = "paused";
            return false;
        }
        true
    }

    pub(crate) fn resume_account(&self, name: &str) {
        self.update(|s| {
            let a = s.accounts.get_mut(name).expect("known account");
            a.paused = false;
            a.paused_until = 0;
            a.pause_gen += 1;
            a.retry_at = 0;
            if a.needs_auth {
                a.status = "signedout";
            }
        });
        self.start(name);
    }

    /// Pause and wait until onedrive has exited.
    pub(crate) async fn stop_and_wait(&self, name: &str) {
        self.update(|s| {
            let a = s.accounts.get_mut(name).expect("known account");
            a.paused = true;
            a.pause_gen += 1;
            a.paused_until = 0;
            a.status = "paused";
            if let Some(pid) = a.pid
                && !a.stopping
            {
                a.stopping = true;
                terminate(pid);
            }
        });
        while self.state.lock().unwrap().accounts[name].running {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub(crate) fn check(&self, account: &str) -> fdo::Result<()> {
        if self.state.lock().unwrap().accounts.contains_key(account) {
            Ok(())
        } else {
            Err(fdo::Error::InvalidArgs(format!(
                "unknown account: {account}"
            )))
        }
    }
}
