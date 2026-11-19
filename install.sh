#!/bin/sh
# Installs cinders to ~/.local/bin. Downloads the release archive for this machine and
# checks it against the release's checksums before unpacking it.
#
#   curl -fsSL https://raw.githubusercontent.com/reforged-dev/cinders/main/install.sh | sh
#
# Set CINDERS_VERSION to install a specific release, for example 0.1.0. The default is the
# latest release.
# Set CINDERS_BASE_URL to download from a mirror instead of GitHub.
set -eu

# Everything runs from main, which is called on the last line. If the download of this
# script is cut short, the shell runs none of it.
main() {
    base=${CINDERS_BASE_URL:-https://github.com/reforged-dev/cinders/releases}
    version=${CINDERS_VERSION:-latest}
    bin=$HOME/.local/bin

    system="$(uname -s) $(uname -m)"
    case $system in
        "Darwin arm64") target=aarch64-apple-darwin ;;
        "Linux x86_64") target=x86_64-unknown-linux-musl ;;
        "Linux aarch64" | "Linux arm64") target=aarch64-unknown-linux-musl ;;
        *)
            echo "cinders has no build for $system" >&2
            exit 1
            ;;
    esac
    if [ "$version" = latest ]; then
        release=$base/latest/download
    else
        release=$base/download/v${version#v}
    fi
    name=cinders-$target

    scratch=$(mktemp -d)
    trap 'rm -rf "$scratch"' EXIT
    cd "$scratch"
    echo "downloading $release/$name.tar.gz"
    curl -fsSL -o "$name.tar.gz" "$release/$name.tar.gz"
    curl -fsSL -o SHA256SUMS "$release/SHA256SUMS"

    expected=$(awk -v file="$name.tar.gz" '$2 == file { print $1 }' SHA256SUMS)
    if command -v sha256sum > /dev/null; then
        actual=$(sha256sum "$name.tar.gz")
    else
        actual=$(shasum -a 256 "$name.tar.gz")
    fi
    if [ -z "$expected" ] || [ "${actual%% *}" != "$expected" ]; then
        echo "the checksum of $name.tar.gz does not match SHA256SUMS" >&2
        exit 1
    fi

    tar -xzf "$name.tar.gz"
    mkdir -p "$bin"
    # Copy to a temporary name, then rename. Writing straight over the old binary could
    # stop a cinders that is running.
    cp "$name/cinders" "$bin/cinders.new"
    mv -f "$bin/cinders.new" "$bin/cinders"
    echo "installed $("$bin/cinders" --version) in $bin"
    case :$PATH: in
        *":$bin:"*) ;;
        *) echo "$bin is not on your PATH. Add it to run cinders by name." ;;
    esac
}
main
