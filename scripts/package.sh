#!/bin/sh
# Builds the release archive for this machine and writes it to the given directory. The
# binary is checked before it is packed.
set -eu
out=${1:?usage: scripts/package.sh DIRECTORY}
mkdir -p "$out"
out=$(cd "$out" && pwd)
cd "$(dirname "$0")/.."

target=$(nix eval --raw .#default.stdenv.hostPlatform.rust.rustcTarget)
bin=$(nix build --no-link --print-out-paths)/bin/cinders
scripts/portable.sh "$bin"

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
# Building the test fixture from an empty store runs every encoder on this platform.
mkdir "$scratch/site"
cp -R tests/fixtures/site/. "$scratch/site"
rm -rf "$scratch/site/.cinders" "$scratch/site/dist"
"$bin" build "$scratch/site"

name=cinders-$target
mkdir "$scratch/$name"
cp "$bin" LICENSE NOTICES.md "$scratch/$name"
# The binary is read-only in the Nix store, so make the copy writable by its owner.
chmod 755 "$scratch/$name/cinders"
tar -czf "$out/$name.tar.gz" -C "$scratch" "$name"
echo "packed $out/$name.tar.gz"
