//! Client scripts. A module script and the modules it imports by relative path are joined into
//! one module: each loses its import and export syntax, and a top-level name that another
//! module also uses is renamed. oxc strips types and minifies.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write;
use std::fs;
use std::mem;
use std::path::{Path, PathBuf};

use anyhow::{Error, Result, anyhow, bail, ensure};
use oxc::allocator::{Allocator, Box as ArenaBox, GetAllocator, Vec as ArenaVec};
use oxc::ast::ast::*;
use oxc::ast::builder::AstBuilder;
use oxc::ast_visit::{VisitMut, walk_mut};
use oxc::codegen::{Codegen, CodegenOptions, CommentOptions, LegalComment};
use oxc::diagnostics::OxcDiagnostic;
use oxc::mangler::MangleOptions;
use oxc::minifier::{CompressOptions, Minifier, MinifierOptions};
use oxc::parser::Parser;
use oxc::semantic::{Reference, Scoping, SemanticBuilder, SymbolId};
use oxc::span::{GetSpan, SPAN, SourceType, Span};
use oxc::str::Ident;
use oxc::syntax::node::NodeId;
use oxc::syntax::reference::ReferenceFlags;
use oxc::syntax::symbol::SymbolFlags;
use oxc::transformer::{TransformOptions, Transformer};

use super::asset::Kind;
use crate::Mode;
use crate::paths;

/// The binding that `export default <expression>` becomes.
const DEFAULT: &str = "$default";

/// A top-level binding, by the module that declares it.
#[derive(Clone, Copy)]
struct Binding {
    module: usize,
    symbol: SymbolId,
}

/// An import binding, and the binding it names.
struct Import {
    local: SymbolId,
    binding: Binding,
}

/// A module with its export syntax removed, so its statements declare what it exported.
struct Module<'a> {
    /// The file's path relative to the site root, for messages.
    name: String,
    program: Program<'a>,
    scoping: Scoping,
    imports: Vec<Import>,
    /// What each exported name names, in this module or in one it imports.
    exports: HashMap<String, Binding>,
    /// The modules of its `export *` declarations.
    stars: Vec<usize>,
}

/// Where a module's imports lead.
#[derive(Clone, Copy)]
enum Origin<'p> {
    /// A file, whose imports name other files by relative path.
    File(&'p Path),
    /// The module of a namespace object, whose imports all name the module it describes.
    Namespace(usize),
}

/// The modules of one script, each after the modules it imports, which is the order they run
/// in.
struct Joiner<'a, 'r> {
    /// The site directory.
    root: &'r Path,
    /// One arena holds every module's syntax tree.
    allocator: &'a Allocator,
    modules: Vec<Module<'a>>,
    loaded: HashMap<PathBuf, usize>,
    /// The files being loaded, from the entry down, to name an import cycle.
    chain: Vec<PathBuf>,
    /// The namespace object of each module that has one, made when it is first used as a
    /// value.
    namespaces: HashMap<usize, Binding>,
}

/// Joins `entry` and every module it imports into one module, for the site in `root`.
pub(super) fn join(root: &Path, entry: &Path, mode: Mode) -> Result<Vec<u8>> {
    let allocator = Allocator::default();
    let mut joiner = Joiner {
        root,
        allocator: &allocator,
        modules: Vec::new(),
        loaded: HashMap::new(),
        chain: Vec::new(),
        namespaces: HashMap::new(),
    };
    joiner.load(entry)?;
    let mut code = joiner.code();
    // Unused exports go only here, so a fast build's script carries the whole of each module.
    if mode == Mode::Optimized {
        let program = Parser::new(&allocator, &code, SourceType::mjs()).parse().program;
        code = shrink(&allocator, program, Shrink { joined: true, mangle: true });
    }
    Ok(code.into_bytes())
}

/// Minifies a script that cannot import: an inline one, which is a module if `kind` is
/// `Kind::Module`, or a classic `<script src>`. A classic script keeps its top-level names,
/// which are globals that other scripts may read.
pub(super) fn minify(code: &str, kind: Kind, mode: Mode) -> Result<String> {
    let allocator = Allocator::default();
    let source_type = match kind {
        Kind::Module => SourceType::mjs(),
        _ => SourceType::script(),
    };
    let parsed = Parser::new(&allocator, code, source_type).parse();
    // The parser leaves the errors that need scopes, such as a name declared twice.
    let checked = SemanticBuilder::new().with_check_syntax_error(true).build(&parsed.program);
    if let Some(error) = parsed.diagnostics.first().or(checked.diagnostics.first()) {
        bail!("{error}");
    }
    // Inline scripts are not joined, so they cannot import or export. `import.meta` is allowed.
    if parsed.program.body.iter().any(Statement::is_module_declaration) {
        bail!("this script cannot import or export; give it type=\"module\" and a src");
    }
    if mode == Mode::Fast {
        return Ok(code.to_string());
    }
    // oxc renames a local inside `with`, where a short name may be a property.
    let scoping = checked.semantic.into_scoping();
    let with =
        scoping.scope_descendants_from_root().any(|scope| scoping.scope_flags(scope).is_with());
    Ok(shrink(&allocator, parsed.program, Shrink { joined: false, mangle: !with }))
}

/// How far `shrink` goes.
struct Shrink {
    /// The program is the joined modules of a script, which declare every top-level name once.
    joined: bool,
    /// Whether local names are shortened.
    mangle: bool,
}

/// Minifies a program that parsed without errors.
fn shrink<'a>(allocator: &'a Allocator, mut program: Program<'a>, how: Shrink) -> String {
    let options = |mangle: bool| MinifierOptions {
        mangle: mangle.then(MangleOptions::default),
        mangle_properties: None,
        compress: Some(CompressOptions::smallest()),
    };
    // Keep license comments, which vendored libraries need.
    let comments = CommentOptions { legal: LegalComment::Inline, ..CommentOptions::disabled() };
    let codegen = CodegenOptions { comments, ..CodegenOptions::minify() };
    if how.joined {
        // Joined modules compress first with `let` and `const` intact, so constants inline,
        // and then with each as `var`, so one statement declares every top-level name.
        Minifier::new(options(false)).minify(allocator, &mut program);
        for statement in &mut program.body {
            if let Statement::VariableDeclaration(declaration) = statement
                && matches!(
                    declaration.kind,
                    VariableDeclarationKind::Let | VariableDeclarationKind::Const
                )
            {
                declaration.kind = VariableDeclarationKind::Var;
            }
        }
    }
    let minified = Minifier::new(options(how.mangle)).minify(allocator, &mut program);
    Codegen::new().with_options(codegen).with_scoping(minified.scoping).build(&program).code
}

/// The 1-based line of the byte `at` of `source`.
fn line(source: &str, at: u32) -> usize {
    source[..at as usize].matches('\n').count() + 1
}

/// A parser's or a transformer's error, with the line it points at.
fn diagnostic(name: &str, source: &str, error: &OxcDiagnostic) -> Error {
    let at = error.labels.first().map_or(0, |label| label.offset());
    anyhow!("{name}:{}: {error}", line(source, at))
}

/// Parses the module `name`, checks it as a browser does before it runs a script, and strips
/// the types of a TypeScript `file`.
fn parse<'a>(
    allocator: &'a Allocator,
    name: &str,
    source: &'a str,
    file: Option<&Path>,
) -> Result<(Program<'a>, Scoping)> {
    let fail =
        |span: Span, problem: &str| anyhow!("{name}:{}: {problem}", line(source, span.start));
    let typed = file.filter(|file| matches!(paths::extension(file).as_str(), "ts" | "mts"));
    let source_type = if typed.is_some() { SourceType::ts() } else { SourceType::mjs() };
    let parsed = Parser::new(allocator, source, source_type).parse();
    if let Some(error) = parsed.diagnostics.first() {
        return Err(diagnostic(name, source, error));
    }
    if let Some(import) = parsed.module_record.dynamic_imports.first() {
        let problem = "import() is not supported, since a script ships as one file. Import the \
                       module at the top of the file";
        return Err(fail(import.span, problem));
    }
    let mut program = parsed.program;
    // The parser leaves the errors that need scopes, such as a name declared twice. The
    // transformer reads the value of each enum member from the scopes.
    let builder = SemanticBuilder::new().with_check_syntax_error(true).with_enum_eval(true);
    let checked = builder.build(&program);
    if let Some(error) = checked.diagnostics.first() {
        return Err(diagnostic(name, source, error));
    }
    let mut scoping = checked.semantic.into_scoping();
    if scoping.root_scope_flags().contains_direct_eval() {
        // Only this error needs the program's nodes, so they are built for it.
        let semantic = SemanticBuilder::new().with_build_nodes(true).build(&program).semantic;
        let mut calls = semantic.nodes().iter().filter_map(|node| node.kind().as_call_expression());
        let call = calls.find(|call| !call.optional && call.callee.is_specific_id("eval"));
        let problem = "eval is not supported in a module script, since a build renames what \
                       its code could read";
        return Err(fail(call.map_or(SPAN, |call| call.span), problem));
    }
    if let Some(file) = typed {
        let options = TransformOptions::default();
        let transformer = Transformer::new(allocator, file, &options);
        let done = transformer.build_with_scoping(scoping, &mut program);
        if let Some(error) = done.diagnostics.first() {
            return Err(diagnostic(name, source, error));
        }
        // The types have left the program, and leave its scopes here.
        scoping = SemanticBuilder::new().build(&program).semantic.into_scoping();
    }
    Ok((program, scoping))
}

/// The symbol of a default export and the declaration it leaves, or `None` if it exported
/// only a type. An export without a name gets one, and stays or becomes a declaration.
fn default_export<'a>(
    export: ExportDefaultDeclarationKind<'a>,
    scoping: &mut Scoping,
    builder: &AstBuilder<'a>,
) -> Option<(SymbolId, Statement<'a>)> {
    // Only the name of the new symbol is read.
    let mut declare = || {
        // The module may use the name itself, as a binding or as a global.
        let taken = |name: &str| {
            let name = Ident::from(name);
            scoping.get_root_binding(name).is_some()
                || scoping.root_unresolved_references().contains_key(&name)
        };
        let mut name = DEFAULT.to_string();
        while taken(&name) {
            name.push('$');
        }
        let name = Ident::from(builder.allocator().alloc_str(&name));
        let root = scoping.root_scope_id();
        let symbol = scoping.create_symbol(SPAN, name, SymbolFlags::empty(), root, NodeId::DUMMY);
        scoping.add_binding(root, name, symbol);
        BindingIdentifier::new_with_symbol_id(SPAN, name, symbol, builder)
    };
    Some(match export {
        ExportDefaultDeclarationKind::FunctionDeclaration(mut function) => {
            let symbol = function.id.get_or_insert_with(declare).symbol_id();
            (symbol, Statement::FunctionDeclaration(function))
        }
        ExportDefaultDeclarationKind::ClassDeclaration(mut class) => {
            let symbol = class.id.get_or_insert_with(declare).symbol_id();
            (symbol, Statement::ClassDeclaration(class))
        }
        ExportDefaultDeclarationKind::TSInterfaceDeclaration(_) => return None,
        value => {
            let id = declare();
            let symbol = id.symbol_id();
            let id = BindingPattern::BindingIdentifier(ArenaBox::new_in(id, builder));
            let value = Some(value.into_expression());
            let variable = VariableDeclarator::new(SPAN, id, None, value, false, builder);
            let variables = ArenaVec::from_array_in([variable], builder);
            let kind = VariableDeclarationKind::Var;
            (symbol, Statement::new_variable_declaration(SPAN, kind, variables, false, builder))
        }
    })
}

/// The binding that the module `m` exports as `name`.
fn exported(modules: &[Module], m: usize, name: &str) -> Option<Binding> {
    let module = &modules[m];
    // `export *` passes on every name but `default`.
    let mut stars = module.stars.iter().filter(|_| name != "default");
    let own = module.exports.get(name).copied();
    own.or_else(|| stars.find_map(|&dep| exported(modules, dep, name)))
}

/// Every name the module `m` exports, in the order a namespace object lists them.
fn export_names<'m>(modules: &'m [Module], m: usize) -> BTreeSet<&'m str> {
    let stars = modules[m].stars.iter().flat_map(|&dep| export_names(modules, dep));
    let mut names: BTreeSet<&str> = stars.collect();
    names.remove("default");
    names.extend(modules[m].exports.keys().map(String::as_str));
    names
}

impl<'a> Joiner<'a, '_> {
    /// Loads `file` and the modules it imports, and returns its index.
    fn load(&mut self, file: &Path) -> Result<usize> {
        if let Some(&module) = self.loaded.get(file) {
            return Ok(module);
        }
        let name = paths::name(self.root, file);
        let source = fs::read_to_string(file).map_err(|e| anyhow!("{name}: {e}"))?;
        self.chain.push(file.to_path_buf());
        let module = self.add(name, &source, Origin::File(file))?;
        self.chain.pop();
        self.loaded.insert(file.to_path_buf(), module);
        Ok(module)
    }

    /// Resolves an import of the module `from`, which names a file in the site exactly, by a
    /// relative path.
    fn resolve(&self, from: &Path, request: &str) -> Result<PathBuf> {
        ensure!(
            request.starts_with("./") || request.starts_with("../"),
            "`{request}` is not a relative path. To use a package, save its ESM build in the \
             site and import that file"
        );
        let file = paths::normalize(&from.with_file_name(request));
        // A module that has loaded passed the checks below, and closes no cycle.
        if self.loaded.contains_key(&file) {
            return Ok(file);
        }
        if !file.is_file() {
            let problem =
                if file.is_dir() { "is a directory, not a file" } else { "does not exist" };
            bail!("`{request}` {problem}. An import names its file in full");
        }
        ensure!(file.starts_with(self.root), "`{request}` is outside the site directory");
        match paths::extension(&file).as_str() {
            "js" | "mjs" | "ts" | "mts" => {}
            "json" => bail!("`{request}` is JSON, which a script fetches and cannot import"),
            "css" => bail!("`{request}` is a stylesheet, which a page links and cannot import"),
            _ => bail!("`{request}` is not a JavaScript or TypeScript module"),
        }
        if let Some(start) = self.chain.iter().position(|seen| *seen == file) {
            let cycle = self.chain[start..].iter().chain([&file]);
            let cycle: Vec<String> = cycle.map(|file| paths::name(self.root, file)).collect();
            bail!("`{request}` closes an import cycle: {}", cycle.join(" to "));
        }
        Ok(file)
    }

    /// Adds the module `name` after the modules it imports, and returns its index.
    fn add(&mut self, name: String, source: &str, origin: Origin) -> Result<usize> {
        let allocator = self.allocator;
        let source = allocator.alloc_str(source);
        let fail =
            |span: Span, problem: String| anyhow!("{name}:{}: {problem}", line(source, span.start));
        let file = match origin {
            Origin::File(file) => Some(file),
            Origin::Namespace(_) => None,
        };
        let (mut program, mut scoping) = parse(allocator, &name, source, file)?;

        // The modules this one imports load first. The types are gone by now, and with them
        // the imports that only named types.
        let mut deps: HashMap<&str, usize> = HashMap::new();
        for statement in &program.body {
            let literal = match statement {
                Statement::ImportDeclaration(import) => &import.source,
                Statement::ExportFromDeclaration(export) => &export.source,
                Statement::ExportAllDeclaration(export) => &export.source,
                _ => continue,
            };
            // A path with a lone surrogate is not a string that can name a file.
            let Some(request) = literal.value.as_str() else {
                let written = literal.span.source_text(source);
                return Err(fail(literal.span, format!("{written} names no file")));
            };
            let dep = match origin {
                Origin::File(file) => {
                    let file = self.resolve(file, request);
                    self.load(&file.map_err(|e| fail(literal.span, e.to_string()))?)?
                }
                Origin::Namespace(of) => of,
            };
            deps.insert(request, dep);
        }
        // Every source is a key by now.
        let request = |literal: &StringLiteral<'a>| literal.value.as_str().unwrap_or_default();
        let dep = |literal: &StringLiteral<'a>| deps[request(literal)];
        let missing = |literal: &StringLiteral<'a>, export: &str| {
            format!("`{}` has no export `{export}`", request(literal))
        };

        // Export syntax goes, and what it declared stays. Import declarations stay until
        // the modules are joined, since their bindings are renamed there.
        let builder = AstBuilder::new(allocator);
        let mut exports = HashMap::new();
        // The exports of the module's own top-level bindings, some of which it imports.
        let mut locals = Vec::new();
        let mut stars = Vec::new();
        let mut body = ArenaVec::with_capacity_in(program.body.len(), &builder);
        for statement in mem::replace(&mut program.body, ArenaVec::new_in(&builder)) {
            let span = statement.span();
            match statement {
                Statement::ExportDeclaration(export) => {
                    let declaration = export.unbox().declaration;
                    let ids: Vec<&BindingIdentifier> = match &declaration {
                        Declaration::FunctionDeclaration(function) => function.id.iter().collect(),
                        Declaration::ClassDeclaration(class) => class.id.iter().collect(),
                        Declaration::VariableDeclaration(variables) => {
                            let ids = variables.declarations.iter();
                            ids.flat_map(|declarator| declarator.id.get_binding_identifiers())
                                .collect()
                        }
                        _ => Vec::new(),
                    };
                    for id in ids {
                        locals.push((id.name.to_string(), id.symbol_id()));
                    }
                    body.push(Statement::from(declaration));
                }
                Statement::ExportNamedDeclaration(export) => {
                    for specifier in &export.specifiers {
                        let local = specifier.local.name();
                        let Some(symbol) = scoping.get_root_binding(Ident::from(local.as_str()))
                        else {
                            let problem = format!("`{local}` is exported but not declared");
                            return Err(fail(span, problem));
                        };
                        locals.push((specifier.exported.name().to_string(), symbol));
                    }
                }
                Statement::ExportFromDeclaration(export) => {
                    let from = dep(&export.source);
                    for specifier in &export.specifiers {
                        let name = specifier.local.name();
                        let Some(binding) = exported(&self.modules, from, &name) else {
                            return Err(fail(span, missing(&export.source, &name)));
                        };
                        exports.insert(specifier.exported.name().to_string(), binding);
                    }
                }
                Statement::ExportAllDeclaration(export) => match &export.exported {
                    None => stars.push(dep(&export.source)),
                    Some(name) => {
                        let object = self.namespace(dep(&export.source))?;
                        exports.insert(name.name().to_string(), object);
                    }
                },
                Statement::ExportDefaultDeclaration(export) => {
                    let export = export.unbox().declaration;
                    if let Some((symbol, statement)) =
                        default_export(export, &mut scoping, &builder)
                    {
                        locals.push(("default".to_string(), symbol));
                        body.push(statement);
                    }
                }
                statement => body.push(statement),
            }
        }
        program.body = body;
        // A directive or a hashbang would land in the middle of the joined module.
        program.directives.clear();
        program.hashbang = None;

        let mut imports = Vec::new();
        let mut namespaces = Vec::new();
        for statement in &program.body {
            let Statement::ImportDeclaration(import) = statement else { continue };
            let from = dep(&import.source);
            for specifier in import.specifiers.iter().flatten() {
                let local = specifier.local();
                if scoping.symbol_is_mutated(local.symbol_id()) {
                    let problem =
                        format!("`{}` is an import, which cannot be assigned", local.name);
                    return Err(fail(local.span, problem));
                }
                let name = match specifier {
                    ImportDeclarationSpecifier::ImportSpecifier(specifier) => {
                        specifier.imported.name()
                    }
                    ImportDeclarationSpecifier::ImportDefaultSpecifier(_) => "default".into(),
                    ImportDeclarationSpecifier::ImportNamespaceSpecifier(_) => {
                        namespaces.push((local.symbol_id(), from));
                        continue;
                    }
                };
                let Some(binding) = exported(&self.modules, from, &name) else {
                    return Err(fail(specifier.span(), missing(&import.source, &name)));
                };
                imports.push(Import { local: local.symbol_id(), binding });
            }
        }
        if !namespaces.is_empty() {
            let mut members = Members {
                builder,
                scoping: &mut scoping,
                modules: &self.modules,
                namespaces: &namespaces,
                imports: &mut imports,
            };
            members.visit_program(&mut program);
        }
        for (symbol, from) in namespaces {
            // A namespace that is only read by member needs no object.
            if !scoping.get_resolved_reference_ids(symbol).is_empty() {
                imports.push(Import { local: symbol, binding: self.namespace(from)? });
            }
        }
        let module = self.modules.len();
        for (name, symbol) in locals {
            // An export of an import names what the import names.
            let import = imports.iter().find(|import| import.local == symbol);
            let binding = import.map_or(Binding { module, symbol }, |import| import.binding);
            exports.insert(name, binding);
        }
        self.modules.push(Module { name, program, scoping, imports, exports, stars });
        Ok(module)
    }

    /// The namespace object of the module `of`: the default export of a module made for it,
    /// an object with a getter for each export, so that it reads live bindings.
    fn namespace(&mut self, of: usize) -> Result<Binding> {
        if let Some(&object) = self.namespaces.get(&of) {
            return Ok(object);
        }
        let names = export_names(&self.modules, of);
        let mut source = "import{".to_string();
        for (i, name) in names.iter().enumerate() {
            write!(source, "{name:?} as ${i},")?;
        }
        source.push_str("}from\"\";export default{__proto__:null,");
        for (i, name) in names.iter().enumerate() {
            write!(source, "get {name:?}(){{return ${i}}},")?;
        }
        source.push('}');
        let name = self.modules[of].name.clone();
        let module = self.add(name, &source, Origin::Namespace(of))?;
        let object = self.modules[module].exports["default"];
        self.namespaces.insert(of, object);
        Ok(object)
    }

    /// The modules as one module, in order. A top-level name that another module also uses
    /// is renamed, and each import binding takes the name of the binding it imports.
    fn code(mut self) -> String {
        // How many modules use each name. A name in an inner scope and a global count too,
        // since a top-level binding of that name would capture them.
        let mut uses: HashMap<String, usize> = HashMap::new();
        for module in &self.modules {
            let scoping = &module.scoping;
            let declared = scoping.symbol_ids().filter(|&id| !scoping.symbol_flags(id).is_import());
            let declared = declared.map(|id| scoping.symbol_name(id));
            let globals = scoping.root_unresolved_references().keys().map(|name| name.as_str());
            let names: HashSet<&str> = declared.chain(globals).collect();
            for name in names {
                *uses.entry(name.to_string()).or_default() += 1;
            }
        }
        let mut taken = HashSet::new();
        for m in 0..self.modules.len() {
            let (done, rest) = self.modules.split_at_mut(m);
            let Module { scoping, imports, .. } = &mut rest[0];
            let symbols = scoping.iter_bindings_in(scoping.root_scope_id());
            let symbols: Vec<SymbolId> =
                symbols.filter(|&id| !scoping.symbol_flags(id).is_import()).collect();
            for symbol in symbols {
                let name = scoping.symbol_name(symbol);
                if uses[name] == 1 {
                    continue;
                }
                // A name no module uses and no rename took can neither collide nor be captured.
                let fresh = |new: &String| !uses.contains_key(new) && !taken.contains(new);
                let mut names = (1..).map(|k| format!("{name}${k}"));
                let Some(new) = names.find(fresh) else { continue };
                scoping.set_symbol_name(symbol, Ident::from(new.as_str()));
                taken.insert(new);
            }
            for import in imports.iter() {
                let binding = import.binding;
                let name = done[binding.module].scoping.symbol_name(binding.symbol);
                scoping.set_symbol_name(import.local, Ident::from(name));
            }
        }
        let mut code = String::new();
        for module in self.modules {
            let Module { mut program, scoping, .. } = module;
            program.body.retain(|statement| !matches!(statement, Statement::ImportDeclaration(_)));
            code.push_str(&Codegen::new().with_scoping(Some(scoping)).build(&program).code);
        }
        code
    }
}

/// Turns each `ns.name` that reads a namespace import into a binding that imports `name`,
/// so that a namespace import drops what it does not use, as a named import does.
struct Members<'a, 'j> {
    builder: AstBuilder<'a>,
    scoping: &'j mut Scoping,
    modules: &'j [Module<'a>],
    /// Each namespace import and the module it names.
    namespaces: &'j [(SymbolId, usize)],
    imports: &'j mut Vec<Import>,
}

impl<'a> VisitMut<'a> for Members<'a, '_> {
    fn visit_expression(&mut self, it: &mut Expression<'a>) {
        if let Expression::StaticMemberExpression(member) = it
            && let Expression::Identifier(object) = &member.object
            && let Some(symbol) = self.scoping.get_reference(object.reference_id()).symbol_id()
            && let Some(&(_, dep)) = self.namespaces.iter().find(|(local, _)| *local == symbol)
        {
            let span = member.span;
            let name = member.property.name;
            self.scoping.delete_resolved_reference(symbol, object.reference_id());
            // A namespace has no other properties, so a name it lacks reads as undefined.
            let Some(binding) = exported(self.modules, dep, &name) else {
                *it = Expression::new_void_0(span, &self.builder);
                return;
            };
            let root = self.scoping.root_scope_id();
            let local =
                self.scoping.create_symbol(span, name, SymbolFlags::Import, root, NodeId::DUMMY);
            let reference =
                Reference::new_with_symbol_id(NodeId::DUMMY, local, root, ReferenceFlags::Read);
            let reference = self.scoping.create_reference(reference);
            self.imports.push(Import { local, binding });
            *it =
                Expression::new_identifier_with_reference_id(span, name, reference, &self.builder);
            return;
        }
        // `delete ns.name` deletes a property, and must not come to delete a binding.
        if !matches!(it, Expression::UnaryExpression(unary) if unary.operator.is_delete()) {
            walk_mut::walk_expression(self, it);
        }
    }
}
