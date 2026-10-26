//! Supervises one `onedrive --monitor` process per account and exposes status on D-Bus.
//!
//! Safety rules: never touch synced files or the onedrive database; only pass `--monitor --confdir`,
//! plus `--resync --resync-auth` once after the user applies settings (onedrive refuses to start
//! otherwise), and `--reauth --auth-files` to sign in; stop onedrive with SIGTERM only so it can
//! shut down cleanly; edit the config only while onedrive is stopped, atomically, with a backup.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use zbus::fdo;
use zbus::object_server::SignalEmitter;

const NAME: &str = "org.onedrivegui.Daemon";
const PATH: &str = "/org/onedrivegui/Daemon";
const RECENT_MAX: usize = 100;

#[derive(Debug, PartialEq)]
enum Event {
    Syncing,
    Synced,
    SyncedWithFailures,
    Transfer { action: &'static str, path: String, ok: bool },
    SignedOut(String),
    AuthWaiting,
    Error(String),
}

/// Map one line of onedrive console output to an event.
/// Strings come from onedrive v2.5.x src/sync.d, src/onedrive.d; unknown lines are ignored.
fn parse(line: &str) -> Option<Event> {
    const TRANSFERS: [(&str, &str); 9] = [
        ("Uploading new file: ", "upload"),
        ("Uploading modified file: ", "upload"),
        ("Downloading file: ", "download"),
        ("Deleting item from Microsoft OneDrive: ", "delete-remote"),
        ("Deleting online file from Microsoft OneDrive: ", "delete-remote"),
        ("Deleting online folder from Microsoft OneDrive: ", "delete-remote"),
        ("Deleting local file: ", "delete-local"),
        ("Deleting local directory: ", "delete-local"),
        ("Moving this local file to the configured 'Recycle Bin': ", "delete-local"),
    ];
    let line = line.trim_end();
    for (prefix, action) in TRANSFERS {
        // find, not starts_with: progress dots printed without a newline can precede a message
        let Some(i) = line.find(prefix) else { continue };
        let rest = &line[i + prefix.len()..];
        let (path, ok) = if let Some(p) = rest.strip_suffix(" ... done") {
            (p, true)
        } else if let Some(p) = rest.strip_suffix(" ... failed!") {
            (p, false)
        } else if action.starts_with("delete") {
            (rest, true)
        } else {
            return None;
        };
        // onedrive keeps a conflicting local copy as <name>-<host>-safeBackup-NNNN.<ext> and uploads it
        let action = if action == "upload" && path.contains("-safeBackup-") { "conflict" } else { action };
        return Some(Event::Transfer { action, path: path.to_string(), ok });
    }
    if line.contains("Starting a sync with Microsoft OneDrive") || line.contains("Syncing changes from Microsoft OneDrive") {
        Some(Event::Syncing)
    } else if line.contains("Sync with Microsoft OneDrive is complete") {
        Some(Event::Synced)
    } else if line.contains("Sync with Microsoft OneDrive has completed, however there are items that failed to sync") {
        Some(Event::SyncedWithFailures)
    } else if line.contains("Waiting for --auth-files elements to be available") {
        Some(Event::AuthWaiting)
    } else if line.contains("--reauth") {
        Some(Event::SignedOut(line.trim_start_matches("ERROR: ").to_string()))
    } else {
        line.find("ERROR: ").map(|i| Event::Error(line[i + 7..].to_string()))
    }
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

/// Value of a `key = "value"` config line, if this line sets `key`.
fn config_value(line: &str, key: &str) -> Option<String> {
    let v = line.trim().strip_prefix(key)?.trim_start().strip_prefix('=')?;
    Some(v.trim().trim_matches('"').to_string())
}

/// Folder names the GUI can write as `skip_dir = "/<name>"`: one segment, no skip_dir pattern syntax.
fn valid_folder_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', '|', '*', '?', '"'])
}

/// Top-level folder excluded by a GUI-managed line `skip_dir = "/<name>"`. Other skip_dir lines are the user's.
fn managed_exclude(line: &str) -> Option<String> {
    let v = config_value(line, "skip_dir")?;
    let name = v.strip_prefix('/')?;
    valid_folder_name(name).then(|| name.to_string())
}

/// Read `sync_dir` from an account's config the way onedrive does (default `~/OneDrive`).
fn sync_dir(confdir: &Path) -> String {
    let config = std::fs::read_to_string(confdir.join("config")).unwrap_or_default();
    let dir = config
        .lines()
        .filter_map(|l| config_value(l, "sync_dir"))
        .next_back()
        .unwrap_or_else(|| "~/OneDrive".into());
    match dir.strip_prefix("~/") {
        Some(rest) => home().join(rest).to_string_lossy().into_owned(),
        None => dir,
    }
}

/// New config text: sync_dir replaced if `new_dir` is set, GUI-managed skip_dir lines replaced by `excluded`.
fn rewrite_config(old: &str, new_dir: Option<&str>, excluded: &BTreeSet<String>) -> String {
    let mut lines: Vec<String> = old
        .lines()
        .filter(|l| !(new_dir.is_some() && config_value(l, "sync_dir").is_some()) && managed_exclude(l).is_none())
        .map(str::to_string)
        .collect();
    if let Some(d) = new_dir {
        lines.push(format!("sync_dir = \"{d}\""));
    }
    lines.extend(excluded.iter().map(|e| format!("skip_dir = \"/{e}\"")));
    lines.join("\n") + "\n"
}

/// A new sync folder must be absolute, empty (else its files would be uploaded) and not overlap another account's.
fn check_new_sync_dir(dir: &str, others: &[String]) -> Result<(), String> {
    let path = Path::new(dir);
    if !path.is_absolute() || dir.contains('"') {
        return Err(format!("Not a usable folder path: {dir}"));
    }
    if path.exists() {
        let empty = std::fs::read_dir(path).map_err(|e| format!("Cannot read {dir}: {e}"))?.next().is_none();
        if !empty {
            return Err("Choose an empty folder. Files already in it would be uploaded to OneDrive.".into());
        }
    }
    for other in others {
        if path.starts_with(other) || Path::new(other).starts_with(path) {
            return Err(format!("This folder overlaps another account's folder: {other}"));
        }
    }
    Ok(())
}

fn config_base() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"))
        .join("onedrive")
}

/// Accounts: `~/.config/onedrive` if it was ever used, and every `~/.config/onedrive/accounts/<name>`.
fn discover() -> Vec<(String, PathBuf)> {
    let base = config_base();
    let mut found = vec![];
    if base.join("refresh_token").exists() || base.join("items.sqlite3").exists() {
        found.push(("default".to_string(), base.clone()));
    }
    for entry in std::fs::read_dir(base.join("accounts")).into_iter().flatten().flatten() {
        if entry.path().is_dir() {
            found.push((entry.file_name().to_string_lossy().into_owned(), entry.path()));
        }
    }
    found
}

fn signed_in(confdir: &Path) -> bool {
    std::fs::metadata(confdir.join("refresh_token")).is_ok_and(|m| m.len() > 0)
}

fn terminate(pid: u32) {
    // SIGTERM only: onedrive finishes its current step and closes its database cleanly.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Monitor,
    SignIn,
}

#[derive(Default)]
struct Account {
    dir: PathBuf,
    sync_dir: String,
    status: &'static str,
    error: String,
    pid: Option<u32>,
    running: bool,
    stopping: bool,
    paused: bool,
    paused_until: u64,
    pause_gen: u64,
    needs_auth: bool,
    auth_dir: Option<PathBuf>,
    auth_url: String,
    resync_next: bool,
}

#[derive(Default)]
struct State {
    accounts: BTreeMap<String, Account>,
    recent: VecDeque<Value>,
}

impl State {
    fn apply(&mut self, name: &str, event: Event) {
        let a = self.accounts.get_mut(name).expect("known account");
        if let Event::Transfer { action, path, ok } = event {
            self.recent.push_front(json!({"account": name, "action": action, "path": path, "ok": ok, "time": now()}));
            self.recent.truncate(RECENT_MAX);
            return;
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
            }
            Event::SyncedWithFailures => {
                a.status = "error";
                a.error = "Some items couldn't be synced".into();
            }
            Event::SignedOut(msg) => {
                a.needs_auth = true;
                a.error = msg;
            }
            Event::Error(msg) => {
                a.status = "error";
                a.error = msg;
            }
            Event::Transfer { .. } | Event::AuthWaiting => {}
        }
    }
}

#[derive(Clone)]
struct Daemon {
    state: Arc<Mutex<State>>,
    conn: zbus::Connection,
}

impl Daemon {
    fn update<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        let r = f(&mut self.state.lock().unwrap());
        let conn = self.conn.clone();
        tokio::spawn(async move {
            if let Ok(e) = SignalEmitter::new(&conn, PATH) {
                let _ = Daemon::changed(&e).await;
            }
        });
        r
    }

    fn start(&self, name: &str) {
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

    async fn run(self, name: String, mode: Mode) {
        let (dir, resync, auth_dir) = {
            let mut s = self.state.lock().unwrap();
            let a = s.accounts.get_mut(&name).expect("known account");
            let resync = mode == Mode::Monitor && std::mem::take(&mut a.resync_next);
            (a.dir.clone(), resync, a.auth_dir.clone())
        };
        let mut cmd = Command::new(std::env::var_os("ONEDRIVE_BIN").unwrap_or_else(|| "onedrive".into()));
        match (mode, &auth_dir) {
            (Mode::SignIn, Some(auth)) => {
                let files = format!("{}:{}", auth.join("auth-url").display(), auth.join("response-url").display());
                cmd.arg("--confdir").arg(&dir).args(["--reauth", "--auth-files", &files]);
            }
            _ => {
                cmd.arg("--monitor").arg("--confdir").arg(&dir);
                if resync {
                    cmd.args(["--resync", "--resync-auth"]);
                }
            }
        }
        let spawned = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn();
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
                    if let Some(url) = auth_dir.as_ref().and_then(|d| std::fs::read_to_string(d.join("auth-url")).ok()) {
                        self.update(|s| s.accounts.get_mut(&name).expect("known account").auth_url = url.trim().to_string());
                    }
                }
                Some(event) => self.update(|s| s.apply(&name, event)),
                None => {}
            }
        }
        let exit = child.wait().await;

        let restart = self.update(|s| {
            let a = s.accounts.get_mut(&name).expect("known account");
            a.pid = None;
            a.running = false;
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
                return !a.needs_auth;
            }
            let restart = !a.paused && a.stopping; // resumed while still stopping
            if a.paused {
                a.status = "paused";
            } else if a.needs_auth {
                a.status = "signedout";
            } else if !restart {
                // shortcut: no auto-restart on unexpected exit, user presses Retry; add backoff restart if crashes prove common
                a.status = "stopped";
                if a.error.is_empty() {
                    a.error = match exit {
                        Ok(st) => format!("onedrive stopped unexpectedly ({st})"),
                        Err(e) => format!("onedrive stopped unexpectedly ({e})"),
                    };
                }
            }
            a.stopping = false;
            restart
        });
        if restart {
            self.start(&name);
        }
    }

    fn resume_account(&self, name: &str) {
        self.update(|s| {
            let a = s.accounts.get_mut(name).expect("known account");
            a.paused = false;
            a.paused_until = 0;
            a.pause_gen += 1;
            if a.needs_auth {
                a.status = "signedout";
            }
        });
        self.start(name);
    }

    /// Pause and wait until onedrive has exited.
    async fn stop_and_wait(&self, name: &str) {
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

    fn check(&self, account: &str) -> fdo::Result<()> {
        if self.state.lock().unwrap().accounts.contains_key(account) {
            Ok(())
        } else {
            Err(fdo::Error::InvalidArgs(format!("unknown account: {account}")))
        }
    }
}

#[zbus::interface(name = "org.onedrivegui.Daemon1")]
impl Daemon {
    /// JSON: {"accounts": [{name, syncDir, status, error, pausedUntil, authUrl}], "recent": [{account, action, path, ok, time}]}
    /// status: starting | syncing | synced | paused | error (onedrive running) | stopped (onedrive not running)
    ///         | signedout | signingin (authUrl set once the browser page can be opened)
    fn state(&self) -> String {
        let s = self.state.lock().unwrap();
        let accounts: Vec<Value> = s
            .accounts
            .iter()
            .map(|(name, a)| {
                json!({"name": name, "syncDir": a.sync_dir, "status": a.status, "error": a.error,
                       "pausedUntil": a.paused_until, "authUrl": a.auth_url})
            })
            .collect();
        json!({"accounts": accounts, "recent": s.recent}).to_string()
    }

    /// Stop syncing `account`; resume automatically after `seconds` (0 = until Resume).
    fn pause(&self, account: &str, seconds: u32) -> fdo::Result<()> {
        self.check(account)?;
        let generation = self.update(|s| {
            let a = s.accounts.get_mut(account).expect("checked");
            a.paused = true;
            a.status = "paused";
            a.pause_gen += 1;
            a.paused_until = if seconds > 0 { now() + u64::from(seconds) } else { 0 };
            if let Some(pid) = a.pid
                && !a.stopping
            {
                a.stopping = true;
                terminate(pid);
            }
            a.pause_gen
        });
        if seconds > 0 {
            let (d, name) = (self.clone(), account.to_string());
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(seconds.into())).await;
                let still = d.state.lock().unwrap().accounts[&name].pause_gen == generation;
                if still {
                    d.resume_account(&name);
                }
            });
        }
        Ok(())
    }

    fn resume(&self, account: &str) -> fdo::Result<()> {
        self.check(account)?;
        self.resume_account(account);
        Ok(())
    }

    /// Start the browser sign-in; the URL to open appears as `authUrl` in State. Cancel with Pause.
    fn sign_in(&self, account: &str) -> fdo::Result<()> {
        self.check(account)?;
        let base = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let safe_name: String = account.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
        // holds the sign-in code: private to this user
        let auth_dir = base.join("onedrive-gui").join(safe_name);
        let _ = std::fs::remove_dir_all(&auth_dir);
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&auth_dir)
            .and_then(|_| std::fs::set_permissions(&auth_dir, std::fs::Permissions::from_mode(0o700)))
            .map_err(|e| fdo::Error::Failed(format!("Cannot create {}: {e}", auth_dir.display())))?;
        let busy = self.update(|s| {
            let a = s.accounts.get_mut(account).expect("checked");
            if a.running {
                return true;
            }
            a.running = true;
            a.stopping = false;
            a.paused = false;
            a.status = "signingin";
            a.error.clear();
            a.auth_url.clear();
            a.auth_dir = Some(auth_dir);
            false
        });
        if busy {
            return Err(fdo::Error::Failed("Pause syncing before signing in again".into()));
        }
        tokio::spawn(self.clone().run(account.to_string(), Mode::SignIn));
        Ok(())
    }

    /// Hand onedrive the address of the page the browser landed on after sign-in.
    fn sign_in_response(&self, account: &str, url: &str) -> fdo::Result<()> {
        self.check(account)?;
        let url = url.trim();
        // shortcut: global Microsoft cloud only; national clouds (.us, .de, .cn) need their login hosts here
        if !url.starts_with("https://login.microsoftonline.com/") || !url.contains("code=") {
            return Err(fdo::Error::InvalidArgs(
                "That isn't the sign-in result address. After signing in, copy the full address of the blank page.".into(),
            ));
        }
        let dir = {
            let s = self.state.lock().unwrap();
            let a = &s.accounts[account];
            a.auth_dir.clone().filter(|_| !a.auth_url.is_empty())
        }
        .ok_or_else(|| fdo::Error::Failed("Sign-in isn't waiting for an answer".into()))?;
        // onedrive reads the file as soon as it exists: write elsewhere, then rename into place
        let tmp = dir.join("response.tmp");
        std::fs::write(&tmp, url)
            .and_then(|_| std::fs::rename(&tmp, dir.join("response-url")))
            .map_err(|e| fdo::Error::Failed(e.to_string()))
    }

    /// JSON: {"syncDir", "folders": [{name, excluded, selectable}]}: top-level folders on this PC
    /// plus folders the GUI excluded.
    fn folders(&self, account: &str) -> fdo::Result<String> {
        self.check(account)?;
        let (confdir, sync) = {
            let s = self.state.lock().unwrap();
            (s.accounts[account].dir.clone(), s.accounts[account].sync_dir.clone())
        };
        let config = std::fs::read_to_string(confdir.join("config")).unwrap_or_default();
        let excluded: BTreeSet<String> = config.lines().filter_map(managed_exclude).collect();
        // shortcut: lists folders present on this PC only; online-only folders need a Graph API listing
        let mut names: BTreeSet<String> = std::fs::read_dir(&sync)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.extend(excluded.iter().cloned());
        let folders: Vec<Value> = names
            .iter()
            .map(|n| json!({"name": n, "excluded": excluded.contains(n), "selectable": valid_folder_name(n)}))
            .collect();
        Ok(json!({"syncDir": sync, "folders": folders}).to_string())
    }

    /// Change the sync folder and/or excluded top-level folders. Stops onedrive, rewrites its config
    /// (backup kept as config.onedrive-gui.bak), then restarts it with a one-time --resync.
    async fn apply_settings(&self, account: &str, sync_dir: &str, excluded: Vec<String>) -> fdo::Result<()> {
        self.check(account)?;
        let (confdir, current, others, busy) = {
            let s = self.state.lock().unwrap();
            let a = &s.accounts[account];
            let others: Vec<String> = s.accounts.iter().filter(|(n, _)| *n != account).map(|(_, o)| o.sync_dir.clone()).collect();
            (a.dir.clone(), a.sync_dir.clone(), others, a.status == "signingin")
        };
        if busy {
            return Err(fdo::Error::Failed("Finish signing in first".into()));
        }
        let new_dir = (sync_dir != current).then_some(sync_dir);
        if let Some(d) = new_dir {
            check_new_sync_dir(d, &others).map_err(fdo::Error::InvalidArgs)?;
        }
        if let Some(bad) = excluded.iter().find(|e| !valid_folder_name(e)) {
            return Err(fdo::Error::InvalidArgs(format!("Folder name can't be excluded: {bad}")));
        }
        let config_path = confdir.join("config");
        let old = std::fs::read_to_string(&config_path).unwrap_or_default();
        let excluded: BTreeSet<String> = excluded.into_iter().collect();
        let old_excluded: BTreeSet<String> = old.lines().filter_map(managed_exclude).collect();
        if new_dir.is_none() && excluded == old_excluded {
            return Ok(());
        }

        // Never edit the config under a running onedrive.
        self.stop_and_wait(account).await;
        let write = || -> std::io::Result<()> {
            if config_path.exists() {
                std::fs::copy(&config_path, confdir.join("config.onedrive-gui.bak"))?;
            }
            let tmp = confdir.join("config.onedrive-gui.tmp");
            std::fs::write(&tmp, rewrite_config(&old, new_dir, &excluded))?;
            std::fs::rename(&tmp, &config_path)
        };
        let result = write();
        self.update(|s| {
            let a = s.accounts.get_mut(account).expect("checked");
            a.sync_dir = self::sync_dir(&confdir);
            // onedrive refuses to start after these changes until it has rebuilt its database
            a.resync_next |= result.is_ok();
        });
        self.resume_account(account);
        result.map_err(|e| fdo::Error::Failed(format!("Cannot write {}: {e}", config_path.display())))
    }

    #[zbus(signal)]
    async fn changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let accounts = discover();
    if accounts.is_empty() {
        eprintln!("No onedrive accounts found in {}", config_base().display());
        std::process::exit(1);
    }

    let conn = zbus::Connection::session().await?;
    let daemon = Daemon { state: Arc::default(), conn: conn.clone() };
    for (name, dir) in &accounts {
        let needs_auth = !signed_in(dir);
        let a = Account {
            sync_dir: sync_dir(dir),
            dir: dir.clone(),
            status: if needs_auth { "signedout" } else { "starting" },
            needs_auth,
            ..Default::default()
        };
        daemon.state.lock().unwrap().accounts.insert(name.clone(), a);
    }
    conn.object_server().at(PATH, daemon.clone()).await?;
    conn.request_name(NAME).await?;
    for (name, _) in &accounts {
        daemon.start(name);
    }

    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        _ = term.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
    // Let every onedrive exit cleanly before we go; no SIGKILL, ever (systemd's stop timeout is the backstop).
    daemon.update(|s| {
        for a in s.accounts.values_mut() {
            a.paused = true;
            if let Some(pid) = a.pid
                && !a.stopping
            {
                a.stopping = true;
                terminate(pid);
            }
        }
    });
    while daemon.state.lock().unwrap().accounts.values().any(|a| a.running) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transfer(action: &'static str, path: &str, ok: bool) -> Option<Event> {
        Some(Event::Transfer { action, path: path.into(), ok })
    }

    #[test]
    fn parses_onedrive_lines() {
        assert_eq!(parse("Uploading new file: ./Docs/a b.txt ... done"), transfer("upload", "./Docs/a b.txt", true));
        assert_eq!(parse("Uploading modified file: x.md ... failed!"), transfer("upload", "x.md", false));
        assert_eq!(parse("...Downloading file: Photos/p.jpg ... done\n"), transfer("download", "Photos/p.jpg", true));
        assert_eq!(parse("Deleting local file: old.txt"), transfer("delete-local", "old.txt", true));
        assert_eq!(parse("Deleting item from Microsoft OneDrive: ./gone"), transfer("delete-remote", "./gone", true));
        assert_eq!(
            parse("Uploading new file: ./db-pc-safeBackup-0003.kdbx ... done"),
            transfer("conflict", "./db-pc-safeBackup-0003.kdbx", true)
        );
        assert_eq!(parse("Starting a sync with Microsoft OneDrive"), Some(Event::Syncing));
        assert_eq!(parse("Sync with Microsoft OneDrive is complete"), Some(Event::Synced));
        assert_eq!(
            parse("Sync with Microsoft OneDrive has completed, however there are items that failed to sync."),
            Some(Event::SyncedWithFailures)
        );
        assert_eq!(parse("ERROR: Unable to reach the Microsoft OneDrive API"), Some(Event::Error("Unable to reach the Microsoft OneDrive API".into())));
        let reauth = "You will need to issue a --reauth and re-authorise this client to obtain a fresh auth token.";
        assert_eq!(parse(reauth), Some(Event::SignedOut(reauth.into())));
        assert_eq!(
            parse("Client requires authentication before proceeding. Waiting for --auth-files elements to be available."),
            Some(Event::AuthWaiting)
        );
        // in-progress transfer without result, and noise
        assert_eq!(parse("Downloading file: big.iso ... "), None);
        assert_eq!(parse("Processing API Response Bundle: 1"), None);
    }

    #[test]
    fn reads_sync_dir() {
        let dir = std::env::temp_dir().join(format!("odg-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config"), "# sync_dir = \"/nope\"\nsync_dir_permissions = \"700\"\nsync_dir = \"/data/od\"\n").unwrap();
        assert_eq!(sync_dir(&dir), "/data/od");
        std::fs::write(dir.join("config"), "sync_dir = \"~/OneDrive_x\"\n").unwrap();
        assert_eq!(sync_dir(&dir), home().join("OneDrive_x").to_string_lossy());
        std::fs::remove_file(dir.join("config")).unwrap();
        assert_eq!(sync_dir(&dir), home().join("OneDrive").to_string_lossy());
        std::fs::remove_dir(&dir).unwrap();
    }

    #[test]
    fn rewrites_config_keeping_user_lines() {
        let old = "# comment\nsync_dir = \"~/OD\"\nskip_dir = \"Temp|*.cache\"\nskip_dir = \"/Old\"\nmonitor_interval = \"300\"\n";
        let ex: BTreeSet<String> = ["Music".to_string()].into();
        assert_eq!(
            rewrite_config(old, None, &ex),
            "# comment\nsync_dir = \"~/OD\"\nskip_dir = \"Temp|*.cache\"\nmonitor_interval = \"300\"\nskip_dir = \"/Music\"\n"
        );
        assert_eq!(
            rewrite_config(old, Some("/data/od"), &BTreeSet::new()),
            "# comment\nskip_dir = \"Temp|*.cache\"\nmonitor_interval = \"300\"\nsync_dir = \"/data/od\"\n"
        );
        assert_eq!(managed_exclude("skip_dir = \"/Docs\""), Some("Docs".into()));
        assert_eq!(managed_exclude("skip_dir = \"/Docs/Sub\""), None);
        assert_eq!(managed_exclude("skip_dir = \"Docs\""), None);
        assert!(!valid_folder_name("a|b") && !valid_folder_name("x*") && valid_folder_name("My Files"));
    }

    #[test]
    fn checks_new_sync_dir() {
        let dir = std::env::temp_dir().join(format!("odg-sd-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("empty")).unwrap();
        std::fs::write(dir.join("file"), "x").unwrap();
        let d = dir.to_string_lossy().into_owned();
        assert!(check_new_sync_dir(&format!("{d}/empty"), &[]).is_ok());
        assert!(check_new_sync_dir(&format!("{d}/missing"), &[]).is_ok());
        assert!(check_new_sync_dir(&d, &[]).is_err()); // not empty
        assert!(check_new_sync_dir("relative/path", &[]).is_err());
        assert!(check_new_sync_dir(&format!("{d}/empty"), std::slice::from_ref(&d)).is_err()); // inside another account's
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
