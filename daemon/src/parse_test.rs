use super::*;

fn transfer(action: &'static str, path: &str, ok: bool) -> Option<Event> {
    Some(Event::Transfer {
        action,
        path: path.into(),
        ok,
    })
}

#[test]
fn parses_onedrive_lines() {
    assert_eq!(
        parse("Uploading new file: ./Docs/a b.txt ... done"),
        transfer("upload", "./Docs/a b.txt", true)
    );
    assert_eq!(
        parse("Uploading modified file: x.md ... failed!"),
        transfer("upload", "x.md", false)
    );
    assert_eq!(
        parse("...Downloading file: Photos/p.jpg ... done\n"),
        transfer("download", "Photos/p.jpg", true)
    );
    assert_eq!(
        parse("Deleting local file: old.txt"),
        transfer("delete-local", "old.txt", true)
    );
    assert_eq!(
        parse("Deleting item from Microsoft OneDrive: ./gone"),
        transfer("delete-remote", "./gone", true)
    );
    assert_eq!(
        parse("Uploading new file: ./db-pc-safeBackup-0003.kdbx ... done"),
        transfer("conflict", "./db-pc-safeBackup-0003.kdbx", true)
    );
    assert_eq!(
        parse("Starting a sync with Microsoft OneDrive"),
        Some(Event::Syncing)
    );
    assert_eq!(
        parse("Sync with Microsoft OneDrive is complete"),
        Some(Event::Synced)
    );
    assert_eq!(
        parse(
            "Sync with Microsoft OneDrive has completed, however there are items that failed to sync."
        ),
        Some(Event::SyncedWithFailures)
    );
    assert_eq!(
        parse("ERROR: Unable to reach the Microsoft OneDrive API"),
        Some(Event::Error(
            "Unable to reach the Microsoft OneDrive API".into()
        ))
    );
    let reauth = "You will need to issue a --reauth and re-authorise this client to obtain a fresh auth token.";
    assert_eq!(parse(reauth), Some(Event::SignedOut(reauth.into())));
    assert_eq!(
        parse(
            "Client requires authentication before proceeding. Waiting for --auth-files elements to be available."
        ),
        Some(Event::AuthWaiting)
    );
    assert_eq!(
        parse("Downloading: Videos/big.mp4 ... 45%  |  ETA    00:00:12"),
        Some(Event::Progress {
            path: "Videos/big.mp4".into(),
            percent: 45
        })
    );
    assert_eq!(
        parse("Uploading: ./a ... b.iso ... 100% | DONE in 00:01:02"),
        Some(Event::Progress {
            path: "./a ... b.iso".into(),
            percent: 100
        })
    );
    assert_eq!(
        parse("Number of items to download from Microsoft OneDrive: 12"),
        Some(Event::Planned(12))
    );
    assert_eq!(
        parse("New items to upload to Microsoft OneDrive: 3"),
        Some(Event::Planned(3))
    );
    assert_eq!(
        parse_quota("Used:      0.15 GB (161061273 bytes)"),
        Some(("Used", 161061273))
    );
    assert_eq!(
        parse_quota("Total:     5 GB (5368709120 bytes)"),
        Some(("Total", 5368709120))
    );
    assert_eq!(parse_quota("Deleted:   0 GB (0 bytes)"), None);
    // in-progress transfer without result, and noise
    assert_eq!(parse("Downloading file: big.iso ... "), None);
    assert_eq!(parse("Processing API Response Bundle: 1"), None);
}
