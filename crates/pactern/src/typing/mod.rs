//! Core value type checking over parser-produced, package-relative files.
//!
//! [`check`] resolves names first and checks types only if resolution succeeds.
//! Results retain file-aware origins and result-local symbol IDs. This is a
//! library compiler phase, not a protocol envelope or executable/typed core IR.
//! Rank-1 generics use rigid body parameters and fresh call-site inference;
//! a postpass validates declared effects and conservative handle ownership.
//! Execution and verification consume the checked result separately.
//!
//! ```
//! use pactern::resolution::ParsedFile;
//! use pactern::syntax::{FileKind, SourceFile, parse};
//! use pactern::typing::{ValueType, check};
//!
//! let source = SourceFile::new("src/main.ptn",
//!     "module main; fn answer() -> I64 { let n = 42; return n; }");
//! let tree = parse(&source, FileKind::Implementation)?;
//! let checked = check(&[ParsedFile::new(source.name(), tree)]);
//! assert!(checked.is_success());
//! assert!(checked.symbol_types.values().any(|ty| *ty == ValueType::I64));
//! # Ok::<(), pactern::syntax::SyntaxDiagnostic>(())
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::resolution::{
    self, FileId, Location, ParsedFile, RelatedLocation, Resolution, SymbolId, SymbolKind,
};
use crate::syntax::{self as ast, *};

mod coverage;
use coverage::{Constructor, Pattern as CoverPattern};

/// Bounds expanded inferred types, including types assembled across many locals.
pub const MAX_TYPE_DEPTH: usize = 64;
pub const MAX_TYPE_NODES: usize = 1024;

/// An inferred ordinary value type, possibly containing rigid generic parameters.
/// Named and applied types use nominal, resolution-local declaration IDs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ValueType {
    Bool,
    I64,
    String,
    Unit,
    List(Box<ValueType>),
    Option(Box<ValueType>),
    Result(Box<ValueType>, Box<ValueType>),
    Named(SymbolId),
    /// A generic record or enum with its ordered type arguments.
    Applied(SymbolId, Vec<ValueType>),
    /// A rigid parameter inside a generic declaration, never an inference variable.
    Parameter(SymbolId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypeDiagnostic {
    pub code: &'static str,
    pub phase: &'static str,
    pub message: String,
    pub location: Location,
    pub related: Vec<RelatedLocation>,
}

impl fmt::Display for TypeDiagnostic {
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
impl std::error::Error for TypeDiagnostic {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpressionType {
    pub location: Location,
    pub ty: ValueType,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckResult {
    /// Ordered, fully inferred function type arguments at each call path.
    pub call_type_arguments: Vec<(Location, Vec<ValueType>)>,
    pub resolution: Resolution,
    pub diagnostics: Vec<TypeDiagnostic>,
    /// Parameters, locals, fields, pattern bindings, and postcondition results.
    /// Failed or ambiguous types are omitted; always inspect `is_success()`.
    pub symbol_types: BTreeMap<SymbolId, ValueType>,
    pub expression_types: Vec<ExpressionType>,
}

impl CheckResult {
    /// Success establishes value typing, declared effects, and handle ownership.
    pub fn is_success(&self) -> bool {
        self.resolution.is_success() && self.diagnostics.is_empty()
    }
}

/// Check a closed package without filesystem or network access.
///
/// Type diagnostics are empty on resolution failure. Callers should report the
/// resolution diagnostics first. CLI project loading remains a later milestone.
pub fn check(files: &[ParsedFile]) -> CheckResult {
    let resolution = resolution::resolve(files);
    if !resolution.is_success() {
        return CheckResult {
            resolution,
            diagnostics: vec![],
            symbol_types: BTreeMap::new(),
            expression_types: vec![],
            call_type_arguments: vec![],
        };
    }
    let mut checked = Checker::new(files, resolution).run();
    if checked.is_success() {
        checked
            .diagnostics
            .extend(crate::effects::check(files, &checked));
    }
    checked
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Ty {
    Bool,
    I64,
    String,
    Unit,
    List(Box<Ty>),
    Option(Box<Ty>),
    Result(Box<Ty>, Box<Ty>),
    Named(SymbolId),
    Applied(SymbolId, Vec<Ty>),
    Parameter(SymbolId),
    Var(usize),
    Error,
}

#[derive(Clone)]
struct Signature {
    parameters: Vec<Ty>,
    result: Ty,
}

struct PendingMatch {
    span: Span,
    input: Ty,
    patterns: Vec<(Span, CoverPattern)>,
}

struct Checker<'a> {
    files: &'a [ParsedFile],
    out: CheckResult,
    declarations: BTreeMap<(usize, usize, usize), SymbolId>,
    references: BTreeMap<(usize, usize, usize), SymbolId>,
    signatures: BTreeMap<SymbolId, Signature>,
    fields: BTreeMap<SymbolId, BTreeMap<String, (SymbolId, Ty)>>,
    variants: BTreeMap<SymbolId, Vec<(SymbolId, Vec<Ty>)>>,
    generic: BTreeMap<SymbolId, Vec<SymbolId>>,
    types: BTreeMap<SymbolId, Ty>,
    mutable: BTreeSet<SymbolId>,
    variables: Vec<Option<Ty>>,
    expressions: Vec<(Location, Ty)>,
    matches: Vec<PendingMatch>,
    instantiations: Vec<(Location, Ty)>,
    call_arguments: Vec<(Location, Vec<Ty>)>,
    current_function: Option<SymbolId>,
    calls: BTreeMap<SymbolId, BTreeSet<SymbolId>>,
}

fn key(file: FileId, span: Span) -> (usize, usize, usize) {
    (file.0, span.start.byte, span.end.byte)
}

impl<'a> Checker<'a> {
    fn new(files: &'a [ParsedFile], resolution: Resolution) -> Self {
        let declarations = resolution
            .symbols
            .iter()
            .filter_map(|s| s.declaration.map(|l| (key(l.file, l.span), s.id)))
            .collect();
        let references = resolution
            .references
            .iter()
            .map(|r| (key(r.location.file, r.location.span), r.symbol))
            .collect();
        Self {
            files,
            out: CheckResult {
                resolution,
                diagnostics: vec![],
                symbol_types: BTreeMap::new(),
                expression_types: vec![],
                call_type_arguments: vec![],
            },
            declarations,
            references,
            signatures: BTreeMap::new(),
            fields: BTreeMap::new(),
            variants: BTreeMap::new(),
            generic: BTreeMap::new(),
            types: BTreeMap::new(),
            mutable: BTreeSet::new(),
            variables: vec![],
            expressions: vec![],
            matches: vec![],
            instantiations: vec![],
            call_arguments: vec![],
            current_function: None,
            calls: BTreeMap::new(),
        }
    }

    fn declaration(&self, file: FileId, name: &Identifier) -> SymbolId {
        self.declarations[&key(file, name.span)]
    }
    fn reference(&self, file: FileId, span: Span) -> SymbolId {
        self.references[&key(file, span)]
    }
    fn error(
        &mut self,
        file: FileId,
        span: Span,
        code: &'static str,
        message: impl Into<String>,
        related: Option<SymbolId>,
    ) {
        self.out.diagnostics.push(TypeDiagnostic {
            code,
            phase: "type",
            message: message.into(),
            location: Location { file, span },
            related: related
                .and_then(|id| self.out.resolution.symbols[id.0].declaration)
                .into_iter()
                .map(|location| RelatedLocation {
                    location,
                    message: "declared here".into(),
                })
                .collect(),
        });
    }

    fn run(mut self) -> CheckResult {
        // Mark generic declarations before checking forward-referenced signatures.
        for (i, file) in self.files.iter().enumerate() {
            for d in &file.tree.declarations {
                let (name, parameters) = match &d.kind {
                    DeclarationKind::Function(f) => (&f.name, &f.type_parameters),
                    DeclarationKind::Struct(s) => (&s.name, &s.type_parameters),
                    DeclarationKind::Enum(e) => (&e.name, &e.type_parameters),
                    DeclarationKind::Pact(_) => continue,
                };
                if !parameters.is_empty() {
                    let id = self.declaration(FileId(i), name);
                    let ids = parameters
                        .iter()
                        .map(|p| self.declaration(FileId(i), p))
                        .collect();
                    self.generic.insert(id, ids);
                }
            }
        }
        self.collect_signatures();
        for (i, file) in self.files.iter().enumerate() {
            let file_id = FileId(i);
            for d in &file.tree.declarations {
                match &d.kind {
                    DeclarationKind::Function(f) => {
                        let id = self.declaration(file_id, &f.name);
                        self.current_function = Some(id);
                        let result = self.signatures[&id].result.clone();
                        if self.block(file_id, &f.body, &result, 0)
                            && result != Ty::Unit
                            && result != Ty::Error
                        {
                            self.error(
                                file_id,
                                f.body.span,
                                "PTN_TYPE_MISSING_RETURN",
                                "non-Unit function can reach its end without returning",
                                Some(id),
                            );
                        }
                    }
                    DeclarationKind::Pact(p) => {
                        self.current_function = None;
                        let target = self.reference(file_id, p.target.span);
                        let result = self.signatures[&target].result.clone();
                        let results: Vec<_> = self
                            .out
                            .resolution
                            .symbols
                            .iter()
                            .filter(|s| s.kind == SymbolKind::Result && s.parent == Some(target))
                            .map(|s| s.id)
                            .collect();
                        for id in results {
                            self.types.insert(id, result.clone());
                        }
                        for clause in &p.clauses {
                            let (PactClause::Requires(e) | PactClause::Ensures(e)) = &clause.value;
                            self.check_expr(file_id, e, Some(Ty::Bool));
                        }
                    }
                    _ => {}
                }
            }
        }
        self.check_generic_recursion();
        for (id, ty) in &self.types {
            if let Some(ty) = self.value_type(ty) {
                self.out.symbol_types.insert(*id, ty);
            }
        }
        for (location, ty) in &self.expressions {
            if let Some(ty) = self.value_type(ty) {
                self.out.expression_types.push(ExpressionType {
                    location: *location,
                    ty,
                });
            }
        }
        for (location, arguments) in &self.call_arguments {
            if let Some(arguments) = arguments
                .iter()
                .map(|t| self.value_type(t))
                .collect::<Option<Vec<_>>>()
            {
                self.out.call_type_arguments.push((*location, arguments));
            }
        }
        self.out
    }

    fn collect_signatures(&mut self) {
        let handle = self
            .out
            .resolution
            .symbols
            .iter()
            .find(|s| s.name == "host::quota::Txn")
            .unwrap()
            .id;
        for symbol in &self.out.resolution.symbols {
            let (parameters, result) = match symbol.name.as_str() {
                "host::quota::read" => (vec![Ty::Named(handle)], Ty::I64),
                "host::quota::reserve" => (vec![Ty::Named(handle), Ty::I64], Ty::Bool),
                "host::quota::commit" | "host::quota::rollback" => {
                    (vec![Ty::Named(handle)], Ty::Unit)
                }
                _ => continue,
            };
            self.signatures
                .insert(symbol.id, Signature { parameters, result });
        }
        for i in 0..self.files.len() {
            let file = FileId(i);
            for d in &self.files[i].tree.declarations {
                match &d.kind {
                    DeclarationKind::Function(f) => {
                        let id = self.declaration(file, &f.name);
                        let mut parameters = vec![];
                        for p in &f.parameters {
                            let ty = self.annotation(file, &p.ty);
                            if p.borrowed && ty != Ty::Named(handle) {
                                self.error(
                                    file,
                                    p.span,
                                    "PTN_TYPE_UNSUPPORTED_HANDLE",
                                    "borrow applies only to registered handle types",
                                    Some(id),
                                );
                            }
                            self.types
                                .insert(self.declaration(file, &p.name), ty.clone());
                            parameters.push(ty);
                        }
                        let result = self.annotation(file, &f.return_type);
                        self.signatures.insert(id, Signature { parameters, result });
                    }
                    DeclarationKind::Struct(s) => {
                        let id = self.declaration(file, &s.name);
                        let mut fields = BTreeMap::new();
                        for f in &s.fields {
                            let ty = self.annotation(file, &f.ty);
                            let field = self.declaration(file, &f.name);
                            self.types.insert(field, ty.clone());
                            fields.insert(f.name.value.clone(), (field, ty));
                        }
                        self.fields.insert(id, fields);
                    }
                    DeclarationKind::Enum(e) => {
                        let id = self.declaration(file, &e.name);
                        let mut variants = vec![];
                        for v in &e.variants {
                            let variant = self.declaration(file, &v.name);
                            let payload = v
                                .payload
                                .iter()
                                .map(|t| self.annotation(file, t))
                                .collect::<Vec<_>>();
                            self.signatures.insert(
                                variant,
                                Signature {
                                    parameters: payload.clone(),
                                    result: self.named_template(id),
                                },
                            );
                            variants.push((variant, payload));
                        }
                        self.variants.insert(id, variants);
                    }
                    DeclarationKind::Pact(_) => {}
                }
            }
        }
    }

    fn annotation(&mut self, file: FileId, ty: &ast::Type) -> Ty {
        let id = self.reference(file, ty.path.span);
        let symbol = &self.out.resolution.symbols[id.0];
        let name = symbol.name.clone();
        let arity = if symbol.kind == SymbolKind::BuiltinType {
            match name.as_str() {
                "List" | "Option" => 1,
                "Result" => 2,
                _ => 0,
            }
        } else {
            self.generic.get(&id).map_or(0, Vec::len)
        };
        let arguments: Vec<_> = ty
            .arguments
            .iter()
            .map(|a| self.annotation(file, a))
            .collect();
        if arguments.len() != arity {
            self.error(
                file,
                ty.span,
                "PTN_TYPE_ARITY",
                format!(
                    "type {name} expects {arity} type arguments, got {}",
                    arguments.len()
                ),
                Some(id),
            );
            return Ty::Error;
        }
        let span = ty.span;
        let ty = match name.as_str() {
            "Bool" if symbol_kind(&self.out.resolution, id) == SymbolKind::BuiltinType => Ty::Bool,
            "I64" if symbol_kind(&self.out.resolution, id) == SymbolKind::BuiltinType => Ty::I64,
            "String" if symbol_kind(&self.out.resolution, id) == SymbolKind::BuiltinType => {
                Ty::String
            }
            "Unit" if symbol_kind(&self.out.resolution, id) == SymbolKind::BuiltinType => Ty::Unit,
            "List" => Ty::List(Box::new(arguments[0].clone())),
            "Option" => Ty::Option(Box::new(arguments[0].clone())),
            "Result" => Ty::Result(
                Box::new(arguments[0].clone()),
                Box::new(arguments[1].clone()),
            ),
            _ if symbol_kind(&self.out.resolution, id) == SymbolKind::TypeParameter => {
                Ty::Parameter(id)
            }
            _ if !arguments.is_empty() => Ty::Applied(id, arguments),
            _ => Ty::Named(id),
        };
        if !self.bounded(&ty) {
            self.error(
                file,
                span,
                "PTN_TYPE_LIMIT",
                "declared type exceeds the type complexity limit",
                Some(id),
            );
            Ty::Error
        } else {
            ty
        }
    }

    fn named_template(&self, id: SymbolId) -> Ty {
        match self.generic.get(&id) {
            Some(parameters) => {
                Ty::Applied(id, parameters.iter().copied().map(Ty::Parameter).collect())
            }
            None => Ty::Named(id),
        }
    }

    fn substitute(ty: &Ty, substitutions: &BTreeMap<SymbolId, Ty>) -> Ty {
        match ty {
            Ty::Parameter(id) => substitutions.get(id).cloned().unwrap_or_else(|| ty.clone()),
            Ty::List(t) => Ty::List(Box::new(Self::substitute(t, substitutions))),
            Ty::Option(t) => Ty::Option(Box::new(Self::substitute(t, substitutions))),
            Ty::Result(t, e) => Ty::Result(
                Box::new(Self::substitute(t, substitutions)),
                Box::new(Self::substitute(e, substitutions)),
            ),
            Ty::Applied(id, ts) => Ty::Applied(
                *id,
                ts.iter()
                    .map(|t| Self::substitute(t, substitutions))
                    .collect(),
            ),
            _ => ty.clone(),
        }
    }

    fn substitute_signature(sig: &Signature, substitutions: &BTreeMap<SymbolId, Ty>) -> Signature {
        Signature {
            parameters: sig
                .parameters
                .iter()
                .map(|t| Self::substitute(t, substitutions))
                .collect(),
            result: Self::substitute(&sig.result, substitutions),
        }
    }

    fn fresh_substitutions(&mut self, id: SymbolId) -> BTreeMap<SymbolId, Ty> {
        self.generic
            .get(&id)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|p| (p, self.fresh()))
            .collect()
    }

    fn type_substitutions(&self, ty: &Ty) -> BTreeMap<SymbolId, Ty> {
        match self.prune(ty) {
            Ty::Applied(id, ts) => self.generic[&id].iter().copied().zip(ts).collect(),
            _ => BTreeMap::new(),
        }
    }

    fn check_generic_recursion(&mut self) {
        for id in self.generic.keys().copied().collect::<Vec<_>>() {
            if symbol_kind(&self.out.resolution, id) != SymbolKind::Function {
                continue;
            }
            let mut pending: Vec<_> = self.calls.get(&id).into_iter().flatten().copied().collect();
            let mut seen = BTreeSet::new();
            while let Some(next) = pending.pop() {
                if next == id {
                    let location = self.out.resolution.symbols[id.0].declaration.unwrap();
                    self.error(
                        location.file,
                        location.span,
                        "PTN_TYPE_GENERIC_RECURSION",
                        "recursive call cycles involving generic functions are unsupported",
                        Some(id),
                    );
                    break;
                }
                if seen.insert(next) {
                    pending.extend(self.calls.get(&next).into_iter().flatten().copied());
                }
            }
        }
    }

    fn fresh(&mut self) -> Ty {
        let id = self.variables.len();
        self.variables.push(None);
        Ty::Var(id)
    }
    fn prune(&self, ty: &Ty) -> Ty {
        let mut current = ty;
        while let Ty::Var(i) = current {
            match &self.variables[*i] {
                Some(bound) => current = bound,
                None => break,
            }
        }
        current.clone()
    }
    fn occurs(&self, id: usize, ty: &Ty) -> bool {
        match self.prune(ty) {
            Ty::Var(i) => i == id,
            Ty::Applied(_, ts) => ts.iter().any(|t| self.occurs(id, t)),
            Ty::List(t) | Ty::Option(t) => self.occurs(id, &t),
            Ty::Result(t, e) => self.occurs(id, &t) || self.occurs(id, &e),
            _ => false,
        }
    }
    fn unify(&mut self, a: &Ty, b: &Ty) -> bool {
        let (a, b) = (self.prune(a), self.prune(b));
        if a == b || a == Ty::Error || b == Ty::Error {
            return true;
        }
        match (&a, &b) {
            (Ty::Var(i), Ty::Var(j)) => {
                // Keep the oldest variable as the representative so long lists
                // of context-dependent constructors do not build linear chains.
                self.variables[(*i).max(*j)] = Some(Ty::Var((*i).min(*j)));
                true
            }
            (Ty::Var(i), t) | (t, Ty::Var(i)) => {
                if self.occurs(*i, t) {
                    return false;
                }
                self.variables[*i] = Some(t.clone());
                true
            }
            (Ty::Applied(a, xs), Ty::Applied(b, ys)) if a == b && xs.len() == ys.len() => xs
                .iter()
                .zip(ys)
                .fold(true, |ok, (x, y)| self.unify(x, y) && ok),
            (Ty::List(a), Ty::List(b)) | (Ty::Option(a), Ty::Option(b)) => self.unify(a, b),
            (Ty::Result(a, e), Ty::Result(b, f)) => {
                let left = self.unify(a, b);
                self.unify(e, f) && left
            }
            _ => false,
        }
    }
    fn expect(
        &mut self,
        file: FileId,
        span: Span,
        actual: &Ty,
        expected: &Ty,
        related: Option<SymbolId>,
    ) {
        if !self.unify(actual, expected) {
            self.error(
                file,
                span,
                "PTN_TYPE_MISMATCH",
                format!(
                    "expected {}, got {}",
                    self.describe(expected),
                    self.describe(actual)
                ),
                related,
            );
        }
    }
    fn describe(&self, ty: &Ty) -> String {
        if !self.bounded(ty) {
            return "type exceeding the complexity limit".into();
        }
        match self.prune(ty) {
            Ty::Parameter(id) => {
                format!("type parameter {}", self.out.resolution.symbols[id.0].name)
            }
            Ty::Applied(id, ts) => format!(
                "{}<{}>",
                self.out.resolution.symbols[id.0].name,
                ts.iter()
                    .map(|t| self.describe(t))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Ty::Bool => "Bool".into(),
            Ty::I64 => "I64".into(),
            Ty::String => "String".into(),
            Ty::Unit => "Unit".into(),
            Ty::Named(id) => format!(
                "{} (symbol {})",
                self.out.resolution.symbols[id.0].name, id.0
            ),
            Ty::List(t) => format!("List<{}>", self.describe(&t)),
            Ty::Option(t) => format!("Option<{}>", self.describe(&t)),
            Ty::Result(t, e) => format!("Result<{}, {}>", self.describe(&t), self.describe(&e)),
            Ty::Var(_) => "unknown type".into(),
            Ty::Error => "invalid type".into(),
        }
    }
    fn value_type(&self, ty: &Ty) -> Option<ValueType> {
        if !self.bounded(ty) {
            return None;
        }
        Some(match self.prune(ty) {
            Ty::Parameter(id) => ValueType::Parameter(id),
            Ty::Applied(id, ts) => ValueType::Applied(
                id,
                ts.iter()
                    .map(|t| self.value_type(t))
                    .collect::<Option<_>>()?,
            ),
            Ty::Bool => ValueType::Bool,
            Ty::I64 => ValueType::I64,
            Ty::String => ValueType::String,
            Ty::Unit => ValueType::Unit,
            Ty::Named(id) => ValueType::Named(id),
            Ty::List(t) => ValueType::List(Box::new(self.value_type(&t)?)),
            Ty::Option(t) => ValueType::Option(Box::new(self.value_type(&t)?)),
            Ty::Result(t, e) => ValueType::Result(
                Box::new(self.value_type(&t)?),
                Box::new(self.value_type(&e)?),
            ),
            Ty::Var(_) | Ty::Error => return None,
        })
    }
    fn ambiguous(&self, ty: &Ty) -> bool {
        match self.prune(ty) {
            Ty::Var(_) => true,
            Ty::Applied(_, ts) => ts.iter().any(|t| self.ambiguous(t)),
            Ty::List(t) | Ty::Option(t) => self.ambiguous(&t),
            Ty::Result(t, e) => self.ambiguous(&t) || self.ambiguous(&e),
            _ => false,
        }
    }
    fn bounded(&self, ty: &Ty) -> bool {
        let mut pending = vec![(ty, 0)];
        let mut nodes = 0;
        while let Some((mut ty, depth)) = pending.pop() {
            nodes += 1;
            if nodes > MAX_TYPE_NODES || depth > MAX_TYPE_DEPTH {
                return false;
            }
            while let Ty::Var(i) = ty {
                match &self.variables[*i] {
                    Some(bound) => ty = bound,
                    None => break,
                }
            }
            match ty {
                Ty::Applied(_, ts) => pending.extend(ts.iter().map(|t| (t, depth + 1))),
                Ty::List(t) | Ty::Option(t) => pending.push((t, depth + 1)),
                Ty::Result(t, e) => {
                    pending.push((t, depth + 1));
                    pending.push((e, depth + 1));
                }
                _ => {}
            }
        }
        true
    }

    // Complete inference over the entire expression before checking ambiguity
    // and coverage. Later list elements/arms can constrain earlier constructors.
    fn check_expr(&mut self, file: FileId, expr: &Expr, expected: Option<Ty>) -> Ty {
        let start = self.expressions.len();
        let ty = self.expr(file, expr, expected);
        let instantiations = std::mem::take(&mut self.instantiations);
        let ambiguous = self.expressions[start..]
            .iter()
            .chain(&instantiations)
            .find_map(|(location, ty)| {
                (self.bounded(ty) && self.ambiguous(ty)).then_some(location.span)
            });
        if let Some(span) = ambiguous {
            self.error(
                file,
                span,
                "PTN_TYPE_CANNOT_INFER",
                "type is ambiguous; add a type annotation",
                None,
            );
        }
        for pending in std::mem::take(&mut self.matches) {
            if self.value_type(&pending.input).is_some() {
                self.check_coverage(file, pending.span, &pending.input, pending.patterns);
            }
        }
        ty
    }

    /// Returns whether control may fall through. Unreachable statements are still checked.
    fn block(&mut self, file: FileId, block: &Block, result: &Ty, loops: usize) -> bool {
        let mut falls = true;
        for statement in &block.statements {
            let next = self.statement(file, statement, result, loops);
            falls &= next;
        }
        falls
    }
    fn statement(
        &mut self,
        file: FileId,
        statement: &Statement,
        result: &Ty,
        loops: usize,
    ) -> bool {
        match &statement.value {
            StatementKind::Let {
                mutable,
                name,
                ty,
                value,
            } => {
                let expected = ty.as_ref().map(|t| self.annotation(file, t));
                let actual = self.check_expr(file, value, expected);
                let id = self.declaration(file, name);
                self.types.insert(id, actual);
                if *mutable {
                    self.mutable.insert(id);
                }
            }
            StatementKind::Assign { name, value } => {
                let id = self.reference(file, name.span);
                if !self.mutable.contains(&id) {
                    self.error(
                        file,
                        name.span,
                        "PTN_TYPE_IMMUTABLE_ASSIGNMENT",
                        "cannot assign to an immutable binding",
                        Some(id),
                    );
                }
                self.check_expr(file, value, Some(self.types[&id].clone()));
            }
            StatementKind::Expression(e) => {
                self.check_expr(file, e, Some(Ty::Unit));
            }
            StatementKind::Return(e) => {
                if let Some(e) = e {
                    self.check_expr(file, e, Some(result.clone()));
                } else {
                    self.expect(file, statement.span, &Ty::Unit, result, None);
                }
                return false;
            }
            StatementKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.check_expr(file, condition, Some(Ty::Bool));
                let then_falls = self.block(file, then_branch, result, loops);
                let else_falls = else_branch
                    .as_ref()
                    .is_none_or(|e| self.statement(file, e, result, loops));
                return then_falls || else_falls;
            }
            StatementKind::While { condition, body } => {
                self.check_expr(file, condition, Some(Ty::Bool));
                self.block(file, body, result, loops + 1);
            }
            StatementKind::Break | StatementKind::Continue => {
                if loops == 0 {
                    self.error(
                        file,
                        statement.span,
                        "PTN_TYPE_LOOP_CONTROL",
                        "break and continue require an enclosing loop",
                        None,
                    );
                }
                return false;
            }
            StatementKind::Block(b) => return self.block(file, b, result, loops),
        }
        true
    }

    fn integer(&mut self, file: FileId, span: Span, digits: &str, negative: bool) {
        let limit = if negative {
            "9223372036854775808"
        } else {
            "9223372036854775807"
        };
        if digits.len() > limit.len() || (digits.len() == limit.len() && digits > limit) {
            self.error(
                file,
                span,
                "PTN_TYPE_INTEGER_RANGE",
                "integer literal is outside the I64 range",
                None,
            );
        }
    }

    fn expr(&mut self, file: FileId, expr: &Expr, expected: Option<Ty>) -> Ty {
        let mut ty = match &expr.kind {
            ExprKind::Bool(_) => Ty::Bool,
            ExprKind::Integer(digits) => {
                self.integer(file, expr.span, digits, false);
                Ty::I64
            }
            ExprKind::String(_) => Ty::String,
            ExprKind::Unit => Ty::Unit,
            ExprKind::List(values) => {
                let item = self.fresh();
                let ty = Ty::List(Box::new(item.clone()));
                if let Some(e) = &expected {
                    self.expect(file, expr.span, &ty, e, None);
                }
                for value in values {
                    self.expr(file, value, Some(item.clone()));
                }
                ty
            }
            ExprKind::Parenthesized(inner) => self.expr(file, inner, expected.clone()),
            ExprKind::Unary { operator, operand } => {
                let ty = if operator.value == UnaryOp::Not {
                    Ty::Bool
                } else {
                    Ty::I64
                };
                if operator.value == UnaryOp::Negate
                    && let ExprKind::Integer(digits) = &operand.kind
                {
                    self.integer(file, operand.span, digits, true);
                    self.expressions.push((
                        Location {
                            file,
                            span: operand.span,
                        },
                        Ty::I64,
                    ));
                } else {
                    self.expr(file, operand, Some(ty.clone()));
                }
                ty
            }
            ExprKind::Binary {
                left,
                operator,
                right,
            } => {
                use BinaryOp::*;
                match operator.value {
                    And | Or => {
                        self.expr(file, left, Some(Ty::Bool));
                        self.expr(file, right, Some(Ty::Bool));
                        Ty::Bool
                    }
                    Equal | NotEqual => {
                        let a = self.expr(file, left, None);
                        self.expr(file, right, Some(a.clone()));
                        match self.prune(&a) {
                            Ty::Bool | Ty::I64 | Ty::String | Ty::Unit | Ty::Error => {}
                            _ => self.error(file, operator.span, "PTN_TYPE_INVALID_EQUALITY", "equality requires Bool, I64, String, or Unit operands of the same type", None),
                        }
                        Ty::Bool
                    }
                    _ => {
                        self.expr(file, left, Some(Ty::I64));
                        self.expr(file, right, Some(Ty::I64));
                        if matches!(operator.value, Less | LessEqual | Greater | GreaterEqual) {
                            Ty::Bool
                        } else {
                            Ty::I64
                        }
                    }
                }
            }
            ExprKind::Path(path) => {
                let id = self.reference(file, path.span);
                match symbol_kind(&self.out.resolution, id) {
                    SymbolKind::Function => {
                        self.error(
                            file,
                            expr.span,
                            "PTN_TYPE_FUNCTION_VALUE",
                            "function names cannot be stored as values",
                            Some(id),
                        );
                        Ty::Error
                    }
                    SymbolKind::Variant | SymbolKind::BuiltinVariant => {
                        let sig = self.constructor(id);
                        if !sig.parameters.is_empty() {
                            self.error(
                                file,
                                expr.span,
                                "PTN_TYPE_ARITY",
                                "variant payload requires constructor arguments",
                                Some(id),
                            );
                        }
                        sig.result
                    }
                    _ => self.types.get(&id).cloned().unwrap_or(Ty::Error),
                }
            }
            ExprKind::Result => self.types[&self.reference(file, expr.span)].clone(),
            ExprKind::Call { callee, arguments } => {
                if let ExprKind::Path(path) = &callee.kind {
                    self.call(file, path, arguments, expected.clone(), None)
                } else {
                    self.error(
                        file,
                        callee.span,
                        "PTN_TYPE_INVALID_CALLEE",
                        "callee must name a function or enum constructor",
                        None,
                    );
                    self.expr(file, callee, None);
                    for a in arguments {
                        self.argument(file, a, None);
                    }
                    Ty::Error
                }
            }
            ExprKind::GenericCall {
                path,
                type_arguments,
                arguments,
            } => {
                let types = type_arguments
                    .iter()
                    .map(|t| self.annotation(file, t))
                    .collect();
                self.call(file, path, arguments, expected.clone(), Some(types))
            }
            ExprKind::Record { path, fields } => {
                let id = self.reference(file, path.span);
                let substitutions = self.fresh_substitutions(id);
                let record = Self::substitute(&self.named_template(id), &substitutions);
                if let Some(ref expected) = expected {
                    self.expect(file, path.span, &record, expected, Some(id));
                }
                let declared: BTreeMap<_, _> = self.fields[&id]
                    .iter()
                    .map(|(name, (field, ty))| {
                        (name.clone(), (*field, Self::substitute(ty, &substitutions)))
                    })
                    .collect();
                let mut seen = BTreeSet::new();
                for field in fields {
                    let name = &field.name.value;
                    if !seen.insert(name.clone()) {
                        self.error(
                            file,
                            field.name.span,
                            "PTN_TYPE_DUPLICATE_FIELD",
                            format!("field '{name}' appears more than once"),
                            Some(id),
                        );
                    }
                    if let Some((_, ty)) = declared.get(name) {
                        self.expr(file, &field.value, Some(ty.clone()));
                    } else {
                        self.error(
                            file,
                            field.name.span,
                            "PTN_TYPE_UNKNOWN_FIELD",
                            format!("record has no field '{name}'"),
                            Some(id),
                        );
                        self.expr(file, &field.value, None);
                    }
                }
                for name in declared.keys() {
                    if !seen.contains(name) {
                        self.error(
                            file,
                            expr.span,
                            "PTN_TYPE_MISSING_FIELD",
                            format!("missing record field '{name}'"),
                            Some(id),
                        );
                    }
                }
                record
            }
            ExprKind::Field { base, name } => {
                let base = self.expr(file, base, None);
                match self.prune(&base) {
                    Ty::Named(id) | Ty::Applied(id, _) => {
                        if let Some((_, ty)) = self
                            .fields
                            .get(&id)
                            .and_then(|fields| fields.get(&name.value))
                        {
                            let substitutions = self.type_substitutions(&base);
                            Self::substitute(ty, &substitutions)
                        } else {
                            self.error(
                                file,
                                name.span,
                                "PTN_TYPE_UNKNOWN_FIELD",
                                format!("type has no field '{}'", name.value),
                                Some(id),
                            );
                            Ty::Error
                        }
                    }
                    Ty::Error => Ty::Error,
                    _ => {
                        self.error(
                            file,
                            name.span,
                            "PTN_TYPE_UNKNOWN_FIELD",
                            "field access requires a record",
                            None,
                        );
                        Ty::Error
                    }
                }
            }
            ExprKind::Index { base, index } => {
                let base = self.expr(file, base, None);
                self.expr(file, index, Some(Ty::I64));
                match self.prune(&base) {
                    Ty::List(item) => *item,
                    Ty::Error => Ty::Error,
                    _ => {
                        self.error(
                            file,
                            expr.span,
                            "PTN_TYPE_INVALID_INDEX",
                            "only lists can be indexed",
                            None,
                        );
                        Ty::Error
                    }
                }
            }
            ExprKind::Match { scrutinee, arms } => {
                let input = self.expr(file, scrutinee, None);
                let result = expected.clone().unwrap_or_else(|| self.fresh());
                let mut patterns = vec![];
                let before = self.out.diagnostics.len();
                for arm in arms {
                    let pattern = self.pattern(file, &arm.pattern, &input);
                    patterns.push((arm.pattern.span, pattern));
                    self.expr(file, &arm.value, Some(result.clone()));
                }
                if self.out.diagnostics.len() == before {
                    self.matches.push(PendingMatch {
                        span: expr.span,
                        input,
                        patterns,
                    });
                }
                result
            }
        };
        if !self.bounded(&ty) {
            self.error(
                file,
                expr.span,
                "PTN_TYPE_LIMIT",
                "inferred type exceeds the type complexity limit",
                None,
            );
            ty = Ty::Error;
        }
        if let Some(expected) = expected
            && !matches!(
                expr.kind,
                ExprKind::List(_)
                    | ExprKind::Parenthesized(_)
                    | ExprKind::Call { .. }
                    | ExprKind::GenericCall { .. }
            )
        {
            self.expect(file, expr.span, &ty, &expected, None);
        }
        self.expressions.push((
            Location {
                file,
                span: expr.span,
            },
            ty.clone(),
        ));
        ty
    }

    fn constructor(&mut self, id: SymbolId) -> Signature {
        let symbol = &self.out.resolution.symbols[id.0];
        if symbol.kind == SymbolKind::Variant {
            let parent = symbol.parent.unwrap();
            let substitutions = self.fresh_substitutions(parent);
            return Self::substitute_signature(&self.signatures[&id], &substitutions);
        }
        let parent = symbol.parent.unwrap();
        let name = symbol.name.clone();
        let parent_name = self.out.resolution.symbols[parent.0].name.clone();
        let t = self.fresh();
        match (parent_name.as_str(), name.as_str()) {
            ("Option", "Some") => Signature {
                parameters: vec![t.clone()],
                result: Ty::Option(Box::new(t)),
            },
            ("Option", "None") => Signature {
                parameters: vec![],
                result: Ty::Option(Box::new(t)),
            },
            ("Result", "Ok") => {
                let e = self.fresh();
                Signature {
                    parameters: vec![t.clone()],
                    result: Ty::Result(Box::new(t), Box::new(e)),
                }
            }
            ("Result", "Err") => {
                let e = self.fresh();
                Signature {
                    parameters: vec![e.clone()],
                    result: Ty::Result(Box::new(t), Box::new(e)),
                }
            }
            _ => unreachable!("resolver builtin variant registry"),
        }
    }
    fn call(
        &mut self,
        file: FileId,
        path: &Path,
        arguments: &[Argument],
        expected: Option<Ty>,
        explicit: Option<Vec<Ty>>,
    ) -> Ty {
        let id = self.reference(file, path.span);
        let sig = if symbol_kind(&self.out.resolution, id) == SymbolKind::Function {
            if let Some(caller) = self.current_function {
                self.calls.entry(caller).or_default().insert(id);
            }
            let parameters = self.generic.get(&id).cloned().unwrap_or_default();
            let substitutions = if let Some(types) = explicit {
                if types.len() != parameters.len() {
                    self.error(
                        file,
                        path.span,
                        "PTN_TYPE_ARITY",
                        format!(
                            "expected {} type arguments, got {}",
                            parameters.len(),
                            types.len()
                        ),
                        Some(id),
                    );
                }
                parameters
                    .iter()
                    .copied()
                    .enumerate()
                    .map(|(i, p)| (p, types.get(i).cloned().unwrap_or(Ty::Error)))
                    .collect()
            } else {
                self.fresh_substitutions(id)
            };
            self.call_arguments.push((
                Location {
                    file,
                    span: path.span,
                },
                parameters
                    .iter()
                    .map(|p| substitutions[p].clone())
                    .collect(),
            ));
            // Retain unused type arguments so ambiguity cannot disappear from a Unit result.
            for ty in substitutions.values() {
                self.instantiations.push((
                    Location {
                        file,
                        span: path.span,
                    },
                    ty.clone(),
                ));
            }
            Self::substitute_signature(&self.signatures[&id], &substitutions)
        } else {
            let sig = self.constructor(id);
            if let Some(types) = explicit {
                let inferred = match &sig.result {
                    Ty::Applied(_, ts) => ts.clone(),
                    Ty::Option(t) => vec![*t.clone()],
                    Ty::Result(t, e) => vec![*t.clone(), *e.clone()],
                    _ => vec![],
                };
                if types.len() != inferred.len() {
                    self.error(
                        file,
                        path.span,
                        "PTN_TYPE_ARITY",
                        format!(
                            "expected {} type arguments, got {}",
                            inferred.len(),
                            types.len()
                        ),
                        Some(id),
                    );
                }
                for (actual, expected) in inferred.iter().zip(&types) {
                    self.expect(file, path.span, actual, expected, Some(id));
                }
            }
            sig
        };
        if sig.parameters.is_empty()
            && symbol_kind(&self.out.resolution, id) != SymbolKind::Function
        {
            self.error(
                file,
                path.span,
                "PTN_TYPE_UNIT_VARIANT_CALL",
                "unit variants omit parentheses",
                Some(id),
            );
        }
        if let Some(e) = expected {
            self.expect(file, path.span, &sig.result, &e, Some(id));
        }
        if arguments.len() != sig.parameters.len() {
            self.error(
                file,
                path.span,
                "PTN_TYPE_ARITY",
                format!(
                    "expected {} arguments, got {}",
                    sig.parameters.len(),
                    arguments.len()
                ),
                Some(id),
            );
        }
        for (i, a) in arguments.iter().enumerate() {
            self.argument(file, a, sig.parameters.get(i).cloned());
        }
        sig.result
    }
    fn argument(&mut self, file: FileId, argument: &Argument, expected: Option<Ty>) {
        match argument {
            Argument::Expression(e) => {
                self.expr(file, e, expected);
            }
            Argument::Borrow(name) => {
                let id = self.reference(file, name.value.span);
                let actual = self.types.get(&id).cloned().unwrap_or(Ty::Error);
                if let Some(expected) = expected {
                    self.expect(file, name.span, &actual, &expected, Some(id));
                }
            }
        }
    }
    fn pattern(&mut self, file: FileId, pattern: &ast::Pattern, expected: &Ty) -> CoverPattern {
        match &pattern.value {
            PatternKind::Wildcard => CoverPattern::Wild,
            PatternKind::Binding(name) => {
                self.types
                    .insert(self.declaration(file, name), expected.clone());
                CoverPattern::Wild
            }
            PatternKind::Bool(value) => {
                self.expect(file, pattern.span, &Ty::Bool, expected, None);
                CoverPattern::Construct(Constructor::Bool(*value), vec![])
            }
            PatternKind::Integer { negative, digits } => {
                self.integer(file, pattern.span, digits, *negative);
                self.expect(file, pattern.span, &Ty::I64, expected, None);
                let value = if *negative && digits != "0" {
                    format!("-{digits}")
                } else {
                    digits.clone()
                };
                CoverPattern::Construct(Constructor::Integer(value), vec![])
            }
            PatternKind::String(value) => {
                self.expect(file, pattern.span, &Ty::String, expected, None);
                CoverPattern::Construct(Constructor::String(value.clone()), vec![])
            }
            PatternKind::Variant { path, payload } => {
                let id = self.reference(file, path.span);
                let sig = self.constructor(id);
                self.expect(file, path.span, &sig.result, expected, Some(id));
                if payload.len() != sig.parameters.len() {
                    self.error(
                        file,
                        pattern.span,
                        "PTN_TYPE_ARITY",
                        format!(
                            "pattern expects {} payloads, got {}",
                            sig.parameters.len(),
                            payload.len()
                        ),
                        Some(id),
                    );
                }
                let patterns = payload
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        self.pattern(file, p, sig.parameters.get(i).unwrap_or(&Ty::Error))
                    })
                    .collect();
                CoverPattern::Construct(Constructor::Variant(id), patterns)
            }
        }
    }
}

fn symbol_kind(resolution: &Resolution, id: SymbolId) -> SymbolKind {
    resolution.symbols[id.0].kind
}
