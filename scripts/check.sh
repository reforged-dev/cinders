#!/bin/sh
# Runs every check a change must pass. Run it inside the dev shell:
# nix develop -c scripts/check.sh
set -eu
cd "$(dirname "$0")/.."

# A failed step does not stop the rest, so one run reports everything that is wrong.
failed=""
step() {
    echo "== $1"
    name=$1
    shift
    start=$(date +%s)
    "$@" || failed="$failed
  $name"
    echo "   $(($(date +%s) - start)) s"
}

bin="target/${CARGO_BUILD_TARGET:+$CARGO_BUILD_TARGET/}dist/cinders"
step "format" cargo fmt --check
step "lints" cargo clippy --all-targets --quiet -- -D warnings
step "tests" cargo test --quiet
step "shipped binary" cargo build --profile dist --quiet
step "portability" scripts/portable.sh "$bin"
step "notices" scripts/notices.sh --check
if [ -n "$failed" ]; then
    echo "failed:$failed" >&2
    exit 1
fi
echo "all checks passed"
