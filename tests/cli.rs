//! Runs the `cinders` binary and checks its arguments and what it prints.
#![expect(clippy::unwrap_used, clippy::panic, reason = "test code may unwrap and panic")]

mod common;

use std::process::{Command, Output};

/// Runs `cinders` with `args`.
fn cinders(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cinders")).args(args).output().unwrap()
}

#[test]
fn rejects_arguments_it_does_not_know() {
    let rejected: [&[&str]; 10] = [
        &["dev", "--port"],
        &["dev", "--port", "x"],
        &["dev", "--port", "65536"],
        &["dev", "a", "b"],
        &["build", "--optimized"],
        &["build", "--host"],
        &["build", "a", "b"],
        &["build", "--help"],
        &["serve"],
        &[],
    ];
    for args in rejected {
        let output = cinders(args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        let usage = String::from_utf8(output.stderr).unwrap();
        assert!(usage.starts_with("usage: cinders build [DIR]\n"), "{usage}");
        assert!(usage.contains("cinders dev [DIR] [--optimized] [--port N] [--host]"), "{usage}");
        assert!(usage.contains("cinders --help"), "{usage}");
    }
}

/// What a run of `cinders` printed.
struct Printed {
    out: String,
    err: String,
}

/// Runs `cinders` with `args`, which it has to accept.
fn run(args: &[&str]) -> Printed {
    let output = cinders(args);
    assert!(output.status.success(), "{args:?}");
    let out = String::from_utf8(output.stdout).unwrap();
    Printed { out, err: String::from_utf8(output.stderr).unwrap() }
}

#[test]
fn prints_help_a_version_and_a_summary_of_a_build() {
    for flag in ["--help", "-h"] {
        let help = run(&[flag]).out;
        assert!(
            help.contains("usage: cinders build [DIR]") && help.contains("  --host  "),
            "{help}"
        );
    }
    for flag in ["--version", "-V"] {
        assert_eq!(run(&[flag]).out, format!("cinders {}\n", env!("CARGO_PKG_VERSION")));
    }

    let stale = ".cinders/images/stale.avif";
    let files = [
        ("cinders.toml", "[navigation]\nclient = false\n"),
        ("pages/index.html", "home"),
        ("pages/about.html", "about"),
        ("public/a.txt", "a"),
        (stale, "an encode of an image the site no longer has"),
    ];
    let root = common::site("summary", &files);
    let built = run(&["build", root.to_str().unwrap()]);
    let expected =
        format!("built 2 pages and 1 other file in {}, 10 bytes in ", root.join("dist").display());
    assert!(built.out.is_empty(), "the summary goes to standard error: {}", built.out);
    assert!(built.err.starts_with(&expected) && built.err.ends_with(" ms\n"), "{}", built.err);
    assert!(!root.join(stale).exists(), "a build prunes the store");

    let files = [("pages/index.html", r#"<a href="/x">x</a>"#), (stale, "kept")];
    let broken = common::site("summary-broken", &files);
    let output = cinders(&["build", broken.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1), "a failed build");
    let error = String::from_utf8(output.stderr).unwrap();
    let expected = "error: pages/index.html: <a href>: `/x` is not a route or a file in public/\n";
    assert_eq!(error, expected);
    assert!(!broken.join("dist").exists(), "a failed build writes nothing");
    assert!(broken.join(stale).exists(), "and prunes nothing");
}

#[test]
fn build_prints_notes_for_unmarked_scripts() {
    let unmarked = [
        (
            "pages/index.html",
            r#"<script type="module" src="../scripts/app.ts"></script><p>home</p>"#,
        ),
        ("pages/about.html", "<p>about</p>"),
        ("scripts/app.ts", "console.log('hi');"),
    ];
    let root = common::site("build-note-unmarked", &unmarked);
    let built = run(&["build", root.to_str().unwrap()]);
    assert!(built.out.is_empty(), "notes and the summary go to standard error: {}", built.out);
    let lines: Vec<&str> = built.err.lines().collect();
    assert_eq!(lines.len(), 2, "expected one note line and the summary line: {}", built.err);
    assert_eq!(
        lines[0],
        "note: pages with scripts/app.ts will use full page loads \
         (add data-client-nav to opt in, or set [navigation] client = false)",
        "the note names the unmarked script and both remedies"
    );
    assert!(
        lines[1].starts_with(&format!(
            "built 2 pages and 2 other files in {}, ",
            root.join("dist").display()
        )) && lines[1].ends_with(" ms"),
        "a build ends with its summary line: {}",
        lines[1]
    );

    let marked = [
        (
            "pages/index.html",
            r#"<script type="module" src="../scripts/app.ts" data-client-nav></script><p>home</p>"#,
        ),
        ("pages/about.html", "<p>about</p>"),
        ("scripts/app.ts", "console.log('hi');"),
    ];
    let clean = run(&["build", common::site("build-note-marked", &marked).to_str().unwrap()]);
    assert!(clean.out.is_empty(), "{}", clean.out);
    assert!(
        !clean.err.contains("note:"),
        "a site with no unmarked script does not print a note: {}",
        clean.err
    );
    assert_eq!(clean.err.lines().count(), 1, "only the summary line prints: {}", clean.err);
}
