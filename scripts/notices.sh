#!/bin/sh
# Writes NOTICES.md, the license texts for everything in a cinders binary. With --check, it
# writes nothing and fails if NOTICES.md is out of date. Run it inside the dev shell, which
# provides the tools it needs.
set -eu
case ${1:-} in
    "" | --check) ;;
    *)
        echo "usage: scripts/notices.sh [--check]" >&2
        exit 2
        ;;
esac
cd "$(dirname "$0")/.."

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
# cargo-about lists the crates and their license texts as JSON. about.toml configures it.
tree=$scratch/tree.json
cargo about generate --locked --fail --format json --output-file "$tree"

# Prints the source directory of the crate with the given name. Fails unless exactly one
# crate has that name.
crate() {
    jq -r --arg name "$1" '
        [.crates[].package | select(.name == $name).manifest_path | rtrimstr("/Cargo.toml")]
        | if length == 1 then .[0] else error("\(length) crates are named \($name)") end
    ' "$tree"
}
# Prints a file as a Markdown code block under a label. Removes carriage returns and makes
# sure the last line ends with a line feed.
text() {
    printf '`%s`:\n\n```text\n' "$1"
    awk '{ sub(/\r$/, ""); print }' "$2"
    printf '```\n\n'
}
# Prints a section for a library bundled in a crate. The arguments are the library's name,
# the crate's name, and the license files to print, as paths inside the crate.
vendored() {
    library=$1
    name=$2
    shift 2
    directory=$(crate "$name")
    printf '### %s, in the `%s` crate\n\n' "$library" "$name"
    for file in "$@"; do
        text "$file" "$directory/$file"
    done
}

# cargo-about does not read NOTICE files, so look for one in each crate's directory.
jq -r '.crates | map(.package) | sort_by(.name, .version)[]
    | "\(.name) \(.version)\t\(.manifest_path | rtrimstr("/Cargo.toml"))"' "$tree" |
    while IFS="$(printf '\t')" read -r name directory; do
        for file in "$directory"/[Nn][Oo][Tt][Ii][Cc][Ee]*; do
            [ -f "$file" ] || continue
            printf '### %s\n\n' "$name"
            text "$(basename "$file")" "$file"
        done
    done > "$scratch/files.md"

{
    cat <<'EOF'
# Third-party notices

Cinders binaries include code from the projects listed below. Cinders itself is under the
MIT license. See `LICENSE`.

## Crates

Each section shows one license text and the crates that use it, with their versions. When a
crate offers a choice of licenses, it is listed under one of them. When a crate has no
license file in its top directory, the standard text of its license is shown. The source
code of every crate is available on crates.io under the same name and version.

EOF
    # Normalize whitespace, then group crates by license text. A crate can ship two copies
    # of a license that differ only in blank lines, and cargo-about picks one by directory
    # order, which is not the same on every file system.
    jq -r '
        def tidy: gsub("\r"; "") | gsub("[ \t]+\n"; "\n") | gsub("\n{3,}"; "\n\n")
            | sub("\\A\n+"; "") | sub("\\s+\\z"; "");
        [.licenses[] | {id, name, text: (.text | tidy)}
            + {crates: [.used_by[].crate | "\(.name) \(.version)"]}]
        | group_by([.id, .text])
        | map(.[0] + {crates: (map(.crates) | add | unique)})
        | sort_by(.id, .crates)[]
        | "### \(.name)\n", (.crates[] | "- \(.)"), "\n```text\n\(.text)\n```\n"' "$tree"
    if [ -s "$scratch/files.md" ]; then
        printf '## Notice files\n\n'
        printf 'These crates include a NOTICE file. Each is reproduced in full.\n\n'
        cat "$scratch/files.md"
    fi
    cat <<'EOF'
## Libraries inside crates

These crates bundle the source of a C or C++ library and compile it into cinders. Each file
below is copied from the crate and labeled with its path inside the crate.

EOF
    # libaom's third_party licenses are for the code that its build compiles in.
    vendored libaom libaom-sys vendor/LICENSE vendor/PATENTS \
        vendor/third_party/fastfeat/LICENSE vendor/third_party/libyuv/LICENSE \
        vendor/third_party/vector/LICENSE vendor/third_party/x86inc/LICENSE
    vendored libavif libavif-sys libavif/LICENSE
    vendored mimalloc libmimalloc-sys c_src/mimalloc/v3/LICENSE
    vendored woff2 woofwoof vendor/woff2/LICENSE
    cat <<'EOF'
## The toolchain

The compiler and linker add this code to the binaries they build. It comes with the
toolchain, not from a crate.

### The Rust standard library

Included in every binary. It is available under the MIT license or the Apache License 2.0,
and the MIT text is below. The standard library has dependencies of its own. Each Rust
release lists their licenses in the `share/doc/rust/COPYRIGHT-library.html` file of its
toolchain.

EOF
    # The dev shell has no copy of the toolchain's license texts, so scripts/notices/ keeps
    # one from the Rust and musl sources. Compare them again when the toolchain is upgraded.
    text LICENSE-MIT scripts/notices/rust/LICENSE-MIT
    cat <<'EOF'
### musl

Included in the Linux binaries, which link the musl C library statically.

EOF
    text COPYRIGHT scripts/notices/musl/COPYRIGHT
    cat <<'EOF'
### The C++ runtime

Included in the Linux binaries: libstdc++ and libgcc, from GCC. They are under the GNU
General Public License version 3 with the GCC Runtime Library Exception. The exception
allows a program that links them to be distributed under its own terms.
EOF
} > "$scratch/all.md"

if [ -z "${1:-}" ]; then
    cp "$scratch/all.md" NOTICES.md
elif cmp -s "$scratch/all.md" NOTICES.md; then
    echo "NOTICES.md is current"
else
    echo "NOTICES.md is out of date. Run scripts/notices.sh and commit the result." >&2
    exit 1
fi
