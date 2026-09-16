#!/bin/sh
#
# Install WabiCalendar and its `wabi` command line tool.
#
#   curl -fsSL https://raw.githubusercontent.com/hinokinokenkyushitsu/WabiCalendar/main/install.sh | sh
#
# Environment:
#   WABICALENDAR_VERSION   a tag such as v0.1.0, instead of the latest release
#   WABICALENDAR_BIN_DIR   where `wabi` goes, instead of ~/.local/bin
#   WABICALENDAR_APP_DIR   where the macOS app goes, instead of /Applications
#
# Written for POSIX sh so that `| sh` is honest, and wrapped in a function that
# is only called on the very last line: a download cut off halfway through
# defines some functions and runs none of them, rather than executing half an
# installation.

set -eu

REPO=hinokinokenkyushitsu/WabiCalendar
API=https://api.github.com/repos/$REPO/releases

say() {
    printf '%s\n' "$*"
}

die() {
    printf 'install: %s\n' "$*" >&2
    exit 1
}

need() {
    command -v "$1" > /dev/null 2>&1 || die "this needs $1, which is not on PATH"
}

# The checksum tool is spelled differently on the two platforms and neither has
# the other: sha256sum is coreutils, shasum ships with macOS's perl.
sha_check() {
    if command -v sha256sum > /dev/null 2>&1; then
        sha256sum -c "$1" > /dev/null
    else
        shasum -a 256 -c "$1" > /dev/null
    fi
}

# Every asset's download URL, one per line. Parsed with sed rather than jq
# because jq is not something an installer may assume.
release_assets() {
    if [ -n "${WABICALENDAR_VERSION:-}" ]; then
        url=$API/tags/$WABICALENDAR_VERSION
    else
        url=$API/latest
    fi

    body=$(curl -fsSL "$url" 2> /dev/null) || die "no release to install from at $url"
    # The checksums and signatures are dropped here rather than filtered at
    # every use: `grab` builds a checksum URL out of its subject's, and
    # wabi-linux-x86_64.sha256 matches every pattern wabi-linux-x86_64 does.
    printf '%s' "$body" \
        | tr ',' '\n' \
        | sed -n 's/.*"browser_download_url": *"\([^"]*\)".*/\1/p' \
        | { grep -v -e '\.sha256$' -e '\.sig$' || true; }
}

# The first asset whose name ends the way we asked. Matching on the name rather
# than building one keeps this working when a version number moves.
pick() {
    pattern=$1
    printf '%s\n' "$ASSETS" | while read -r url; do
        case "${url##*/}" in
            $pattern)
                printf '%s\n' "$url"
                break
                ;;
        esac
    done
}

# Download into $WORK under the asset's own name, then check it against the
# .sha256 published beside it. The name matters: the checksum file records it.
grab() {
    url=$1
    name=${url##*/}

    curl -fsSL -o "$WORK/$name" "$url" || die "could not download $name"
    curl -fsSL -o "$WORK/$name.sha256" "$url.sha256" \
        || die "$name has no published checksum; refusing to install it"

    (cd "$WORK" && sha_check "$name.sha256") \
        || die "$name does not match its published checksum"
    say "  verified $name"
}

install_cli() {
    url=$(pick "wabi-$PLATFORM-*")
    [ -n "$url" ] || die "this release has no wabi build for $PLATFORM"
    grab "$url"

    mkdir -p "$BIN_DIR"
    cp "$WORK/${url##*/}" "$BIN_DIR/wabi"
    chmod 755 "$BIN_DIR/wabi"
    say "  wabi -> $BIN_DIR/wabi"
}

install_macos() {
    url=$(pick '*.app.tar.gz')
    [ -n "$url" ] || die "this release has no macOS app"
    grab "$url"

    tar -xzf "$WORK/${url##*/}" -C "$WORK"
    app=$(ls -d "$WORK"/*.app 2> /dev/null | head -1) || app=
    [ -n "$app" ] || die "the downloaded archive holds no .app"

    # /Applications is group-writable by admins, so this usually needs no sudo.
    dest=${WABICALENDAR_APP_DIR:-/Applications}
    [ -w "$dest" ] || dest=$HOME/Applications
    mkdir -p "$dest"

    # Replacing rather than merging: a bundle with leftovers from an older
    # version in it is a worse outcome than a clean reinstall.
    rm -rf "$dest/$(basename "$app")"
    cp -R "$app" "$dest/"

    # curl does not set the quarantine flag -- only the APIs browsers use do --
    # so this is for the case where the archive arrived some other way. The app
    # is not notarised; with the flag set, Gatekeeper would refuse it outright.
    xattr -dr com.apple.quarantine "$dest/$(basename "$app")" 2> /dev/null || true

    say "  $(basename "$app") -> $dest"
}

install_linux() {
    url=$(pick '*.AppImage')
    [ -n "$url" ] || die "this release has no Linux AppImage"
    grab "$url"

    mkdir -p "$BIN_DIR"
    cp "$WORK/${url##*/}" "$BIN_DIR/WabiCalendar.AppImage"
    chmod 755 "$BIN_DIR/WabiCalendar.AppImage"
    say "  WabiCalendar.AppImage -> $BIN_DIR"

    # The installed copy, not the downloaded one: curl leaves what it writes
    # unexecutable, and an AppImage that cannot be run cannot be unpacked.
    desktop_entry "$(icon_from_appimage "$BIN_DIR/WabiCalendar.AppImage")"
}

# The icon out of the AppImage itself, so it always matches the build that was
# installed and costs no second download. Failure is not fatal: an entry with a
# generic icon beats no entry at all.
icon_from_appimage() {
    (cd "$WORK" && "$1" --appimage-extract \
        'usr/share/icons/hicolor/128x128/apps/*' > /dev/null 2>&1) || true

    found=$(ls "$WORK"/squashfs-root/usr/share/icons/hicolor/128x128/apps/*.png 2> /dev/null | head -1) || found=
    [ -n "$found" ] || return 0

    dir=$HOME/.local/share/icons/hicolor/128x128/apps
    mkdir -p "$dir"
    cp "$found" "$dir/wabicalendar.png"
    printf '%s' wabicalendar
}

desktop_entry() {
    icon=$1
    dir=$HOME/.local/share/applications
    mkdir -p "$dir"

    {
        say '[Desktop Entry]'
        say 'Type=Application'
        say 'Name=WabiCalendar'
        say 'Comment=Local-first calendar and pomodoro timer'
        say "Exec=$BIN_DIR/WabiCalendar.AppImage"
        [ -z "$icon" ] || say "Icon=$icon"
        say 'Terminal=false'
        say 'Categories=Office;Calendar;Utility;'
    } > "$dir/wabicalendar.desktop"

    # Menus that cache the index will not show it until this runs; menus that do
    # not have the tool do not need it.
    if command -v update-desktop-database > /dev/null 2>&1; then
        update-desktop-database "$dir" > /dev/null 2>&1 || true
    fi
    say "  desktop entry -> $dir/wabicalendar.desktop"
}

# `wabi` is no use in a directory the shell will not look in, and ~/.local/bin
# is on the default PATH of many Linux distributions and of no macOS.
path_hint() {
    case ":$PATH:" in
        *":$BIN_DIR:"*) return 0 ;;
    esac
    say ''
    say "$BIN_DIR is not on your PATH. To reach wabi, add this to your shell profile:"
    say ''
    say "  export PATH=\"$BIN_DIR:\$PATH\""
}

main() {
    need curl
    need mktemp
    need tar

    case "$(uname -s)" in
        Darwin) PLATFORM=macos ;;
        Linux) PLATFORM=linux ;;
        *) die "$(uname -s) is not one of the systems this builds for" ;;
    esac

    # macOS ships as one universal build, so only Linux has an architecture to
    # be wrong about.
    if [ "$PLATFORM" = linux ]; then
        case "$(uname -m)" in
            x86_64 | amd64) ;;
            *) die "there is no Linux build for $(uname -m) yet, only x86_64" ;;
        esac
    fi

    BIN_DIR=${WABICALENDAR_BIN_DIR:-$HOME/.local/bin}

    WORK=$(mktemp -d)
    trap 'rm -rf "$WORK"' EXIT INT TERM

    ASSETS=$(release_assets)
    [ -n "$ASSETS" ] || die "that release has no downloadable files"

    say "Installing WabiCalendar for $PLATFORM..."
    if [ "$PLATFORM" = macos ]; then
        install_macos
    else
        install_linux
    fi
    install_cli
    path_hint

    say ''
    say 'Done.'
}

main "$@"
