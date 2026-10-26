# OneDrive GUI

Windows-style tray + activity flyout for [abraunegg/onedrive](https://github.com/abraunegg/onedrive).

- `daemon/` (Rust): runs one `onedrive --monitor --confdir <account>` per authorised account, parses its output, serves status on D-Bus (`org.onedrivegui.Daemon`).
- `app/` (C++/Qt6/QML): tray icon and flyout; talks only to the daemon.

## Safety

The GUI never touches synced files, the onedrive database or its config. The daemon only ever passes
`--monitor --confdir`, and stops onedrive with SIGTERM only (no SIGKILL), so onedrive exits cleanly.
Its systemd unit conflicts with `onedrive.service` so two instances never sync the same account.
Note: `onedrive@.service` template instances are not covered; don't run them alongside.

## Build

```sh
cargo build --release --manifest-path daemon/Cargo.toml
cmake -S app -B app/build -G Ninja -DCMAKE_BUILD_TYPE=Release && cmake --build app/build
```

## Test

```sh
cargo test --manifest-path daemon/Cargo.toml
daemon/tests/smoke.sh   # real daemon on a private D-Bus with a fake onedrive; no real account involved
```

## Install

Accounts live in `~/.config/onedrive` or `~/.config/onedrive/accounts/<name>/`; sign in from Settings.

```sh
cmake -S app -B app/build-release -G Ninja -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=$HOME/.local
cmake --build app/build-release && cmake --install app/build-release
install -Dm755 daemon/target/release/onedrive-gui-daemon ~/.local/bin/onedrive-gui-daemon
install -Dm644 daemon/onedrive-gui-daemon.service ~/.config/systemd/user/onedrive-gui-daemon.service
install -Dm644 ~/.local/share/applications/onedrive-gui.desktop ~/.config/autostart/onedrive-gui.desktop
systemctl --user daemon-reload && systemctl --user enable --now onedrive-gui-daemon
```

## Settings changes

Changing the sync folder or excluded folders stops onedrive, rewrites its `config` (previous copy kept as
`config.onedrive-gui.bak`), and restarts it once with `--resync`, which onedrive requires after such changes.
A resync deletes nothing on either side; differing files are kept as both copies. Excluded folders stay on
disk. A new sync folder must be empty; the old one is left as is.
