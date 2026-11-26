//! The D-Bus interface the GUI talks to.

use std::collections::BTreeSet;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};
use zbus::fdo;
use zbus::object_server::SignalEmitter;

use crate::config::{
    check_new_sync_dir, config_base, excluded_folders, managed_exclude, rewrite_config, sync_dir,
    valid_account_name, valid_folder_name,
};
use crate::state::{Account, Mode, now};
use crate::supervise::{Daemon, terminate};

#[zbus::interface(name = "org.onedrivegui.Daemon1")]
impl Daemon {
    /// JSON: {"accounts": [{name, syncDir, status, error, pausedUntil, authUrl, retryAt, planned, transferred,
    ///         current, percent, quotaUsed, quotaTotal, excluded}], "recent": [{account, action, path, ok, time}]}
    /// status: starting | syncing | synced | paused | error (onedrive running) | stopped (onedrive not running,
    ///         retryAt > 0 if a restart is scheduled) | signedout | signingin (authUrl set once the browser page can be opened)
    /// planned/transferred count files in the current sync; current/percent is a big file in progress; quota 0 = unknown
    fn state(&self) -> String {
        let s = self.state.lock().unwrap();
        let accounts: Vec<Value> = s
            .accounts
            .iter()
            .map(|(name, a)| {
                json!({"name": name, "syncDir": a.sync_dir, "status": a.status, "error": a.error,
                       "pausedUntil": a.paused_until, "authUrl": a.auth_url, "retryAt": a.retry_at,
                       "planned": a.planned, "transferred": a.transferred, "current": a.current, "percent": a.percent,
                       "quotaUsed": a.quota_used, "quotaTotal": a.quota_total, "excluded": a.excluded})
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
            a.paused_until = if seconds > 0 {
                now() + u64::from(seconds)
            } else {
                0
            };
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
        let base = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let safe_name: String = account
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        // holds the sign-in code: private to this user
        let auth_dir = base.join("onedrive-gui").join(safe_name);
        let _ = std::fs::remove_dir_all(&auth_dir);
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&auth_dir)
            .and_then(|_| {
                std::fs::set_permissions(&auth_dir, std::fs::Permissions::from_mode(0o700))
            })
            .map_err(|e| {
                fdo::Error::Failed(format!("Cannot create {}: {e}", auth_dir.display()))
            })?;
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
            return Err(fdo::Error::Failed(
                "Pause syncing before signing in again".into(),
            ));
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
            (
                s.accounts[account].dir.clone(),
                s.accounts[account].sync_dir.clone(),
            )
        };
        let excluded = excluded_folders(&confdir);
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
    async fn apply_settings(
        &self,
        account: &str,
        sync_dir: &str,
        excluded: Vec<String>,
    ) -> fdo::Result<()> {
        self.check(account)?;
        let (confdir, current, others, busy) = {
            let s = self.state.lock().unwrap();
            let a = &s.accounts[account];
            let others: Vec<String> = s
                .accounts
                .iter()
                .filter(|(n, _)| *n != account)
                .map(|(_, o)| o.sync_dir.clone())
                .collect();
            (
                a.dir.clone(),
                a.sync_dir.clone(),
                others,
                a.status == "signingin",
            )
        };
        if busy {
            return Err(fdo::Error::Failed("Finish signing in first".into()));
        }
        let new_dir = (sync_dir != current).then_some(sync_dir);
        if let Some(d) = new_dir {
            check_new_sync_dir(d, &others).map_err(fdo::Error::InvalidArgs)?;
        }
        if let Some(bad) = excluded.iter().find(|e| !valid_folder_name(e)) {
            return Err(fdo::Error::InvalidArgs(format!(
                "Folder name can't be excluded: {bad}"
            )));
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
            a.excluded = excluded_folders(&confdir);
            // onedrive refuses to start after these changes until it has rebuilt its database
            a.resync_next |= result.is_ok();
        });
        self.resume_account(account);
        result
            .map_err(|e| fdo::Error::Failed(format!("Cannot write {}: {e}", config_path.display())))
    }

    /// Create a new account in ~/.config/onedrive/accounts/<name> syncing to `sync_dir`; then SignIn.
    fn add_account(&self, name: &str, sync_dir: &str) -> fdo::Result<()> {
        if !valid_account_name(name) {
            return Err(fdo::Error::InvalidArgs(
                "Use the account's email address as its name".into(),
            ));
        }
        let others: Vec<String> = self
            .state
            .lock()
            .unwrap()
            .accounts
            .values()
            .map(|a| a.sync_dir.clone())
            .collect();
        check_new_sync_dir(sync_dir, &others).map_err(fdo::Error::InvalidArgs)?;
        let dir = config_base().join("accounts").join(name);
        if dir.exists() || self.state.lock().unwrap().accounts.contains_key(name) {
            return Err(fdo::Error::InvalidArgs(format!(
                "An account named {name} already exists"
            )));
        }
        // the account folder will hold its sign-in token: private to this user
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .and_then(|_| {
                std::fs::write(dir.join("config"), format!("sync_dir = \"{sync_dir}\"\n"))
            })
            .map_err(|e| fdo::Error::Failed(format!("Cannot create {}: {e}", dir.display())))?;
        self.update(|s| {
            let a = Account {
                sync_dir: sync_dir.to_string(),
                dir,
                status: "signedout",
                needs_auth: true,
                ..Default::default()
            };
            s.accounts.insert(name.to_string(), a);
        });
        Ok(())
    }

    #[zbus(signal)]
    pub(crate) async fn changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}
