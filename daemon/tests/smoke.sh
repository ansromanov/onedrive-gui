#!/usr/bin/env bash
# End-to-end daemon check on a private D-Bus with a fake onedrive. Never touches a real account.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
[[ -n ${SMOKE_INNER:-} ]] || SMOKE_INNER=1 exec dbus-run-session -- "$0"

cargo build --quiet --manifest-path "$here/../Cargo.toml"
tmp=$(mktemp -d)
daemon=
trap '[[ -n $daemon ]] && kill -TERM "$daemon" 2>/dev/null && wait "$daemon"; rm -rf "$tmp"' EXIT
acct=test@example.com new=new@example.com
A=$tmp/onedrive/accounts/$acct N=$tmp/onedrive/accounts/$new
mkdir -p "$A" "$N" "$tmp/sync/Docs" "$tmp/run"
echo token > "$A/refresh_token"
printf 'sync_dir = "%s"\nskip_dir = "*.tmp"\n' "$tmp/sync" > "$A/config"
export XDG_CONFIG_HOME=$tmp XDG_RUNTIME_DIR=$tmp/run ONEDRIVE_BIN=$here/fake-onedrive FAKE_LOG=$tmp/fake.log FAKE_CRASH=$tmp/crash

"$here/../target/debug/onedrive-gui-daemon" &
daemon=$!

call() { gdbus call --session --dest org.onedrivegui.Daemon --object-path /org/onedrivegui/Daemon --method "org.onedrivegui.Daemon1.$1" "${@:2}"; }
fail() { echo "FAIL: $*"; call State || true; cat "$FAKE_LOG" || true; exit 1; }
wait_for() {
    for _ in $(seq 50); do call State 2>/dev/null | grep -qE "$1" && return; sleep 0.1; done
    fail "state never matched $1"
}
status() { echo "\"name\":\"$1\",\"pausedUntil\":[0-9]+,[^}]*\"status\":\"$2\""; }
last_args() { grep "^args: .*$1" "$FAKE_LOG" | tail -1; }

# --- monitor, pause, resume
wait_for "$(status $acct synced)"
call State | grep -q '"action":"download","ok":true,"path":"Docs/a.txt"' || fail "download not in recent"
call State | grep -q '"action":"conflict","ok":true,"path":"./db-pc-safeBackup-0001.kdbx"' || fail "conflict not flagged"
call State | grep -q '"quotaTotal":5368709120,"quotaUsed":161061273' || fail "quota not read"

call Pause "$acct" 0 >/dev/null
call State | grep -qE "$(status $acct paused)" || fail "pause not shown immediately"
sleep 0.5
call Resume "$acct" >/dev/null
wait_for "$(status $acct synced)"
grep -q 'end: TERM' "$FAKE_LOG" || fail "onedrive not stopped with SIGTERM"

call Pause "$acct" 2 >/dev/null
call State | grep -qE "$(status $acct paused)" || fail "timed pause not shown"
wait_for "$(status $acct synced)" # auto-resume after 2s

call Pause nobody 0 >/dev/null 2>&1 && fail "unknown account accepted"

# --- crash: stopped, restarted on its own after the first backoff step (10s)
touch "$tmp/crash"
call Pause "$acct" 0 >/dev/null; sleep 0.5; call Resume "$acct" >/dev/null
wait_for "\"retryAt\":[1-9][0-9]*,\"status\":\"stopped\""
for _ in $(seq 150); do call State | grep -qE "$(status $acct synced)" && break; sleep 0.1; done
call State | grep -qE "$(status $acct synced)" || fail "not restarted after crash"

# --- sign-in for an account without a token
call State | grep -qE "$(status $new signedout)" || fail "account without token not signed out"
call SignIn "$new" >/dev/null
wait_for '"authUrl":"https://login.microsoftonline.com/common/oauth2/v2.0/authorize\?client_id=fake"'
[[ $(stat -c %a "$tmp/run/onedrive-gui/new_example_com") == 700 ]] || fail "sign-in dir not private"
call SignInResponse "$new" 'https://evil.example/?code=1' >/dev/null 2>&1 && fail "foreign redirect accepted"
call SignInResponse "$new" 'https://login.microsoftonline.com/common/oauth2/nativeclient?code=abc' >/dev/null
wait_for "$(status $new synced)"
[[ ! -e $tmp/run/onedrive-gui/new_example_com ]] || fail "sign-in files left behind"

# --- settings: exclude a folder -> config rewritten with backup, one-time resync
call Folders "$acct" | grep -q '"excluded":false,"name":"Docs","selectable":true' || fail "Docs not listed"
call ApplySettings "$acct" "$tmp/sync" "['Docs']" >/dev/null
grep -qx 'skip_dir = "/Docs"' "$A/config" || fail "skip_dir not written"
grep -qx 'skip_dir = "\*.tmp"' "$A/config" || fail "user skip_dir lost"
cmp -s "$A/config.onedrive-gui.bak" <(printf 'sync_dir = "%s"\nskip_dir = "*.tmp"\n' "$tmp/sync") || fail "backup wrong"
wait_for "$(status $acct synced)"
[[ $(last_args $acct) == *"--resync --resync-auth" ]] || fail "no resync after settings change"
call Folders "$acct" | grep -q '"excluded":true,"name":"Docs"' || fail "Docs not shown excluded"
call State | grep -q '"excluded":\["Docs"\]' || fail "excluded not in state"

call Pause "$acct" 0 >/dev/null; sleep 0.5; call Resume "$acct" >/dev/null
wait_for "$(status $acct synced)"
[[ $(last_args $acct) == *"--confdir $A" ]] || fail "resync not one-time"

before=$(md5sum < "$A/config")
call ApplySettings "$acct" "$tmp" "['Docs']" >/dev/null 2>&1 && fail "non-empty sync folder accepted"
call ApplySettings "$acct" "$tmp/sync" "['a|b']" >/dev/null 2>&1 && fail "pattern accepted as folder"
[[ $(md5sum < "$A/config") == "$before" ]] || fail "config changed by rejected settings"

call ApplySettings "$acct" "$tmp/newsync" "['Docs']" >/dev/null
grep -qx "sync_dir = \"$tmp/newsync\"" "$A/config" || fail "sync_dir not written"
call State | grep -q "\"syncDir\":\"$tmp/newsync\"" || fail "new sync dir not reported"
wait_for "$(status $acct synced)"

# --- add an account
call AddAccount 'added@example.com' "$tmp/added" >/dev/null
D=$tmp/onedrive/accounts/added@example.com
[[ $(stat -c %a "$D") == 700 ]] || fail "new account dir not private"
cmp -s "$D/config" <(printf 'sync_dir = "%s"\n' "$tmp/added") || fail "new account config wrong"
call State | grep -qE "$(status added@example.com signedout)" || fail "new account not signed out"
call AddAccount 'added@example.com' "$tmp/added2" >/dev/null 2>&1 && fail "duplicate account accepted"
call AddAccount '../evil' "$tmp/evil" >/dev/null 2>&1 && fail "bad account name accepted"
call AddAccount 'x@example.com' "$tmp/sync" >/dev/null 2>&1 && fail "overlapping sync folder accepted"
[[ ! -e $tmp/onedrive/accounts/x@example.com ]] || fail "rejected account left a folder"

# --- clean shutdown
kill -TERM "$daemon"
wait "$daemon" || fail "daemon exit code $?"
daemon=
pgrep -f "$here/fake-onedrive" >/dev/null && fail "onedrive left running"

# every onedrive run ended cleanly (SIGTERM, finished sign-in, or its own exit)
[[ $(grep -c '^args:' "$FAKE_LOG") == $(grep -c '^end:' "$FAKE_LOG") ]] || fail "an onedrive run did not end cleanly"
# only these argument sets ever reach onedrive
while read -r line; do
    case $line in
    "args: --monitor --confdir $A" | "args: --monitor --confdir $N") ;;
    "args: --confdir $A --display-quota" | "args: --confdir $N --display-quota") ;;
    "args: --monitor --confdir $A --resync --resync-auth") ;;
    "args: --confdir $N --reauth --auth-files $tmp/run/onedrive-gui/new_example_com/auth-url:$tmp/run/onedrive-gui/new_example_com/response-url") ;;
    *) fail "unexpected onedrive args: $line" ;;
    esac
done < <(grep '^args:' "$FAKE_LOG")
echo "smoke: OK"
