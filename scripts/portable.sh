#!/bin/sh
# Checks that a cinders binary runs anywhere: static on Linux, and on macOS linked only to
# system libraries. The file format decides which check runs.
set -eu
bin=${1:?usage: scripts/portable.sh BINARY}

# Each tool's output is captured first, so a missing or failing tool fails the check.
if [ "$(head -c 4 "$bin" | od -An -c | tr -d ' ')" = '177ELF' ]; then
    headers=$(readelf -lW "$bin")
    if echo "$headers" | grep -Eq 'INTERP|DYNAMIC' || ! echo "$headers" | grep -q 'LOAD'; then
        echo "$bin is not static: it has an interpreter or a dynamic section" >&2
        exit 1
    fi
    echo "$bin: static"
else
    libraries=$(otool -L "$bin")
    foreign=$(echo "$libraries" | tail -n +2 | awk '{ print $1 }' |
        grep -Ev '^(/usr/lib/|/System/Library/)' || true)
    if ! echo "$libraries" | grep -q '/usr/lib/libSystem'; then
        echo "$bin: otool listed no system library, so the check cannot be trusted" >&2
        exit 1
    fi
    if [ -n "$foreign" ]; then
        echo "$bin links libraries outside /usr/lib and /System/Library:" >&2
        echo "$foreign" >&2
        exit 1
    fi
    echo "$bin: system libraries only"
fi
