#!/usr/bin/env bash
# Installer for OneDrive GUI.
#   ./install.sh               install (Arch: pacman package incl. Dolphin plugin; elsewhere: ~/.local)
#   ./install.sh --user        force the ~/.local install (no Dolphin plugin)
#   ./install.sh --uninstall   remove whatever this script installed
#   ./install.sh --no-start    don't enable/start the daemon
#   ./install.sh --yes         don't ask questions
set -euo pipefail

SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOCAL="$HOME/.local"
UNIT="onedrive-gui-daemon"
MODE=auto ACTION=install START=1 YES=0

while [ $# -gt 0 ]; do
    case $1 in
        --user) MODE=user ;;
        --uninstall) ACTION=uninstall ;;
        --no-start) START=0 ;;
        -y|--yes) YES=1 ;;
        -h|--help) sed -n '2,8p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
    shift
done

say()  { printf '\033[1m==> %s\033[0m\n' "$*"; }
warn() { printf '\033[33mwarning:\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }
ask()  { [ "$YES" = 1 ] && return 0; read -r -p "$1 [Y/n] " a; [ "${a:-y}" = y ] || [ "${a:-y}" = Y ]; }
have() { command -v "$1" >/dev/null 2>&1; }

is_arch() { have pacman && have makepkg; }
pkg_installed() { is_arch && pacman -Qi onedrive-gui >/dev/null 2>&1; }

# Files an earlier ~/.local install leaves behind; they shadow the package's copies.
local_files=(
    "$LOCAL/bin/onedrive-gui" "$LOCAL/bin/onedrive-gui-daemon"
    "$LOCAL/share/applications/onedrive-gui.desktop"
    "$HOME/.config/systemd/user/$UNIT.service"
    "$HOME/.config/autostart/onedrive-gui.desktop"
)
remove_local() {
    local f found=0
    for f in "${local_files[@]}"; do [ -e "$f" ] && { rm -f "$f"; found=1; }; done
    [ "$found" = 1 ] && say "Removed ~/.local copies"
    return 0
}

stop_daemon() {
    # SIGTERM via systemd: the daemon passes it on to onedrive so it exits cleanly.
    systemctl --user disable --now "$UNIT" 2>/dev/null || true
    pkill -x onedrive-gui 2>/dev/null || true
}

check_onedrive() {
    have onedrive && return 0
    warn "'onedrive' (abraunegg/onedrive) is not installed; the GUI needs it to sync."
    if is_arch && have yay && [ -t 0 ] && ask "Install onedrive-abraunegg from the AUR with yay?"; then
        # No --noconfirm on purpose: yay's prompts are where you review the AUR build.
        yay -S --needed onedrive-abraunegg
    else
        warn "Install it later (Arch AUR: onedrive-abraunegg) before adding an account."
    fi
}

install_arch() {
    say "Building and installing the pacman package"
    # Never leave a half-removed old ~/.local install shadowing the package.
    stop_daemon; remove_local
    # Runs as you: makepkg refuses root and calls sudo pacman itself for -s (build deps) and -i (install).
    local opts=(-fsci); [ "$YES" = 1 ] && opts+=(--noconfirm)
    (cd "$SRC/packaging/arch" && makepkg "${opts[@]}")
}

install_user() {
    for t in cargo cmake ninja; do have "$t" || die "missing build tool: $t"; done
    say "Building daemon"
    cargo build --release --locked --manifest-path "$SRC/daemon/Cargo.toml"
    say "Building app"
    cmake -S "$SRC/app" -B "$SRC/app/build-release" -G Ninja -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$LOCAL"
    cmake --build "$SRC/app/build-release"
    stop_daemon
    cmake --install "$SRC/app/build-release"
    install -Dm755 "$SRC/daemon/target/release/$UNIT" "$LOCAL/bin/$UNIT"
    install -Dm644 "$SRC/daemon/$UNIT.service" "$HOME/.config/systemd/user/$UNIT.service"
    install -Dm644 "$LOCAL/share/applications/onedrive-gui.desktop" "$HOME/.config/autostart/onedrive-gui.desktop"
    warn "Dolphin sync marks need the system package (Dolphin only loads system plugins); skipped."
    case ":$PATH:" in *":$LOCAL/bin:"*) ;; *) warn "$LOCAL/bin is not in PATH" ;; esac
}

start_daemon() {
    [ "$START" = 1 ] || return 0
    systemctl --user daemon-reload
    systemctl --user enable --now "$UNIT"
    if systemctl --user is-active --quiet "$UNIT"; then say "Daemon running"; else warn "Daemon failed: journalctl --user -u $UNIT"; fi
    if [ -n "${WAYLAND_DISPLAY:-}${DISPLAY:-}" ] && ! pgrep -x onedrive-gui >/dev/null; then
        (setsid onedrive-gui >/dev/null 2>&1 &) || true
    fi
}

uninstall() {
    say "Uninstalling"
    stop_daemon
    pkg_installed && sudo pacman -Rns onedrive-gui
    remove_local
    systemctl --user daemon-reload 2>/dev/null || true
    say "Done. Account data in ~/.config/onedrive and your synced files were not touched."
}

if [ "$ACTION" = uninstall ]; then uninstall; exit 0; fi

check_onedrive
if [ "$MODE" = auto ] && is_arch; then install_arch; else install_user; fi
start_daemon
say "Installed. Open the tray app's Settings (onedrive-gui --settings) to add an account."
