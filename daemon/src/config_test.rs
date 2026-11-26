use super::*;

#[test]
fn reads_sync_dir() {
    let dir = std::env::temp_dir().join(format!("odg-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("config"),
        "# sync_dir = \"/nope\"\nsync_dir_permissions = \"700\"\nsync_dir = \"/data/od\"\n",
    )
    .unwrap();
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
    assert!(
        valid_account_name("me@outlook.com")
            && !valid_account_name("../x")
            && !valid_account_name(".hidden")
    );
    assert!(!valid_account_name("a b") && !valid_account_name(""));
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
