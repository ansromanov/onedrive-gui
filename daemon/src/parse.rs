//! Maps lines of onedrive console output to events.

#[derive(Debug, PartialEq)]
pub(crate) enum Event {
    Syncing,
    Synced,
    SyncedWithFailures,
    Transfer {
        action: &'static str,
        path: String,
        ok: bool,
    },
    /// Files onedrive announced it is about to transfer.
    Planned(u64),
    /// Big-file transfer progress.
    Progress {
        path: String,
        percent: u32,
    },
    SignedOut(String),
    AuthWaiting,
    Error(String),
}

/// Map one line of onedrive console output to an event.
/// Strings come from onedrive v2.5.x src/sync.d, src/onedrive.d; unknown lines are ignored.
pub(crate) fn parse(line: &str) -> Option<Event> {
    const TRANSFERS: [(&str, &str); 9] = [
        ("Uploading new file: ", "upload"),
        ("Uploading modified file: ", "upload"),
        ("Downloading file: ", "download"),
        ("Deleting item from Microsoft OneDrive: ", "delete-remote"),
        (
            "Deleting online file from Microsoft OneDrive: ",
            "delete-remote",
        ),
        (
            "Deleting online folder from Microsoft OneDrive: ",
            "delete-remote",
        ),
        ("Deleting local file: ", "delete-local"),
        ("Deleting local directory: ", "delete-local"),
        (
            "Moving this local file to the configured 'Recycle Bin': ",
            "delete-local",
        ),
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
        let action = if action == "upload" && path.contains("-safeBackup-") {
            "conflict"
        } else {
            action
        };
        return Some(Event::Transfer {
            action,
            path: path.to_string(),
            ok,
        });
    }
    for prefix in ["Downloading: ", "Uploading: "] {
        // "Downloading: <path> ... 45%  |  ETA    00:00:12" (also "| DONE in ...")
        let Some(i) = line.find(prefix) else { continue };
        let (path, tail) = line[i + prefix.len()..].rsplit_once(" ... ")?;
        let percent = tail.split('%').next()?.trim().parse().ok()?;
        return Some(Event::Progress {
            path: path.to_string(),
            percent,
        });
    }
    for prefix in [
        "Number of items to download from Microsoft OneDrive: ",
        "New items to upload to Microsoft OneDrive: ",
        "Changed local items to upload to Microsoft OneDrive: ",
    ] {
        if let Some(n) = line
            .find(prefix)
            .and_then(|i| line[i + prefix.len()..].trim().parse().ok())
        {
            return Some(Event::Planned(n));
        }
    }
    if line.contains("Starting a sync with Microsoft OneDrive")
        || line.contains("Syncing changes from Microsoft OneDrive")
    {
        Some(Event::Syncing)
    } else if line.contains("Sync with Microsoft OneDrive is complete") {
        Some(Event::Synced)
    } else if line.contains(
        "Sync with Microsoft OneDrive has completed, however there are items that failed to sync",
    ) {
        Some(Event::SyncedWithFailures)
    } else if line.contains("Waiting for --auth-files elements to be available") {
        Some(Event::AuthWaiting)
    } else if line.contains("--reauth") {
        Some(Event::SignedOut(
            line.trim_start_matches("ERROR: ").to_string(),
        ))
    } else {
        line.find("ERROR: ")
            .map(|i| Event::Error(line[i + 7..].to_string()))
    }
}

/// `("Total" | "Used", bytes)` from `onedrive --display-quota` output, e.g. "Used:      0.15 GB (161061273 bytes)".
pub(crate) fn parse_quota(line: &str) -> Option<(&str, u64)> {
    let (key, rest) = line.split_once(':')?;
    let bytes = rest
        .split_once('(')?
        .1
        .strip_suffix(" bytes)")?
        .trim()
        .parse()
        .ok()?;
    matches!(key, "Total" | "Used").then_some((key, bytes))
}

#[cfg(test)]
#[path = "parse_test.rs"]
mod tests;
