#!/bin/sh
#
# Remove CalenPomo, the `calpo` command, and everything either of them left on
# this machine.
#
#   curl -fsSL https://raw.githubusercontent.com/hinokinokenkyushitsu/CalenPomo/main/uninstall.sh | sh -s -- --yes
#
# Options:
#   --purge         also delete the vault: your calendar files and pomodoro
#                   records. Asked for separately, and never without printing
#                   which directory is about to go.
#   -y, --yes       do not ask. Required when there is no terminal to ask at,
#                   which is exactly the case for `curl ... | sh`.
#   -n, --dry-run   list what would go and remove nothing.
#
# Environment:
#   CALENPOMO_BIN_DIR   where `calpo` was put, instead of ~/.local/bin
#   CALENPOMO_APP_DIR   where the macOS app was put, instead of /Applications
#
# POSIX sh, and nothing but definitions until the last line, for the same reason
# install.sh is: a download cut off halfway through should leave a pile of
# functions that never ran, not half an uninstall.

set -eu

IDENT=com.hinoki.calenpomo
PRODUCT=CalenPomo
# macOS keys an installed app's leftovers on the bundle identifier, but a binary
# run straight out of a build directory leaves the same things under its
# executable name. Both are this app; sweeping one name leaves the other behind.
EXENAME=calenpomo

NL='
'

PURGE=0
ASSUME_YES=0
DRY=0
TARGETS=

say() {
    printf '%s\n' "$*"
}

die() {
    printf 'uninstall: %s\n' "$*" >&2
    exit 1
}

usage() {
    say 'usage: uninstall.sh [--purge] [-y|--yes] [-n|--dry-run]'
    say ''
    say '  --purge      also delete the vault (calendar files and session records)'
    say '  -y, --yes    do not ask; required when stdin is not a terminal'
    say '  -n, --dry-run   list what would go, remove nothing'
}

# Paths are collected first and removed later, so that the list can be shown
# before anything happens to it. Anything that is not there is not mentioned:
# a machine that only ever had the CLI should not be read a list of misses.
add() {
    [ -n "${1:-}" ] || return 0
    [ -e "$1" ] || [ -L "$1" ] || return 0
    case "$NL$TARGETS" in
        *"$NL$1$NL"*) return 0 ;;
    esac
    TARGETS="$TARGETS$1$NL"
}

collect_macos() {
    # install.sh falls back to ~/Applications when /Applications is not
    # writable, so both are candidates whichever one this machine used.
    add "${CALENPOMO_APP_DIR:-/Applications}/$PRODUCT.app"
    add "/Applications/$PRODUCT.app"
    add "$HOME/Applications/$PRODUCT.app"

    add "$HOME/Library/LaunchAgents/$PRODUCT.plist"

    for name in "$IDENT" "$EXENAME"; do
        add "$HOME/Library/Caches/$name"
        add "$HOME/Library/WebKit/$name"
        add "$HOME/Library/HTTPStorages/$name"
        add "$HOME/Library/HTTPStorages/$name.binarycookies"
        add "$HOME/Library/Preferences/$name.plist"
        add "$HOME/Library/Saved Application State/$name.savedState"
    done
}

collect_linux() {
    add "$BIN_DIR/$PRODUCT.AppImage"
    add "$HOME/.local/share/applications/calenpomo.desktop"
    add "$HOME/.local/share/icons/hicolor/128x128/apps/calenpomo.png"
    add "$HOME/.config/autostart/$PRODUCT.desktop"

    # webkit2gtk's own storage, which the app never writes to directly.
    add "${XDG_DATA_HOME:-$HOME/.local/share}/$IDENT"
    add "${XDG_CACHE_HOME:-$HOME/.cache}/$IDENT"
}

# settings.toml, timer.json and the CLI socket. Read the vault path out of it
# before this goes: afterwards there is nothing left that knows where the vault
# was.
collect_config() {
    add "$CONFIG_DIR"
    add "$BIN_DIR/calpo"
}

vault_path() {
    conf=$CONFIG_DIR/settings.toml
    [ -f "$conf" ] || return 0
    sed -n 's/^[[:space:]]*vault_path[[:space:]]*=[[:space:]]*"\(.*\)"[[:space:]]*$/\1/p' \
        "$conf" | head -1
}

# The bundle's executable is `calenpomo`, not the product name -- Tauri names it
# after the Cargo bin, and `default-run` makes that the lowercase one -- so on no
# platform is there a process called CalenPomo to look for. On Linux the AppImage
# runtime carries its own file name in addition.
app_running() {
    if pgrep -x "$EXENAME" > /dev/null 2>&1; then
        return 0
    fi
    if [ "$PLATFORM" = linux ] && pgrep -f "$PRODUCT.AppImage" > /dev/null 2>&1; then
        return 0
    fi
    return 1
}

# The tray process rewrites timer.json every ten seconds and holds the socket
# open, so a file deleted out from under a running app comes straight back.
quit_app() {
    app_running || return 0
    say '  asking CalenPomo to quit'

    if [ "$PLATFORM" = macos ]; then
        # AppleScript addresses the bundle, which *is* named CalenPomo. Sent only
        # to something already running: `quit app` on an app that is not would
        # launch it first.
        osascript -e "quit app \"$PRODUCT\"" > /dev/null 2>&1 || true
    fi

    n=0
    while app_running; do
        if [ "$n" -ge 10 ]; then
            die "CalenPomo is still running; quit it and try again"
        fi
        # Three seconds of asking nicely, then insist.
        if [ "$n" -ge 3 ]; then
            pkill -x "$EXENAME" > /dev/null 2>&1 || true
            if [ "$PLATFORM" = linux ]; then
                pkill -f "$PRODUCT.AppImage" > /dev/null 2>&1 || true
            fi
        fi
        sleep 1
        n=$((n + 1))
    done
}

# Deleting the plist is not enough: launchd holds the job until it is told to
# let go, and would go on trying to start a binary that is no longer there.
stop_agent() {
    [ "$PLATFORM" = macos ] || return 0
    [ -f "$HOME/Library/LaunchAgents/$PRODUCT.plist" ] || return 0

    launchctl bootout "gui/$(id -u)/$PRODUCT" > /dev/null 2>&1 \
        || launchctl unload -w "$HOME/Library/LaunchAgents/$PRODUCT.plist" > /dev/null 2>&1 \
        || true
}

confirm() {
    if [ "$ASSUME_YES" = 1 ]; then
        return 0
    fi
    # stdin is the script itself under `curl | sh`, so the question has to be
    # asked at the terminal or not at all. Not at all means stopping, never
    # taking silence for a yes. /dev/tty is there and readable even when no
    # terminal is attached to this process -- opening it is the only real test.
    # In a subshell, and not with `:`: a failed redirection onto a special
    # built-in is required to exit the shell outright, which under dash made
    # this leave with status 2 and never say why.
    if ! (true < /dev/tty) 2> /dev/null; then
        die "no terminal to ask at; pass --yes if this is what you meant"
    fi

    printf '%s [y/N] ' "$1" > /dev/tty
    read -r reply < /dev/tty || reply=
    case "$reply" in
        y | Y | yes | YES) return 0 ;;
    esac
    return 1
}

remove_targets() {
    printf '%s' "$TARGETS" | while IFS= read -r path; do
        [ -n "$path" ] || continue
        rm -rf "$path" || die "could not remove $path"
        say "  removed $path"
    done
}

refresh_menu() {
    [ "$PLATFORM" = linux ] || return 0
    command -v update-desktop-database > /dev/null 2>&1 || return 0
    update-desktop-database "$HOME/.local/share/applications" > /dev/null 2>&1 || true
}

# install.sh only ever printed the line to add; it never edited a profile, so
# neither does this. Saying which file still has it is the whole of the help
# that can be given without touching something we did not write.
path_note() {
    found=
    for f in "$HOME/.profile" "$HOME/.bash_profile" "$HOME/.bashrc" \
        "$HOME/.zshrc" "$HOME/.zprofile" "$HOME/.config/fish/config.fish"; do
        [ -f "$f" ] || continue
        if grep -qF "$BIN_DIR" "$f" 2> /dev/null; then
            found="$found  $f$NL"
        fi
    done
    [ -n "$found" ] || return 0

    say ''
    say "These still put $BIN_DIR on your PATH:"
    printf '%s' "$found"
    say 'install.sh never wrote to them, so this does not either. Edit them'
    say 'yourself if that line was there only for calpo.'
}

vault_note() {
    [ -n "$VAULT" ] || return 0
    say ''
    if [ -d "$VAULT" ]; then
        say "Your calendar files and pomodoro records were left alone, in:"
        say ''
        say "  $VAULT"
        say ''
        say 'They are plain .ics and .jsonl files and are yours. To delete them too:'
        say ''
        say "  rm -rf \"$VAULT\""
    else
        say "settings.toml pointed at $VAULT, which is not there."
    fi
}

purge_vault() {
    if [ -z "$VAULT" ]; then
        say ''
        say 'No vault path was recorded, so --purge has nothing to delete.'
        return 0
    fi
    if [ ! -d "$VAULT" ]; then
        say ''
        say "--purge: $VAULT is not there."
        return 0
    fi

    say ''
    say '--purge will delete your calendar files and pomodoro records:'
    say ''
    say "  $VAULT"
    say ''

    if [ "$DRY" = 1 ]; then
        say "  would remove $VAULT"
        return 0
    fi

    # Its own question, after its own path. Agreeing to uninstall a program is
    # not agreeing to throw away the documents it was used to write.
    if ! confirm "Delete it?"; then
        say 'The vault was left alone.'
        return 0
    fi
    rm -rf "$VAULT" || die "could not remove $VAULT"
    say "  removed $VAULT"
}

main() {
    while [ $# -gt 0 ]; do
        case $1 in
            --purge) PURGE=1 ;;
            -y | --yes) ASSUME_YES=1 ;;
            -n | --dry-run) DRY=1 ;;
            -h | --help)
                usage
                return 0
                ;;
            *) die "unknown option $1" ;;
        esac
        shift
    done

    case "$(uname -s)" in
        Darwin) PLATFORM=macos ;;
        Linux) PLATFORM=linux ;;
        *) die "$(uname -s) is not one of the systems this installs on" ;;
    esac

    BIN_DIR=${CALENPOMO_BIN_DIR:-$HOME/.local/bin}
    if [ "$PLATFORM" = macos ]; then
        CONFIG_DIR="$HOME/Library/Application Support/$IDENT"
    else
        CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/$IDENT"
    fi

    VAULT=$(vault_path)

    if [ "$PLATFORM" = macos ]; then
        collect_macos
    else
        collect_linux
    fi
    collect_config

    if [ -z "$TARGETS" ]; then
        say 'CalenPomo is not installed here; nothing to remove.'
        [ "$PURGE" = 0 ] || purge_vault
        return 0
    fi

    if [ "$DRY" = 1 ]; then
        say 'These would be removed:'
    else
        say 'These will be removed:'
    fi
    printf '%s' "$TARGETS" | sed 's/^/  /'

    if [ "$DRY" = 1 ]; then
        [ "$PURGE" = 0 ] || purge_vault
        return 0
    fi

    confirm 'Remove them?' || die 'nothing was removed'

    quit_app
    stop_agent
    remove_targets
    refresh_menu

    if [ "$PURGE" = 1 ]; then
        purge_vault
    else
        vault_note
    fi
    path_note

    say ''
    say 'Done.'
}

main "$@"
