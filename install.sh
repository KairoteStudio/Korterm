#!/usr/bin/env bash
# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.
#
# Korterm installer — binary, icons and desktop entry into a user prefix.
#
#   ./install.sh              install to ~/.local
#   ./install.sh /usr/local   install to another prefix
#   ./install.sh --uninstall  remove from ~/.local
#
set -euo pipefail

cd "$(dirname "$0")"

SIZES=(16 32 48 64 128 256 512)

do_install() {
    local prefix="${1:-$HOME/.local}"
    local bin="target/release/korterm"

    if [[ ! -x "$bin" ]]; then
        echo "==> release binary missing — building (this can take a while)…"
        cargo build --release
    fi

    echo "==> installing to $prefix"
    # `command install` — the coreutils binary, NOT a recursive call to
    # this function (a bare `install` here would resolve to the function
    # itself and recurse forever).
    command install -Dm755 "$bin" "$prefix/bin/korterm"
    for s in "${SIZES[@]}"; do
        command install -Dm644 "assets/icon-$s.png" \
            "$prefix/share/icons/hicolor/${s}x${s}/apps/korterm.png"
    done
    command install -Dm644 assets/korterm.desktop \
        "$prefix/share/applications/korterm.desktop"

    # Best-effort cache refresh (new icons/menu entries without relogin).
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database "$prefix/share/applications" 2>/dev/null || true
    fi
    if command -v gtk-update-icon-cache >/dev/null 2>&1; then
        gtk-update-icon-cache -f -t "$prefix/share/icons/hicolor" 2>/dev/null || true
    fi

    echo "==> done. Make sure '$prefix/bin' is on your PATH."
    echo "    Bind a system shortcut to 'korterm --quick' for the Quake terminal."
}

uninstall() {
    local prefix="${1:-$HOME/.local}"
    echo "==> removing Korterm from $prefix"
    rm -f "$prefix/bin/korterm" \
          "$prefix/share/applications/korterm.desktop" \
          "$prefix/share/applications/korterm-quick.desktop"
    for s in "${SIZES[@]}"; do
        rm -f "$prefix/share/icons/hicolor/${s}x${s}/apps/korterm.png"
    done
    echo "==> done."
}

case "${1:-}" in
    --uninstall) uninstall "${2:-}" ;;
    *)           do_install "$1" ;;
esac
