#!/usr/bin/env bash
# The version number lives in four places -- the git tag, Cargo.toml,
# package.json and tauri.conf.json -- and nothing keeps them in step. The tag is
# what a download URL is built from; the other three end up in the names of the
# files that download produces. A release cut from a tag that disagrees with
# them is not visibly wrong anywhere until someone reports that v0.2.0 put 0.1.0
# on their machine.
#
# Run it before tagging:  .github/scripts/check-versions.sh v0.2.0
set -eu

tag=${1:?usage: check-versions.sh <tag>}
want=${tag#v}

root=$(cd -- "$(dirname -- "$0")/../.." && pwd)

# The `[package]` version is the first line-anchored `version = ` in the file;
# every dependency's own version sits inside a `name = { ... }` table.
cargo=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/src-tauri/Cargo.toml" | head -1)
# Two spaces of indent means a top-level key in both of these files.
npm=$(sed -n 's/^  "version": "\([^"]*\)".*/\1/p' "$root/package.json" | head -1)
tauri=$(sed -n 's/^  "version": "\([^"]*\)".*/\1/p' "$root/src-tauri/tauri.conf.json" | head -1)

printf 'tag              %s\n' "$want"
printf 'Cargo.toml       %s\n' "$cargo"
printf 'package.json     %s\n' "$npm"
printf 'tauri.conf.json  %s\n' "$tauri"

status=0
for pair in "Cargo.toml:$cargo" "package.json:$npm" "tauri.conf.json:$tauri"; do
    name=${pair%%:*}
    got=${pair#*:}
    if [ -z "$got" ]; then
        printf '\n%s: no version found -- the file has changed shape\n' "$name" >&2
        status=1
    elif [ "$got" != "$want" ]; then
        printf '\n%s says %s, the tag says %s\n' "$name" "$got" "$want" >&2
        status=1
    fi
done

if [ "$status" -ne 0 ]; then
    exit 1
fi
printf '\nall four agree on %s\n' "$want"

# What names the release, when this runs in the workflow.
if [ -n "${GITHUB_OUTPUT:-}" ]; then
    printf 'version=%s\n' "$want" >> "$GITHUB_OUTPUT"
fi
