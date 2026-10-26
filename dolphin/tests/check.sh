#!/usr/bin/env bash
# Overlay plugin against the real daemon and fake onedrive on a private D-Bus. Never touches a real account.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
[[ -n ${CHECK_INNER:-} ]] || CHECK_INNER=1 exec dbus-run-session -- "$0"
cargo build --quiet --manifest-path "$here/../../daemon/Cargo.toml"
cmake -S "$here/.." -B "$here/../build" -DBUILD_TESTING=ON >/dev/null && cmake --build "$here/../build" >/dev/null
tmp=$(mktemp -d)
daemon=
trap '[[ -n $daemon ]] && kill -TERM "$daemon" 2>/dev/null && wait "$daemon"; rm -rf "$tmp"' EXIT
A=$tmp/onedrive/accounts/test@example.com S=$tmp/sync
mkdir -p "$A" "$S/Docs" "$S/Music"
echo token > "$A/refresh_token"
printf 'sync_dir = "%s"\nskip_dir = "/Music"\n' "$S" > "$A/config"
export XDG_CONFIG_HOME=$tmp XDG_RUNTIME_DIR=$tmp ONEDRIVE_BIN=$here/../../daemon/tests/fake-onedrive FAKE_LOG=$tmp/fake.log
"$here/../../daemon/target/debug/onedrive-gui-daemon" & daemon=$!
for _ in $(seq 50); do gdbus call --session --dest org.onedrivegui.Daemon --object-path /org/onedrivegui/Daemon \
    --method org.onedrivegui.Daemon1.State 2>/dev/null | grep -q '"status":"synced"' && break; sleep 0.1; done

out=$("$here/../build/overlay-check" "$here/../build/onedrivegui-overlay.so" \
    "$S" "$S/Docs/a.txt" "$S/db-pc-safeBackup-0001.kdbx" "$S/Music/song.mp3" "$S/Docs/x.tmp" "$tmp/elsewhere")
expected="$S emblem-checked
$S/Docs/a.txt emblem-checked
$S/db-pc-safeBackup-0001.kdbx emblem-warning
$S/Music/song.mp3 
$S/Docs/x.tmp 
$tmp/elsewhere "
[[ $out == "$expected" ]] || { echo "FAIL:"; diff <(echo "$expected") <(echo "$out"); exit 1; }
echo "overlay: OK"
