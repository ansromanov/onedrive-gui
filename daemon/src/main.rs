//! Supervises one `onedrive --monitor` process per account and exposes status on D-Bus.
//!
//! Safety rules: never touch synced files or the onedrive database; only pass `--monitor --confdir`,
//! plus `--resync --resync-auth` once after the user applies settings (onedrive refuses to start
//! otherwise), `--reauth --auth-files` to sign in, and `--display-quota` (read-only) before each
//! start; stop onedrive with SIGTERM only so it can
//! shut down cleanly; edit the config only while onedrive is stopped, atomically, with a backup.

mod config;
mod dbus;
mod parse;
mod state;
mod supervise;

use std::sync::Arc;
use std::time::Duration;

use config::{discover, excluded_folders, signed_in, sync_dir};
use state::Account;
use supervise::{Daemon, terminate};

pub(crate) const NAME: &str = "org.onedrivegui.Daemon";
pub(crate) const PATH: &str = "/org/onedrivegui/Daemon";

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // no accounts yet is fine: the GUI can add one
    let accounts = discover();

    let conn = zbus::Connection::session().await?;
    let daemon = Daemon {
        state: Arc::default(),
        conn: conn.clone(),
    };
    for (name, dir) in &accounts {
        let needs_auth = !signed_in(dir);
        let a = Account {
            sync_dir: sync_dir(dir),
            excluded: excluded_folders(dir),
            dir: dir.clone(),
            status: if needs_auth { "signedout" } else { "starting" },
            needs_auth,
            ..Default::default()
        };
        daemon
            .state
            .lock()
            .unwrap()
            .accounts
            .insert(name.clone(), a);
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
    while daemon
        .state
        .lock()
        .unwrap()
        .accounts
        .values()
        .any(|a| a.running)
    {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(())
}
