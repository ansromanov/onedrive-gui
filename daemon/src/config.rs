//! Reading and rewriting onedrive account configs, and finding accounts on disk.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Account names become directory names under ~/.config/onedrive/accounts.
pub(crate) fn valid_account_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "@._+-".contains(c))
}

pub(crate) fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

/// Value of a `key = "value"` config line, if this line sets `key`.
pub(crate) fn config_value(line: &str, key: &str) -> Option<String> {
    let v = line
        .trim()
        .strip_prefix(key)?
        .trim_start()
        .strip_prefix('=')?;
    Some(v.trim().trim_matches('"').to_string())
}

/// Folder names the GUI can write as `skip_dir = "/<name>"`: one segment, no skip_dir pattern syntax.
pub(crate) fn valid_folder_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', '|', '*', '?', '"'])
}

/// Top-level folder excluded by a GUI-managed line `skip_dir = "/<name>"`. Other skip_dir lines are the user's.
pub(crate) fn managed_exclude(line: &str) -> Option<String> {
    let v = config_value(line, "skip_dir")?;
    let name = v.strip_prefix('/')?;
    valid_folder_name(name).then(|| name.to_string())
}

/// Top-level folders the GUI excluded in an account's config.
pub(crate) fn excluded_folders(confdir: &Path) -> BTreeSet<String> {
    std::fs::read_to_string(confdir.join("config"))
        .unwrap_or_default()
        .lines()
        .filter_map(managed_exclude)
        .collect()
}

/// Read `sync_dir` from an account's config the way onedrive does (default `~/OneDrive`).
pub(crate) fn sync_dir(confdir: &Path) -> String {
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
pub(crate) fn rewrite_config(
    old: &str,
    new_dir: Option<&str>,
    excluded: &BTreeSet<String>,
) -> String {
    let mut lines: Vec<String> = old
        .lines()
        .filter(|l| {
            !(new_dir.is_some() && config_value(l, "sync_dir").is_some())
                && managed_exclude(l).is_none()
        })
        .map(str::to_string)
        .collect();
    if let Some(d) = new_dir {
        lines.push(format!("sync_dir = \"{d}\""));
    }
    lines.extend(excluded.iter().map(|e| format!("skip_dir = \"/{e}\"")));
    lines.join("\n") + "\n"
}

/// A new sync folder must be absolute, empty (else its files would be uploaded) and not overlap another account's.
pub(crate) fn check_new_sync_dir(dir: &str, others: &[String]) -> Result<(), String> {
    let path = Path::new(dir);
    if !path.is_absolute() || dir.contains('"') {
        return Err(format!("Not a usable folder path: {dir}"));
    }
    if path.exists() {
        let empty = std::fs::read_dir(path)
            .map_err(|e| format!("Cannot read {dir}: {e}"))?
            .next()
            .is_none();
        if !empty {
            return Err(
                "Choose an empty folder. Files already in it would be uploaded to OneDrive.".into(),
            );
        }
    }
    for other in others {
        if path.starts_with(other) || Path::new(other).starts_with(path) {
            return Err(format!(
                "This folder overlaps another account's folder: {other}"
            ));
        }
    }
    Ok(())
}

pub(crate) fn config_base() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"))
        .join("onedrive")
}

/// Accounts: `~/.config/onedrive` if it was ever used, and every `~/.config/onedrive/accounts/<name>`.
pub(crate) fn discover() -> Vec<(String, PathBuf)> {
    let base = config_base();
    let mut found = vec![];
    if base.join("refresh_token").exists() || base.join("items.sqlite3").exists() {
        found.push(("default".to_string(), base.clone()));
    }
    for entry in std::fs::read_dir(base.join("accounts"))
        .into_iter()
        .flatten()
        .flatten()
    {
        if entry.path().is_dir() {
            found.push((
                entry.file_name().to_string_lossy().into_owned(),
                entry.path(),
            ));
        }
    }
    found
}

pub(crate) fn signed_in(confdir: &Path) -> bool {
    std::fs::metadata(confdir.join("refresh_token")).is_ok_and(|m| m.len() > 0)
}

#[cfg(test)]
#[path = "config_test.rs"]
mod tests;
