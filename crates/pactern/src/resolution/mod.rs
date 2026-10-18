//! Package-local module and lexical name resolution, without filesystem I/O.
//!
//! Call [`resolve`] with parsed files and package-relative source identities.
//! IDs are deterministic for a fixed input order and are local to this result;
//! they are not persistent semantic IDs. Diagnostics and references retain file
//! identities because syntax spans alone cannot identify a source file.
//!
//! This phase does not type-check expressions, infer field receiver types, check
//! effects or host registries, or enforce generic recursion/handle rules. Member
//! access and effect labels are returned as [`DeferredUse`]s for later phases.
//! Diagnostics are local compiler results, not revision-bound protocol envelopes.
//! Inputs must be parser-produced trees; this API does not validate hand-built ASTs.
//!
//! ```
//! use pactern::resolution::{ParsedFile, resolve};
//! use pactern::syntax::{FileKind, SourceFile, parse};
//!
//! let source = SourceFile::new("src/main.ptn",
//!     "module main; fn answer() -> I64 { return 42; }");
//! let tree = parse(&source, FileKind::Implementation)?;
//! let resolution = resolve(&[ParsedFile::new(source.name(), tree)]);
//! assert!(resolution.is_success());
//! # Ok::<(), pactern::syntax::SyntaxDiagnostic>(())
//! ```

use std::collections::BTreeMap;
use std::fmt;

use crate::syntax::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct FileId(pub usize);
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct ModuleId(pub usize);
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct SymbolId(pub usize);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedFile {
    /// Package-relative identity, e.g. `src/util/math.ptn`.
    pub path: String,
    pub tree: SyntaxTree,
}

impl ParsedFile {
    pub fn new(path: impl Into<String>, tree: SyntaxTree) -> Self {
        Self {
            path: path.into(),
            tree,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Location {
    pub file: FileId,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelatedLocation {
    pub location: Location,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolveDiagnostic {
    pub code: &'static str,
    pub phase: &'static str,
    pub message: String,
    pub location: Location,
    pub related: Vec<RelatedLocation>,
}

impl fmt::Display for ResolveDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:file{}:{}:{}: {}",
            self.code,
            self.location.file.0,
            self.location.span.start.line,
            self.location.span.start.column,
            self.message
        )
    }
}
impl std::error::Error for ResolveDiagnostic {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SymbolKind {
    Module,
    BuiltinType,
    BuiltinVariant,
    Function,
    Struct,
    Enum,
    Pact,
    Field,
    Variant,
    TypeParameter,
    Parameter,
    Local,
    PatternBinding,
    Result,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Symbol {
    pub id: SymbolId,
    pub name: String,
    pub kind: SymbolKind,
    pub module: Option<ModuleId>,
    pub parent: Option<SymbolId>,
    pub public: bool,
    /// Prelude symbols and the implicit postcondition result have no declaration.
    pub declaration: Option<Location>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Module {
    pub id: ModuleId,
    pub name: String,
    pub symbol: SymbolId,
    pub implementation: Option<FileId>,
    pub contract: Option<FileId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Reference {
    pub location: Location,
    pub symbol: SymbolId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeferredUseKind {
    FieldAccess,
    EffectLabel,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeferredUse {
    pub location: Location,
    pub name: String,
    pub kind: DeferredUseKind,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Resolution {
    /// Index by FileId; these identities are copied from the input.
    pub files: Vec<String>,
    pub modules: Vec<Module>,
    /// Index by SymbolId, including declarations retained after an error.
    pub symbols: Vec<Symbol>,
    pub references: Vec<Reference>,
    pub deferred: Vec<DeferredUse>,
    pub diagnostics: Vec<ResolveDiagnostic>,
}

impl Resolution {
    pub fn is_success(&self) -> bool {
        self.diagnostics.is_empty()
    }
    pub fn symbol(&self, id: SymbolId) -> Option<&Symbol> {
        self.symbols.get(id.0)
    }
}

/// Resolve an explicitly supplied, closed set of package source files.
///
/// Returns partial indexes alongside all resolution diagnostics. Import names
/// never cause filesystem traversal or dependency downloads. `std` and `host`
/// remain unavailable until a trusted registry can supply their declarations.
/// Imports are file-scoped, while declaration namespaces and cycle detection
/// combine implementation and contract files belonging to the same module.
pub fn resolve(files: &[ParsedFile]) -> Resolution {
    Resolver::new(files).run()
}

const PRELUDE: &[&str] = &["Bool", "I64", "String", "Unit", "List", "Option", "Result"];

type Names = BTreeMap<String, SymbolId>;

struct Resolver<'a> {
    files: &'a [ParsedFile],
    out: Resolution,
    module_names: BTreeMap<String, ModuleId>,
    file_modules: Vec<ModuleId>,
    declarations: Vec<Names>,
    imports: Vec<BTreeMap<String, ModuleId>>,
    members: BTreeMap<SymbolId, Names>,
    generics: BTreeMap<SymbolId, Names>,
    parameters: BTreeMap<SymbolId, Names>,
    declaration_ids: BTreeMap<(usize, usize), SymbolId>,
    prelude: Names,
}

#[derive(Default)]
struct Scope {
    locals: Names,
    types: Names,
    result: Option<SymbolId>,
}

#[derive(Clone, Copy)]
enum Expected {
    Type,
    Value,
    Callable,
    Record,
    Variant,
    Function,
}

impl<'a> Resolver<'a> {
    fn new(files: &'a [ParsedFile]) -> Self {
        Self {
            files,
            out: Resolution {
                files: files.iter().map(|f| f.path.clone()).collect(),
                ..Resolution::default()
            },
            module_names: BTreeMap::new(),
            file_modules: Vec::new(),
            declarations: Vec::new(),
            imports: vec![BTreeMap::new(); files.len()],
            members: BTreeMap::new(),
            generics: BTreeMap::new(),
            parameters: BTreeMap::new(),
            declaration_ids: BTreeMap::new(),
            prelude: Names::new(),
        }
    }

    fn run(mut self) -> Resolution {
        self.add_prelude();
        self.collect_modules();
        self.collect_declarations();
        self.resolve_imports();
        for (file, parsed) in self.files.iter().enumerate() {
            for declaration in &parsed.tree.declarations {
                self.resolve_declaration(FileId(file), declaration);
            }
        }
        self.out
    }

    fn location(file: FileId, span: Span) -> Location {
        Location { file, span }
    }

    fn error(
        &mut self,
        code: &'static str,
        message: impl Into<String>,
        location: Location,
        related: Option<Location>,
    ) {
        self.out.diagnostics.push(ResolveDiagnostic {
            code,
            phase: "resolve",
            message: message.into(),
            location,
            related: related
                .into_iter()
                .map(|location| RelatedLocation {
                    location,
                    message: "related declaration or import".into(),
                })
                .collect(),
        });
    }

    fn add_symbol(
        &mut self,
        name: &str,
        kind: SymbolKind,
        module: Option<ModuleId>,
        parent: Option<SymbolId>,
        public: bool,
        declaration: Option<Location>,
    ) -> SymbolId {
        let id = SymbolId(self.out.symbols.len());
        self.out.symbols.push(Symbol {
            id,
            name: name.into(),
            kind,
            module,
            parent,
            public,
            declaration,
        });
        id
    }

    fn add_prelude(&mut self) {
        for name in PRELUDE {
            let id = self.add_symbol(name, SymbolKind::BuiltinType, None, None, true, None);
            self.prelude.insert((*name).into(), id);
        }
        for (ty, variants) in [
            ("Option", &["Some", "None"][..]),
            ("Result", &["Ok", "Err"][..]),
        ] {
            let parent = self.prelude[ty];
            for name in variants {
                let id = self.add_symbol(
                    name,
                    SymbolKind::BuiltinVariant,
                    None,
                    Some(parent),
                    true,
                    None,
                );
                self.members
                    .entry(parent)
                    .or_default()
                    .insert((*name).into(), id);
            }
        }
    }

    fn collect_modules(&mut self) {
        let mut paths = BTreeMap::<&str, Location>::new();
        for (index, parsed) in self.files.iter().enumerate() {
            let file = FileId(index);
            let path = &parsed.tree.module.value;
            let name = path_name(path);
            let location = Self::location(file, path.span);
            if let Some(previous) = paths.insert(&parsed.path, location) {
                self.error(
                    "PTN_RESOLVE_DUPLICATE_FILE",
                    "source file identity occurs more than once",
                    location,
                    Some(previous),
                );
            }
            let root = path.value.first().map(|s| s.value.as_str()).unwrap_or("");
            if matches!(root, "host" | "std") {
                self.error(
                    "PTN_RESOLVE_RESERVED_MODULE",
                    format!("module root '{root}' belongs to the toolchain"),
                    location,
                    None,
                );
            }
            let expected = match parsed.tree.kind {
                FileKind::Implementation => format!("src/{}.ptn", name.replace("::", "/")),
                FileKind::Contract => format!("contracts/{}.pact.ptn", name.replace("::", "/")),
            };
            if parsed.path != expected {
                self.error(
                    "PTN_RESOLVE_MODULE_PATH",
                    format!("module '{name}' requires source identity '{expected}'"),
                    location,
                    None,
                );
            }
            let module = if let Some(id) = self.module_names.get(&name) {
                *id
            } else {
                let id = ModuleId(self.out.modules.len());
                let symbol = self.add_symbol(
                    &name,
                    SymbolKind::Module,
                    Some(id),
                    None,
                    false,
                    Some(location),
                );
                self.out.modules.push(Module {
                    id,
                    name: name.clone(),
                    symbol,
                    implementation: None,
                    contract: None,
                });
                self.module_names.insert(name, id);
                self.declarations.push(Names::new());
                id
            };
            self.file_modules.push(module);
            let entry = &mut self.out.modules[module.0];
            let slot = match parsed.tree.kind {
                FileKind::Implementation => &mut entry.implementation,
                FileKind::Contract => &mut entry.contract,
            };
            if let Some(previous) = *slot {
                self.error(
                    "PTN_RESOLVE_DUPLICATE_MODULE_FILE",
                    "module already has a file of this kind",
                    location,
                    Some(Self::location(
                        previous,
                        self.files[previous.0].tree.module.value.span,
                    )),
                );
            } else {
                *slot = Some(file);
            }
        }
        for index in 0..self.out.modules.len() {
            let module = &self.out.modules[index];
            if module.implementation.is_none() {
                let file = module.contract.expect("a module has a source file");
                self.error(
                    "PTN_RESOLVE_MISSING_IMPLEMENTATION",
                    "contract module has no implementation file",
                    Self::location(file, self.files[file.0].tree.module.value.span),
                    None,
                );
            }
        }
    }

    fn collect_declarations(&mut self) {
        for (index, parsed) in self.files.iter().enumerate() {
            let file = FileId(index);
            let module = self.file_modules[index];
            for declaration in &parsed.tree.declarations {
                let (name, kind) = match &declaration.kind {
                    DeclarationKind::Function(f) => (&f.name, SymbolKind::Function),
                    DeclarationKind::Struct(s) => (&s.name, SymbolKind::Struct),
                    DeclarationKind::Enum(e) => (&e.name, SymbolKind::Enum),
                    DeclarationKind::Pact(p) => (&p.name, SymbolKind::Pact),
                };
                let location = Self::location(file, name.span);
                let id = self.add_symbol(
                    &name.value,
                    kind,
                    Some(module),
                    None,
                    declaration.public,
                    Some(location),
                );
                self.declaration_ids
                    .insert((index, declaration.span.start.byte), id);
                if let Some(previous) = self.declarations[module.0].get(&name.value).copied() {
                    self.error(
                        "PTN_RESOLVE_DUPLICATE_DECLARATION",
                        format!("duplicate module declaration '{}'", name.value),
                        location,
                        self.out.symbols[previous.0].declaration,
                    );
                } else {
                    self.declarations[module.0].insert(name.value.clone(), id);
                }
                if matches!(kind, SymbolKind::Struct | SymbolKind::Enum)
                    && PRELUDE.contains(&name.value.as_str())
                {
                    self.error(
                        "PTN_RESOLVE_RESERVED_TYPE",
                        format!("prelude type '{}' cannot be redeclared", name.value),
                        location,
                        None,
                    );
                }
                let generics = match &declaration.kind {
                    DeclarationKind::Function(f) => &f.type_parameters,
                    DeclarationKind::Struct(s) => &s.type_parameters,
                    DeclarationKind::Enum(e) => &e.type_parameters,
                    DeclarationKind::Pact(_) => continue,
                };
                let mut names = Names::new();
                for generic in generics {
                    self.child(
                        file,
                        id,
                        generic,
                        SymbolKind::TypeParameter,
                        &mut names,
                        "PTN_RESOLVE_DUPLICATE_TYPE_PARAMETER",
                    );
                    if PRELUDE.contains(&generic.value.as_str()) {
                        self.error(
                            "PTN_RESOLVE_RESERVED_TYPE",
                            format!(
                                "prelude type '{}' cannot be a type parameter",
                                generic.value
                            ),
                            Self::location(file, generic.span),
                            None,
                        );
                    }
                }
                self.generics.insert(id, names);
                let mut names = Names::new();
                match &declaration.kind {
                    DeclarationKind::Function(f) => {
                        for p in &f.parameters {
                            self.child(
                                file,
                                id,
                                &p.name,
                                SymbolKind::Parameter,
                                &mut names,
                                "PTN_RESOLVE_DUPLICATE_LOCAL",
                            );
                        }
                        self.parameters.insert(id, names);
                    }
                    DeclarationKind::Struct(s) => {
                        for field in &s.fields {
                            self.child(
                                file,
                                id,
                                &field.name,
                                SymbolKind::Field,
                                &mut names,
                                "PTN_RESOLVE_DUPLICATE_FIELD",
                            );
                        }
                        self.members.insert(id, names);
                    }
                    DeclarationKind::Enum(e) => {
                        for variant in &e.variants {
                            self.child(
                                file,
                                id,
                                &variant.name,
                                SymbolKind::Variant,
                                &mut names,
                                "PTN_RESOLVE_DUPLICATE_VARIANT",
                            );
                        }
                        self.members.insert(id, names);
                    }
                    DeclarationKind::Pact(_) => unreachable!(),
                }
            }
        }
    }

    fn child(
        &mut self,
        file: FileId,
        parent: SymbolId,
        name: &Identifier,
        kind: SymbolKind,
        names: &mut Names,
        code: &'static str,
    ) -> SymbolId {
        let location = Self::location(file, name.span);
        let id = self.add_symbol(
            &name.value,
            kind,
            Some(self.file_modules[file.0]),
            Some(parent),
            false,
            Some(location),
        );
        if let Some(previous) = names.get(&name.value) {
            self.error(
                code,
                format!("duplicate name '{}'", name.value),
                location,
                self.out.symbols[previous.0].declaration,
            );
        } else {
            names.insert(name.value.clone(), id);
        }
        id
    }

    fn resolve_imports(&mut self) {
        let mut edges = vec![Vec::<(ModuleId, Location)>::new(); self.out.modules.len()];
        for (index, parsed) in self.files.iter().enumerate() {
            let module = self.file_modules[index];
            let mut seen = BTreeMap::new();
            for import in &parsed.tree.imports {
                let name = path_name(&import.value);
                let location = Self::location(FileId(index), import.value.span);
                if let Some(previous) = seen.insert(name.clone(), location) {
                    self.error(
                        "PTN_RESOLVE_DUPLICATE_IMPORT",
                        format!("duplicate import '{name}'"),
                        location,
                        Some(previous),
                    );
                    continue;
                }
                if let Some(target) = self.module_names.get(&name).copied().filter(|id| {
                    !matches!(
                        self.out.modules[id.0].name.split("::").next(),
                        Some("std" | "host")
                    )
                }) {
                    self.imports[index].insert(name, target);
                    self.reference(location, self.out.modules[target.0].symbol);
                    edges[module.0].push((target, location));
                } else {
                    self.error(
                        "PTN_RESOLVE_UNRESOLVED_MODULE",
                        format!(
                            "module '{name}' is not supplied by this package or a trusted registry"
                        ),
                        location,
                        None,
                    );
                }
            }
        }
        // Iterative DFS: source packages can contain arbitrarily long import chains.
        let mut state = vec![0u8; edges.len()];
        for root in 0..edges.len() {
            if state[root] != 0 {
                continue;
            }
            state[root] = 1;
            let mut stack = vec![(root, 0usize)];
            while let Some((node, next)) = stack.last_mut() {
                if *next == edges[*node].len() {
                    state[*node] = 2;
                    stack.pop();
                    continue;
                }
                let (target, location) = edges[*node][*next];
                *next += 1;
                if state[target.0] == 1 {
                    let mut related: Vec<_> = stack
                        .iter()
                        .skip_while(|(n, _)| *n != target.0)
                        .filter_map(|(n, _)| {
                            self.out.symbols[self.out.modules[*n].symbol.0].declaration
                        })
                        .map(|location| RelatedLocation {
                            location,
                            message: "module in import cycle".into(),
                        })
                        .collect();
                    self.error(
                        "PTN_RESOLVE_IMPORT_CYCLE",
                        "module import cycle",
                        location,
                        None,
                    );
                    self.out
                        .diagnostics
                        .last_mut()
                        .expect("just added")
                        .related
                        .append(&mut related);
                } else if state[target.0] == 0 {
                    state[target.0] = 1;
                    stack.push((target.0, 0));
                }
            }
        }
    }

    fn reference(&mut self, location: Location, symbol: SymbolId) {
        self.out.references.push(Reference { location, symbol });
    }

    fn resolve_declaration(&mut self, file: FileId, declaration: &Declaration) {
        let id = self.declaration_ids[&(file.0, declaration.span.start.byte)];
        let mut scope = Scope {
            types: self.generics.get(&id).cloned().unwrap_or_default(),
            ..Scope::default()
        };
        match &declaration.kind {
            DeclarationKind::Function(f) => {
                scope.locals = self.parameters[&id].clone();
                for p in &f.parameters {
                    self.ty(file, &p.ty, &scope, declaration.public);
                }
                self.ty(file, &f.return_type, &scope, declaration.public);
                if let Some(effects) = &f.effects {
                    for path in &effects.value {
                        self.out.deferred.push(DeferredUse {
                            location: Self::location(file, path.span),
                            name: path
                                .value
                                .iter()
                                .map(|n| n.value.as_str())
                                .collect::<Vec<_>>()
                                .join("."),
                            kind: DeferredUseKind::EffectLabel,
                        });
                    }
                }
                self.block(file, id, &f.body, &mut scope);
            }
            DeclarationKind::Struct(s) => {
                for field in &s.fields {
                    self.ty(file, &field.ty, &scope, false);
                }
            }
            DeclarationKind::Enum(e) => {
                for variant in &e.variants {
                    for ty in &variant.payload {
                        self.ty(file, ty, &scope, false);
                    }
                }
            }
            DeclarationKind::Pact(p) => {
                // A pact target is always a same-module, unqualified function.
                let target = if p.target.value.len() == 1 {
                    self.path(file, &p.target, &scope, Expected::Function)
                } else {
                    self.error(
                        "PTN_RESOLVE_INVALID_PACT_TARGET",
                        "pact target must be an unqualified function in the same module",
                        Self::location(file, p.target.span),
                        None,
                    );
                    None
                };
                if let Some(target) = target {
                    if !self.generics[&target].is_empty() {
                        self.error(
                            "PTN_RESOLVE_GENERIC_PACT_TARGET",
                            "pact target must be nongeneric",
                            Self::location(file, p.target.span),
                            self.out.symbols[target.0].declaration,
                        );
                    }
                    scope.locals = self.parameters[&target].clone();
                    let result = self.add_symbol(
                        "result",
                        SymbolKind::Result,
                        Some(self.file_modules[file.0]),
                        Some(target),
                        false,
                        None,
                    );
                    for clause in &p.clauses {
                        let expression = match &clause.value {
                            PactClause::Requires(expr) => {
                                scope.result = None;
                                expr
                            }
                            PactClause::Ensures(expr) => {
                                scope.result = Some(result);
                                expr
                            }
                        };
                        self.expr(file, id, expression, &mut scope);
                    }
                } else {
                    // Retain independent clause errors even when the target is absent.
                    for clause in &p.clauses {
                        let expression = match &clause.value {
                            PactClause::Requires(e) | PactClause::Ensures(e) => e,
                        };
                        self.expr(file, id, expression, &mut scope);
                    }
                }
            }
        }
    }

    fn ty(&mut self, file: FileId, ty: &Type, scope: &Scope, public: bool) {
        if let Some(id) = self.path(file, &ty.path, scope, Expected::Type) {
            let symbol = &self.out.symbols[id.0];
            if public
                && matches!(symbol.kind, SymbolKind::Struct | SymbolKind::Enum)
                && !symbol.public
            {
                self.error(
                    "PTN_RESOLVE_PRIVATE_TYPE_EXPOSURE",
                    format!("public function exposes private type '{}'", symbol.name),
                    Self::location(file, ty.path.span),
                    symbol.declaration,
                );
            }
        }
        for argument in &ty.arguments {
            self.ty(file, argument, scope, public);
        }
    }

    fn path(
        &mut self,
        file: FileId,
        path: &Path,
        scope: &Scope,
        expected: Expected,
    ) -> Option<SymbolId> {
        let names: Vec<&str> = path.value.iter().map(|s| s.value.as_str()).collect();
        let location = Self::location(file, path.span);
        let module = self.file_modules[file.0];
        let mut symbol = None;
        if names.len() == 1 {
            let name = names[0];
            symbol = match expected {
                Expected::Type => scope
                    .types
                    .get(name)
                    .or_else(|| self.prelude.get(name))
                    .or_else(|| self.declarations[module.0].get(name)),
                _ => scope
                    .locals
                    .get(name)
                    .or_else(|| self.declarations[module.0].get(name)),
            }
            .copied();
        } else if names.len() == 2 {
            // Lexical names win over module and prelude names, including for calls.
            let parent = scope
                .locals
                .get(names[0])
                .or_else(|| scope.types.get(names[0]))
                .or_else(|| self.declarations[module.0].get(names[0]))
                .or_else(|| self.prelude.get(names[0]))
                .copied();
            if let Some(parent) = parent {
                symbol = self
                    .members
                    .get(&parent)
                    .and_then(|members| members.get(names[1]))
                    .copied();
                if symbol.is_some() {
                    self.reference(Self::location(file, path.value[0].span), parent);
                }
            }
        }
        if symbol.is_none() && names.len() >= 2 {
            let mut found_module = None;
            // Only exact imported module prefixes qualify declarations. An import
            // of a parent does not recursively import its children.
            for split in (1..names.len()).rev() {
                let prefix = names[..split].join("::");
                if let Some(target) = self.imports[file.0].get(&prefix).copied() {
                    found_module = Some((target, split));
                    break;
                }
            }
            if let Some((target, split)) = found_module {
                if target == module {
                    self.error(
                        "PTN_RESOLVE_QUALIFIED_OWN_MODULE",
                        "own-module declarations must use unqualified names",
                        location,
                        None,
                    );
                    return None;
                }
                if let Some(parent) = self.declarations[target.0].get(names[split]).copied() {
                    if !self.out.symbols[parent.0].public {
                        self.error(
                            "PTN_RESOLVE_PRIVATE_SYMBOL",
                            format!("'{}' is private", names[split]),
                            location,
                            self.out.symbols[parent.0].declaration,
                        );
                        return None;
                    }
                    if split + 1 == names.len() {
                        symbol = Some(parent);
                    } else if split + 2 == names.len() {
                        symbol = self
                            .members
                            .get(&parent)
                            .and_then(|members| members.get(names[split + 1]))
                            .copied();
                        if symbol.is_some() {
                            let span = path.value[0].span.join(path.value[split].span);
                            self.reference(Self::location(file, span), parent);
                        }
                    }
                }
            } else {
                let own_prefix = format!("{}::", self.out.modules[module.0].name);
                if path_name(path).starts_with(&own_prefix) {
                    self.error(
                        "PTN_RESOLVE_QUALIFIED_OWN_MODULE",
                        "own-module declarations must use unqualified names",
                        location,
                        None,
                    );
                    return None;
                }
            }
        }
        let Some(symbol) = symbol else {
            self.error(
                "PTN_RESOLVE_UNRESOLVED_NAME",
                format!("unresolved name '{}'", path_name(path)),
                location,
                None,
            );
            return None;
        };
        let kind = self.out.symbols[symbol.0].kind;
        let valid = match expected {
            Expected::Type => matches!(
                kind,
                SymbolKind::BuiltinType
                    | SymbolKind::Struct
                    | SymbolKind::Enum
                    | SymbolKind::TypeParameter
            ),
            Expected::Value => matches!(
                kind,
                SymbolKind::Parameter
                    | SymbolKind::Local
                    | SymbolKind::PatternBinding
                    | SymbolKind::Function
                    | SymbolKind::Variant
                    | SymbolKind::BuiltinVariant
            ),
            Expected::Callable => matches!(
                kind,
                SymbolKind::Function | SymbolKind::Variant | SymbolKind::BuiltinVariant
            ),
            Expected::Record => kind == SymbolKind::Struct,
            Expected::Variant => matches!(kind, SymbolKind::Variant | SymbolKind::BuiltinVariant),
            Expected::Function => kind == SymbolKind::Function,
        };
        self.reference(location, symbol);
        if !valid {
            self.error(
                "PTN_RESOLVE_WRONG_SYMBOL_KIND",
                format!(
                    "'{}' does not name a {}",
                    path_name(path),
                    match expected {
                        Expected::Type => "type",
                        Expected::Value => "value",
                        Expected::Callable => "function or enum constructor",
                        Expected::Record => "record type",
                        Expected::Variant => "variant",
                        Expected::Function => "function",
                    }
                ),
                location,
                self.out.symbols[symbol.0].declaration,
            );
            None
        } else {
            Some(symbol)
        }
    }

    fn block(&mut self, file: FileId, parent: SymbolId, block: &Block, scope: &mut Scope) {
        let previous = scope.locals.clone();
        for statement in &block.statements {
            self.statement(file, parent, statement, scope);
        }
        scope.locals = previous;
    }

    fn bind(
        &mut self,
        file: FileId,
        parent: SymbolId,
        name: &Identifier,
        kind: SymbolKind,
        scope: &mut Scope,
    ) {
        self.child(
            file,
            parent,
            name,
            kind,
            &mut scope.locals,
            "PTN_RESOLVE_DUPLICATE_LOCAL",
        );
    }

    fn local(&mut self, file: FileId, name: &Identifier, scope: &Scope) {
        let location = Self::location(file, name.span);
        if let Some(id) = scope.locals.get(&name.value) {
            self.reference(location, *id);
        } else {
            self.error(
                "PTN_RESOLVE_UNRESOLVED_NAME",
                format!("unresolved binding '{}'", name.value),
                location,
                None,
            );
        }
    }

    fn statement(
        &mut self,
        file: FileId,
        parent: SymbolId,
        statement: &Statement,
        scope: &mut Scope,
    ) {
        match &statement.value {
            StatementKind::Let {
                name, ty, value, ..
            } => {
                if let Some(ty) = ty {
                    self.ty(file, ty, scope, false);
                }
                self.expr(file, parent, value, scope);
                self.bind(file, parent, name, SymbolKind::Local, scope);
            }
            StatementKind::Assign { name, value } => {
                self.local(file, name, scope);
                self.expr(file, parent, value, scope);
            }
            StatementKind::Expression(expr) | StatementKind::Return(Some(expr)) => {
                self.expr(file, parent, expr, scope)
            }
            StatementKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.expr(file, parent, condition, scope);
                self.block(file, parent, then_branch, scope);
                if let Some(branch) = else_branch {
                    self.statement(file, parent, branch, scope);
                }
            }
            StatementKind::While { condition, body } => {
                self.expr(file, parent, condition, scope);
                self.block(file, parent, body, scope);
            }
            StatementKind::Block(block) => self.block(file, parent, block, scope),
            StatementKind::Return(None) | StatementKind::Break | StatementKind::Continue => {}
        }
    }

    fn expr(&mut self, file: FileId, parent: SymbolId, expr: &Expr, scope: &mut Scope) {
        match &expr.kind {
            ExprKind::Bool(_) | ExprKind::Integer(_) | ExprKind::String(_) | ExprKind::Unit => {}
            ExprKind::Result => {
                let location = Self::location(file, expr.span);
                if let Some(id) = scope.result {
                    self.reference(location, id);
                } else {
                    self.error(
                        "PTN_RESOLVE_RESULT_OUTSIDE_POSTCONDITION",
                        "'result' is available only in postconditions",
                        location,
                        None,
                    );
                }
            }
            ExprKind::Path(path) => {
                self.path(file, path, scope, Expected::Value);
            }
            ExprKind::List(values) => {
                for value in values {
                    self.expr(file, parent, value, scope);
                }
            }
            ExprKind::Record { path, fields } => {
                let record = self.path(file, path, scope, Expected::Record);
                for field in fields {
                    if let Some(record) = record
                        && let Some(id) = self
                            .members
                            .get(&record)
                            .and_then(|m| m.get(&field.name.value))
                            .copied()
                    {
                        self.reference(Self::location(file, field.name.span), id);
                    }
                    // Missing/extra/duplicate construction fields are typing checks.
                    self.expr(file, parent, &field.value, scope);
                }
            }
            ExprKind::Parenthesized(inner) | ExprKind::Unary { operand: inner, .. } => {
                self.expr(file, parent, inner, scope)
            }
            ExprKind::Binary { left, right, .. } => {
                self.expr(file, parent, left, scope);
                self.expr(file, parent, right, scope);
            }
            ExprKind::Call { callee, arguments } => {
                if let ExprKind::Path(path) = &callee.kind {
                    self.path(file, path, scope, Expected::Callable);
                } else {
                    self.expr(file, parent, callee, scope);
                }
                for argument in arguments {
                    self.argument(file, parent, argument, scope);
                }
            }
            ExprKind::GenericCall {
                path,
                type_arguments,
                arguments,
            } => {
                self.path(file, path, scope, Expected::Callable);
                for ty in type_arguments {
                    self.ty(file, ty, scope, false);
                }
                for argument in arguments {
                    self.argument(file, parent, argument, scope);
                }
            }
            ExprKind::Field { base, name } => {
                self.expr(file, parent, base, scope);
                self.out.deferred.push(DeferredUse {
                    location: Self::location(file, name.span),
                    name: name.value.clone(),
                    kind: DeferredUseKind::FieldAccess,
                });
            }
            ExprKind::Index { base, index } => {
                self.expr(file, parent, base, scope);
                self.expr(file, parent, index, scope);
            }
            ExprKind::Match { scrutinee, arms } => {
                self.expr(file, parent, scrutinee, scope);
                for arm in arms {
                    let previous = scope.locals.clone();
                    self.pattern(file, parent, &arm.pattern, scope);
                    self.expr(file, parent, &arm.value, scope);
                    scope.locals = previous;
                }
            }
        }
    }

    fn argument(&mut self, file: FileId, parent: SymbolId, argument: &Argument, scope: &mut Scope) {
        match argument {
            Argument::Expression(expr) => self.expr(file, parent, expr, scope),
            Argument::Borrow(name) => self.local(file, &name.value, scope),
        }
    }

    fn pattern(&mut self, file: FileId, parent: SymbolId, pattern: &Pattern, scope: &mut Scope) {
        match &pattern.value {
            PatternKind::Binding(name) => {
                self.bind(file, parent, name, SymbolKind::PatternBinding, scope)
            }
            PatternKind::Variant { path, payload } => {
                self.path(file, path, scope, Expected::Variant);
                for pattern in payload {
                    self.pattern(file, parent, pattern, scope);
                }
            }
            _ => {}
        }
    }
}

fn path_name(path: &Path) -> String {
    path.value
        .iter()
        .map(|part| part.value.as_str())
        .collect::<Vec<_>>()
        .join("::")
}
