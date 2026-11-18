//! Module scripts: how a script and the modules it imports join into one file, the import
//! rules, and the errors that break them. Each case is a small site in a temporary directory.
#![expect(clippy::unwrap_used, clippy::panic, reason = "test code may unwrap and panic")]

mod common;

use std::fs;
use std::path::PathBuf;

use cinders::Mode;
use common::{asset, build, build_fast, error, text};

const PAGE: (&str, &str) =
    ("pages/index.html", r#"<script type="module" src="../s/main.js"></script>"#);

/// A site whose page loads `s/main.js`, with `modules` under `s/`.
fn site(name: &str, modules: &[(&str, &str)]) -> PathBuf {
    let modules: Vec<_> = modules.iter().map(|(file, code)| (format!("s/{file}"), *code)).collect();
    let mut files = vec![PAGE];
    files.extend(modules.iter().map(|(file, code)| (file.as_str(), *code)));
    common::site(name, &files)
}

/// The script of one site in its two modes.
struct Scripts {
    /// As an optimized build ships it.
    built: String,
    /// As a fast build serves it.
    fast: String,
}

/// Builds the script of a site in both modes.
fn scripts(name: &str, modules: &[(&str, &str)]) -> Scripts {
    let root = site(name, modules);
    let built = asset(&build(&root), "js");
    Scripts { built, fast: text(&build_fast(&root), "_cinders/s/main.js") }
}

#[test]
fn modules_join_in_the_order_they_run() {
    let Scripts { built, fast } = scripts(
        "join",
        &[
            (
                "main.js",
                "import './side.js';\nimport anon, { helper, live, bump } from './a.ts';\n\
                 import Named, { helper as other } from './b.js';\nconst count = 3;\n\
                 function shadow(helper) { return helper + count; }\nbump();\n\
                 console.log(anon(), new Named(), helper(), other(), shadow(live), { helper });\n",
            ),
            ("side.js", "console.log('side');\n"),
            (
                "a.ts",
                "import type { Shape } from './missing.ts';\nimport { Kind } from './types.ts';\n\
                 let count: number = 1;\nexport function helper(): Kind { return 'a' + count; }\n\
                 export let live = 0;\nexport function bump(shape?: Shape): void { live++; }\n\
                 export default function () { return 'anon'; }\n\
                 export function unused() { return 'never used'; }\n",
            ),
            ("types.ts", "export type Kind = string;\nconsole.log('types');\n"),
            (
                "b.js",
                "const count = 2;\nexport function helper() { return 'b' + count; }\n\
                 export default class { }\n",
            ),
        ],
    );
    // A module runs after the modules it imports, and an import that names only types
    // loads nothing.
    let order = ["\"side\"", "let count$1", "function helper$1()", "const count$2 = 2", "bump();"];
    let found = order.map(|part| fast.find(part));
    assert!(found.iter().all(Option::is_some) && found.is_sorted(), "{found:?}: {fast}");
    assert!(
        !fast.contains("types") && !fast.contains("import") && !fast.contains("export"),
        "{fast}"
    );
    // A name that two modules use is renamed, and a use takes the name of what it imports.
    assert!(fast.contains("function helper$2()") && fast.contains("class $default$2"), "{fast}");
    assert!(
        fast.contains("function $default$1()") && fast.contains("const count$3 = 3;"),
        "{fast}"
    );
    assert!(fast.contains("function shadow(helper) {\n\treturn helper + count$3;"), "{fast}");
    let call = "console.log($default$1(), new $default$2(), helper$1(), helper$2(), \
                shadow(live), { helper: helper$1 });";
    assert!(fast.contains(call), "{fast}");
    // A fast build's script keeps what an optimized build drops.
    assert!(fast.contains("never used") && !built.contains("never used"), "{built}");
    assert!(built.contains("`side`") && !built.contains("import"), "{built}");
}

#[test]
fn exports_follow_their_bindings() {
    let Scripts { built, fast } = scripts(
        "exports",
        &[
            (
                "main.js",
                "import fallback, { renamed, direct, star, viaImport, 'a-b' as dashed, abc } \
                 from './barrel.js';\n\
                 console.log(fallback, renamed, direct, star, viaImport, dashed, abc);\n",
            ),
            (
                "barrel.js",
                "import { inner } from './leaf.js';\nexport { inner as viaImport };\n\
                 export { inner as renamed, direct, default } from './leaf.js';\n\
                 export * from './leaf.js';\nexport * from './other.js';\n",
            ),
            (
                "leaf.js",
                "const inner = 'inner', direct = 'direct';\nexport const star = 'star';\n\
                 export const \\u0061bc = 'escaped';\n\
                 export { inner, direct, inner as 'a-b' };\nexport default 'fallback';\n",
            ),
            ("other.js", "export const star = 'shadowed';\nexport default 'not passed on';\n"),
        ],
    );
    assert!(fast.contains("var $default$1 = \"fallback\";"), "{fast}");
    assert!(
        fast.contains("console.log($default$1, inner, direct, star$1, inner, inner, abc);"),
        "{fast}"
    );
    let call = "var e=`inner`;console.log(`fallback`,e,`direct`,`star`,e,e,`escaped`);";
    assert!(built.contains(call), "{built}");
    assert!(!built.contains("shadowed") && !built.contains("not passed on"), "{built}");
}

#[test]
fn a_default_export_takes_a_name_its_module_does_not_use() {
    let Scripts { built, fast } = scripts(
        "default-name",
        &[
            (
                "main.js",
                "import value, { own } from './bound.js';\nimport read from './global.js';\n\
                 console.log(value, own, read());\n",
            ),
            (
                "bound.js",
                "const $default = 'own';\nexport default 42;\nexport { $default as own };\n",
            ),
            ("global.js", "export default function () {\n\treturn $default;\n}\n"),
        ],
    );
    assert!(fast.contains("const $default$1 = \"own\";\nvar $default$$1 = 42;"), "{fast}");
    assert!(fast.contains("function $default$$2() {\n\treturn $default;"), "{fast}");
    assert!(fast.contains("console.log($default$$1, $default$1, $default$$2());"), "{fast}");
    assert!(built.contains("function t(){return $default}console.log(e,`own`,t())"), "{built}");
}

#[test]
fn namespaces_read_the_bindings_they_name() {
    let lib = "export const used = 'used';\nexport let count = 0;\n\
               export function bump() { count++; }\n\
               export function unused() { return 'never read'; }\n";
    // A namespace that is only read by member needs no object, so a build drops the rest.
    let Scripts { built, fast } = scripts(
        "namespace-members",
        &[
            (
                "main.js",
                "import * as lib from './lib.js';\nlib.bump();\n\
                 function shadow(used) { return lib.used + used; }\n\
                 console.log(shadow('x'), lib.count, lib.missing, typeof lib.missing?.deep);\n",
            ),
            ("lib.js", lib),
        ],
    );
    assert!(fast.contains("bump();\nfunction shadow(used) {\n\treturn used$1 + used;"), "{fast}");
    assert!(
        fast.contains("console.log(shadow(\"x\"), count, void 0, typeof (void 0)?.deep);"),
        "{fast}"
    );
    assert!(!fast.contains("__proto__") && !built.contains("never read"), "{built}");

    // A namespace that is used as a value is an object of getters, made once for its module.
    let Scripts { built, fast } = scripts(
        "namespace-values",
        &[
            (
                "main.js",
                "import * as lib from './lib.js';\nimport { again, other } from './barrel.js';\n\
                 delete lib.used;\nlib.count++;\n\
                 console.log(Object.keys(lib), lib[name], again === lib, other, lib.used);\n",
            ),
            ("lib.js", lib),
            (
                "barrel.js",
                "export * as again from './lib.js';\nimport * as other from './lib.js';\n\
                 export { other };\n",
            ),
        ],
    );
    let object = "var $default = {\n\t__proto__: null,\n\tget \"bump\"() {\n\t\treturn bump;";
    assert!(fast.contains(object) && fast.matches("__proto__").count() == 1, "{fast}");
    assert!(fast.contains("delete $default.used;\n$default.count++;"), "{fast}");
    let call = "console.log(Object.keys($default), $default[name], $default === $default, \
                $default, used);";
    assert!(fast.contains(call), "{fast}");
    assert!(built.contains("get unused(){") && built.contains("never read"), "{built}");
}

#[test]
fn larger_shapes_keep_their_meaning() {
    let Scripts { built, fast } = scripts(
        "shapes",
        &[
            (
                "main.js",
                "import { Square, config, top } from './vendor.js';\n\
                 const square = new Square();\nsquare.sides = 5;\n\
                 console.log(square.area, Square.kind, config.mode, top, this);\n",
            ),
            (
                "vendor.js",
                "/*! vendor 1.0, MIT */\nexport const top = this;\n\
                 class Shape {\n  #sides;\n  static made = 0;\n\
                 static { Shape.registry = new Map(); }\n\
                 constructor(sides) { this.#sides = sides; Shape.made++; }\n\
                 get sides() { return this.#sides; }\n  set sides(n) { this.#sides = n; }\n\
                 static get kind() { return 'shape'; }\n}\n\
                 export class Square extends Shape {\n  constructor() { super(4); }\n\
                 get area() { return this.sides ** 2; }\n}\n\
                 export const config = { get mode() { return top === undefined; } };\n",
            ),
        ],
    );
    for part in ["#sides;", "static {", "get sides()", "set sides(n)", "static get kind()"] {
        assert!(fast.contains(part), "{part}: {fast}");
    }
    // `this` at the top of a module is undefined, and the joined script is a module too.
    assert!(
        fast.contains("const top = this;") && fast.contains("config.mode, top, this);"),
        "{fast}"
    );
    assert!(built.starts_with("/*! vendor 1.0, MIT */"), "licence comments stay: {built}");
    for part in ["static{", "get sides(){", "set sides(", "static get kind(){", "get area(){"] {
        assert!(built.contains(part), "{part}: {built}");
    }
}

#[test]
fn enums_hold_the_values_typescript_gives_them() {
    let main = "import { Color, Level } from './enums.ts';\n\
                console.log(Color.Alias, Level.Next, Object.keys(Color));\n";
    let enums = "export enum Level { Low, High = 5, Next }\n\
                 export enum Color { Red = \"red\", Alias = Red }\n";
    let Scripts { built, fast } = scripts("enum", &[("main.js", main), ("enums.ts", enums)]);
    let alias = "Color[\"Red\"] = \"red\";\n\tColor[\"Alias\"] = \"red\";\n\treturn Color;";
    assert!(fast.contains(alias), "a string member gets no reverse mapping: {fast}");
    assert!(fast.contains("Level[Level[\"Next\"] = 6] = \"Next\";"), "a member's value: {fast}");
    assert!(built.contains("e[e.Next=6]=`Next`") && built.contains("e.Alias=`red`,e}"), "{built}");
}

#[test]
fn a_module_that_calls_eval_fails_the_build() {
    let main = "import { run } from './evaluator.js';\nconsole.log(run('secret + 1'));\n";
    let evaluator = "import { helper } from './helper.js';\nconst secret = 41;\n\
                     export function run(code) {\n  return eval(code) + helper();\n}\n";
    let modules =
        [("evaluator.js", evaluator), ("helper.js", "export function helper() { return 1; }\n")];
    let root = site("eval", &[&[("main.js", main)], &modules[..]].concat());
    let expected = "pages/index.html: <script src>: s/evaluator.js:4: eval is not supported in \
                    a module script, since a build renames what its code could read";
    assert_eq!(error(&root), expected, "the module and the line of the call are named");
    let fast = cinders::build(&root, Mode::Fast).err().map(|e| format!("{e:#}"));
    assert_eq!(fast.as_deref(), Some(expected), "a fast build fails the same way");

    let main = "const secret = 1;\nconst run = eval;\n\
                console.log((0, eval)('1'), eval?.('2'), run('3'), globalThis.eval('4'), secret);\n";
    let Scripts { built, fast } = scripts("eval-indirect", &[("main.js", main)]);
    let indirect = "an indirect call runs its code outside the module, so it builds";
    assert!(fast.contains("(0, eval)(\"1\"), eval?.(\"2\"), run(\"3\")"), "{indirect}: {fast}");
    assert!(built.contains("(0,eval)(`1`),eval?.(`2`)"), "and it stays indirect: {built}");
}

#[test]
fn classic_scripts_keep_names_under_eval_and_with() {
    let page =
        r#"<script src="a.js"></script><script src="b.js"></script><script src="c.js"></script>"#;
    let files = [
        ("pages/index.html", page),
        (
            "pages/a.js",
            "function viaWith(longName) { with (scope) { return inner + longName; } }\n",
        ),
        ("pages/b.js", "function viaEval(longName) { return eval('longName'); }\n"),
        ("pages/c.js", "function plain(longName) { return longName; }\n"),
    ];
    let site = build(&common::site("classic-names", &files));
    let [with, eval, plain] = ["a", "b", "c"].map(|name| {
        let path = site.files.keys().find(|path| path.starts_with(&format!("assets/{name}-")));
        text(&site, path.unwrap())
    });
    assert!(
        with.contains("function viaWith(longName){with(scope)return inner+longName}"),
        "{with}"
    );
    assert!(eval.contains("function viaEval(longName){return eval(`longName`)}"), "{eval}");
    assert!(plain.contains("function plain(e){return e}"), "{plain}");
}

#[test]
fn imports_name_a_file_in_the_site_exactly() {
    let page = "pages/index.html: <script src>: ";
    let cases = [
        (
            "package",
            "\nimport { x } from 'tiny-pkg';",
            "s/main.js:2: `tiny-pkg` is not a relative path. To use a package, save its ESM build in the site and import that file",
        ),
        ("rooted", "import '/s/x.js';", "s/main.js:1: `/s/x.js` is not a relative path."),
        (
            "missing",
            "import './nope.js';",
            "s/main.js:1: `./nope.js` does not exist. An import names its file in full",
        ),
        (
            "extension",
            "import './x';",
            "s/main.js:1: `./x` does not exist. An import names its file in full",
        ),
        (
            "directory",
            "import './dir';",
            "s/main.js:1: `./dir` is a directory, not a file. An import names its file in full",
        ),
        (
            "outside",
            "import '../../outside.js';",
            "s/main.js:1: `../../outside.js` is outside the site directory",
        ),
        (
            "json",
            "import data from './data.json';",
            "s/main.js:1: `./data.json` is JSON, which a script fetches and cannot import",
        ),
        (
            "css",
            "import './style.css';",
            "s/main.js:1: `./style.css` is a stylesheet, which a page links and cannot import",
        ),
        (
            "other",
            "import './icon.svg';",
            "s/main.js:1: `./icon.svg` is not a JavaScript or TypeScript module",
        ),
        (
            "dynamic",
            "const x = 1;\nconst later = await import('./x.js');",
            "s/main.js:2: import() is not supported, since a script ships as one file. Import the module at the top of the file",
        ),
        (
            "cycle",
            "import './a.js';",
            "s/b.js:2: `./a.js` closes an import cycle: s/a.js to s/b.js to s/a.js",
        ),
        (
            "itself",
            "import './main.js';",
            "s/main.js:1: `./main.js` closes an import cycle: s/main.js to s/main.js",
        ),
        (
            "no-export",
            "import { x } from './x.js';\nimport {\n  nope,\n} from './x.js';",
            "s/main.js:3: `./x.js` has no export `nope`",
        ),
        ("no-default", "import x from './x.js';", "s/main.js:1: `./x.js` has no export `default`"),
        (
            "no-reexport",
            "export { nope } from './x.js';",
            "s/main.js:1: `./x.js` has no export `nope`",
        ),
        ("undeclared", "export { nope };", "s/main.js:1: Export 'nope' is not defined"),
        ("in-module", "import './bad.js';", "s/bad.js:3: `./x.js` has no export `nope`"),
        ("surrogate", "import '\\uD800';", "s/main.js:1: '\\uD800' names no file"),
        (
            "assigned",
            "import { x } from './x.js';\nx = 2;",
            "s/main.js:1: `x` is an import, which cannot be assigned",
        ),
        ("syntax", "const a = 1;\nconst = 2;", "s/main.js:2: "),
        ("with", "with (document) { title = 'x'; }", "s/main.js:1: "),
    ];
    for (name, main, expected) in cases {
        let modules = [
            ("x.js", "export const x = 1;\n"),
            ("a.js", "import './b.js';\n"),
            ("b.js", "\nimport './a.js';\n"),
            ("bad.js", "\n\nimport { nope } from './x.js';\n"),
            ("data.json", "{}"),
            ("style.css", ""),
            ("icon.svg", ""),
            ("dir/index.js", ""),
        ];
        let root = site(&format!("import-{name}"), &[&[("main.js", main)], &modules[..]].concat());
        fs::write(root.parent().unwrap().join("outside.js"), "").unwrap();
        let built = cinders::build(&root, Mode::Optimized).err();
        let error = format!("{:#}", built.unwrap_or_else(|| panic!("{name} should fail")));
        assert!(error.starts_with(&format!("{page}{expected}")), "{name}: {error}");
        // A file is named once, with the line of the problem.
        assert_eq!(error.matches(".js:").count(), 1, "{name}: {error}");
        let fast = cinders::build(&root, Mode::Fast).err().map(|e| format!("{e:#}"));
        assert_eq!(
            fast.as_deref(),
            Some(error.as_str()),
            "{name}: a fast build fails the same way"
        );
    }
}
