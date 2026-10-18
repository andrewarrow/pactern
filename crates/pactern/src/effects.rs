//! Declared effect bounds and conservative affine ownership for registered handles.
use crate::{
    resolution::{FileId, Location, ParsedFile, SymbolId, SymbolKind},
    syntax::*,
    typing::{CheckResult, TypeDiagnostic, ValueType},
};
use std::collections::{BTreeMap, BTreeSet};

pub const REGISTRY_VERSION: &str = "quota/0.1.0";
pub const EFFECTS: &[&str] = &[
    "db.read",
    "db.write",
    "fs.read",
    "fs.write",
    "net.request",
    "clock.read",
    "random.read",
    "env.read",
    "ai.infer",
];
pub fn is_handle(checked: &CheckResult, ty: &ValueType) -> bool {
    matches!(ty, ValueType::Named(id) if checked.resolution.symbols[id.0].name == "host::quota::Txn")
}
fn contains_handle(checked: &CheckResult, ty: &ValueType) -> bool {
    is_handle(checked, ty)
        || match ty {
            ValueType::List(t) | ValueType::Option(t) => contains_handle(checked, t),
            ValueType::Result(a, b) => contains_handle(checked, a) || contains_handle(checked, b),
            ValueType::Applied(_, ts) => ts.iter().any(|t| contains_handle(checked, t)),
            _ => false,
        }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ownership {
    Owned,
    Borrowed,
    Moved,
}
type State = BTreeMap<SymbolId, Ownership>;
#[derive(Default)]
struct Signature {
    effects: BTreeSet<String>,
    borrowed: Vec<bool>,
    generic: bool,
}
struct Checker<'a> {
    checked: &'a CheckResult,
    diagnostics: Vec<TypeDiagnostic>,
    signatures: BTreeMap<SymbolId, Signature>,
    references: BTreeMap<(usize, usize, usize), SymbolId>,
    declarations: BTreeMap<(usize, usize, usize), SymbolId>,
    allowed: BTreeSet<String>,
    active: BTreeSet<SymbolId>,
    contract: bool,
}
fn key(f: FileId, s: Span) -> (usize, usize, usize) {
    (f.0, s.start.byte, s.end.byte)
}

/// Called only after successful value typing. All calls are checked, including
/// unreachable branches. Branch joins keep only definitely available ownership;
/// moving an outer handle in a loop is conservatively rejected.
pub fn check(files: &[ParsedFile], checked: &CheckResult) -> Vec<TypeDiagnostic> {
    let mut c = Checker {
        checked,
        diagnostics: vec![],
        signatures: BTreeMap::new(),
        references: checked
            .resolution
            .references
            .iter()
            .map(|r| (key(r.location.file, r.location.span), r.symbol))
            .collect(),
        declarations: checked
            .resolution
            .symbols
            .iter()
            .filter_map(|s| s.declaration.map(|l| (key(l.file, l.span), s.id)))
            .collect(),
        allowed: BTreeSet::new(),
        active: BTreeSet::new(),
        contract: false,
    };
    for s in &checked.resolution.symbols {
        let (effects, borrowed) = match s.name.as_str() {
            "host::quota::read" => (vec!["db.read"], vec![true]),
            "host::quota::reserve" => (vec!["db.write"], vec![true, false]),
            "host::quota::commit" => (vec!["db.write"], vec![false]),
            "host::quota::rollback" => (vec![], vec![false]),
            _ => continue,
        };
        c.signatures.insert(
            s.id,
            Signature {
                effects: effects.into_iter().map(str::to_owned).collect(),
                borrowed,
                generic: false,
            },
        );
    }
    for (i, file) in files.iter().enumerate() {
        let f = FileId(i);
        for d in &file.tree.declarations {
            if let DeclarationKind::Function(fun) = &d.kind {
                let mut effects = BTreeSet::new();
                if let Some(bound) = &fun.effects {
                    for effect in &bound.value {
                        let name = effect
                            .value
                            .iter()
                            .map(|s| s.value.as_str())
                            .collect::<Vec<_>>()
                            .join(".");
                        if !EFFECTS.contains(&name.as_str()) {
                            c.error(
                                f,
                                effect.span,
                                "PTN_EFFECT_UNKNOWN",
                                format!("unknown effect '{name}'"),
                            );
                        }
                        if !effects.insert(name) {
                            c.error(
                                f,
                                effect.span,
                                "PTN_EFFECT_DUPLICATE",
                                "effect bounds cannot contain duplicates",
                            );
                        }
                    }
                }
                let id = c.declarations[&key(f, fun.name.span)];
                c.signatures.insert(
                    id,
                    Signature {
                        effects,
                        borrowed: fun.parameters.iter().map(|p| p.borrowed).collect(),
                        generic: !fun.type_parameters.is_empty(),
                    },
                );
                if c.type_is_handle(f, &fun.return_type) {
                    c.error(
                        f,
                        fun.return_type.span,
                        "PTN_HANDLE_ESCAPE",
                        "handles cannot be returned",
                    );
                }
            }
        }
    }
    for (id, ty) in &checked.symbol_types {
        if contains_handle(checked, ty) {
            let s = &checked.resolution.symbols[id.0];
            if (!is_handle(checked, ty)
                || matches!(
                    s.kind,
                    SymbolKind::Field | SymbolKind::Result | SymbolKind::PatternBinding
                ))
                && let Some(l) = s.declaration
            {
                c.error(
                    l.file,
                    l.span,
                    "PTN_HANDLE_ESCAPE",
                    "handles cannot be stored in aggregates or patterns",
                );
            }
        }
    }
    // Covers enum payloads and nested type annotations even when unused.
    for (i, file) in files.iter().enumerate() {
        let f = FileId(i);
        for d in &file.tree.declarations {
            match &d.kind {
                DeclarationKind::Struct(s) => {
                    for field in &s.fields {
                        if c.type_is_handle(f, &field.ty) {
                            c.error(
                                f,
                                field.span,
                                "PTN_HANDLE_ESCAPE",
                                "handles cannot be record fields",
                            );
                        }
                    }
                }
                DeclarationKind::Enum(e) => {
                    for v in &e.variants {
                        for t in &v.payload {
                            if c.type_is_handle(f, t) {
                                c.error(
                                    f,
                                    t.span,
                                    "PTN_HANDLE_ESCAPE",
                                    "handles cannot be enum payloads",
                                );
                            }
                        }
                    }
                }
                DeclarationKind::Function(fun) => {
                    let id = c.declarations[&key(f, fun.name.span)];
                    c.allowed = c.signatures[&id].effects.clone();
                    c.contract = false;
                    let mut state = State::new();
                    for p in &fun.parameters {
                        let id = c.declarations[&key(f, p.name.span)];
                        if checked
                            .symbol_types
                            .get(&id)
                            .is_some_and(|t| is_handle(checked, t))
                        {
                            state.insert(
                                id,
                                if p.borrowed {
                                    Ownership::Borrowed
                                } else {
                                    Ownership::Owned
                                },
                            );
                        }
                    }
                    c.block(f, &fun.body, &mut state);
                }
                DeclarationKind::Pact(p) => {
                    c.allowed.clear();
                    c.contract = true;
                    for clause in &p.clauses {
                        let (PactClause::Requires(e) | PactClause::Ensures(e)) = &clause.value;
                        c.expr(f, e, &mut State::new());
                    }
                }
            }
        }
    }
    c.diagnostics
}
impl Checker<'_> {
    fn error(&mut self, f: FileId, span: Span, code: &'static str, message: impl Into<String>) {
        self.diagnostics.push(TypeDiagnostic {
            code,
            phase: if code.starts_with("PTN_EFFECT") {
                "effect"
            } else {
                "type"
            },
            message: message.into(),
            location: Location { file: f, span },
            related: vec![],
        });
    }
    fn type_is_handle(&self, f: FileId, t: &Type) -> bool {
        self.references
            .get(&key(f, t.path.span))
            .is_some_and(|id| self.checked.resolution.symbols[id.0].name == "host::quota::Txn")
            || t.arguments.iter().any(|t| self.type_is_handle(f, t))
    }
    fn handle_expr(&self, f: FileId, e: &Expr) -> Option<SymbolId> {
        match &e.kind {
            ExprKind::Path(p) => self.references.get(&key(f, p.span)).copied().filter(|id| {
                self.checked
                    .symbol_types
                    .get(id)
                    .is_some_and(|t| is_handle(self.checked, t))
            }),
            ExprKind::Parenthesized(e) => self.handle_expr(f, e),
            _ => None,
        }
    }
    fn use_handle(&mut self, f: FileId, span: Span, id: SymbolId, state: &mut State, borrow: bool) {
        if self.contract {
            self.error(
                f,
                span,
                "PTN_HANDLE_CONTRACT",
                "contracts cannot inspect handles",
            );
        }
        if self.active.contains(&id) {
            self.error(
                f,
                span,
                "PTN_HANDLE_BORROW_OVERLAP",
                "handle is already borrowed by an earlier argument",
            );
            return;
        }
        match state.get(&id) {
            Some(Ownership::Owned) => {}
            Some(Ownership::Borrowed) if borrow => {}
            Some(Ownership::Borrowed) => {
                self.error(
                    f,
                    span,
                    "PTN_HANDLE_BORROW_MOVE",
                    "cannot move a borrowed handle",
                );
                return;
            }
            _ => {
                self.error(
                    f,
                    span,
                    "PTN_HANDLE_USE_AFTER_MOVE",
                    "handle is unavailable on an incoming path",
                );
                return;
            }
        }
        if borrow {
            self.active.insert(id);
        } else {
            state.insert(id, Ownership::Moved);
        }
    }
    fn block(&mut self, f: FileId, b: &Block, state: &mut State) {
        let outer: BTreeSet<_> = state.keys().copied().collect();
        for s in &b.statements {
            self.statement(f, s, state);
        }
        state.retain(|id, _| outer.contains(id));
    }
    fn statement(&mut self, f: FileId, s: &Statement, state: &mut State) {
        match &s.value {
            StatementKind::Let {
                name,
                mutable,
                value,
                ..
            } => {
                if let Some(source) = self.handle_expr(f, value) {
                    self.use_handle(f, value.span, source, state, false);
                    if *mutable {
                        self.error(
                            f,
                            name.span,
                            "PTN_HANDLE_MUTABLE",
                            "handle bindings cannot be mutable",
                        );
                    }
                    let id = self.declarations[&key(f, name.span)];
                    state.insert(id, Ownership::Owned);
                } else {
                    self.expr(f, value, state);
                }
            }
            StatementKind::Assign { name, value } => {
                if let Some(id) = self.references.get(&key(f, name.span))
                    && state.contains_key(id)
                {
                    self.error(
                        f,
                        name.span,
                        "PTN_HANDLE_MUTABLE",
                        "handles cannot be reassigned",
                    );
                }
                self.expr(f, value, state);
            }
            StatementKind::Expression(e) => self.expr(f, e, state),
            StatementKind::Return(e) => {
                if let Some(e) = e {
                    self.expr(f, e, state);
                }
            }
            StatementKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.expr(f, condition, state);
                let mut left = state.clone();
                let mut right = state.clone();
                self.block(f, then_branch, &mut left);
                if let Some(s) = else_branch {
                    self.statement(f, s, &mut right);
                }
                join(state, &left, &right);
            }
            StatementKind::While { condition, body } => {
                let before = state.clone();
                self.expr(f, condition, state);
                self.block(f, body, state);
                for (id, mode) in &before {
                    if state.get(id) != Some(mode) {
                        self.error(
                            f,
                            s.span,
                            "PTN_HANDLE_LOOP_MOVE",
                            "a loop cannot consume an outer handle",
                        );
                    }
                }
                join(state, &before, &state.clone());
            }
            StatementKind::Block(b) => self.block(f, b, state),
            StatementKind::Break | StatementKind::Continue => {}
        }
    }
    fn expr(&mut self, f: FileId, e: &Expr, state: &mut State) {
        if self.handle_expr(f, e).is_some() {
            self.error(
                f,
                e.span,
                "PTN_HANDLE_VALUE",
                "a handle may only be moved to a binding or passed to a handle parameter",
            );
            return;
        }
        match &e.kind {
            ExprKind::Call { callee, arguments } => {
                if let ExprKind::Path(path) = &callee.kind {
                    self.call(f, path, arguments, state);
                }
            }
            ExprKind::GenericCall {
                path, arguments, ..
            } => self.call(f, path, arguments, state),
            ExprKind::List(es) => {
                for e in es {
                    self.expr(f, e, state);
                }
            }
            ExprKind::Record { fields, .. } => {
                for field in fields {
                    self.expr(f, &field.value, state);
                }
            }
            ExprKind::Parenthesized(e)
            | ExprKind::Unary { operand: e, .. }
            | ExprKind::Field { base: e, .. } => self.expr(f, e, state),
            ExprKind::Binary {
                left,
                operator,
                right,
            } => {
                self.expr(f, left, state);
                if matches!(operator.value, BinaryOp::And | BinaryOp::Or) {
                    let before = state.clone();
                    self.expr(f, right, state);
                    join(state, &before, &state.clone());
                } else {
                    self.expr(f, right, state);
                }
            }
            ExprKind::Index { base, index } => {
                self.expr(f, base, state);
                self.expr(f, index, state);
            }
            ExprKind::Match { scrutinee, arms } => {
                self.expr(f, scrutinee, state);
                let before = state.clone();
                let mut merged = state.clone();
                for arm in arms {
                    let mut branch = before.clone();
                    self.expr(f, &arm.value, &mut branch);
                    let previous = merged.clone();
                    join(&mut merged, &branch, &previous);
                }
                *state = merged;
            }
            _ => {}
        }
    }
    fn call(&mut self, f: FileId, path: &Path, args: &[Argument], state: &mut State) {
        let Some(id) = self.references.get(&key(f, path.span)).copied() else {
            return;
        };
        let (effects, borrowed, generic) = self
            .signatures
            .get(&id)
            .map(|s| (s.effects.clone(), s.borrowed.clone(), s.generic))
            .unwrap_or_default();
        for effect in effects.difference(&self.allowed.clone()) {
            self.error(
                f,
                path.span,
                "PTN_EFFECT_BOUND",
                format!("call requires undeclared effect '{effect}'"),
            );
        }
        let previous = self.active.clone();
        for (i, arg) in args.iter().enumerate() {
            let wants_borrow = borrowed.get(i) == Some(&true);
            match arg {
                Argument::Borrow(name) => {
                    let id = self.references[&key(f, name.value.span)];
                    let handle = self
                        .checked
                        .symbol_types
                        .get(&id)
                        .is_some_and(|t| is_handle(self.checked, t));
                    if !handle || !wants_borrow {
                        self.error(
                            f,
                            name.span,
                            "PTN_HANDLE_BORROW_MODE",
                            "borrow requires a borrowed handle parameter",
                        );
                    }
                    if handle {
                        self.use_handle(f, name.span, id, state, true);
                    }
                }
                Argument::Expression(e) => {
                    if wants_borrow {
                        self.error(
                            f,
                            e.span,
                            "PTN_HANDLE_BORROW_MODE",
                            "borrowed handle argument requires 'borrow name'",
                        );
                    }
                    if let Some(handle) = self.handle_expr(f, e) {
                        if generic || !self.signatures.contains_key(&id) {
                            self.error(
                                f,
                                e.span,
                                "PTN_HANDLE_ESCAPE",
                                "handles cannot instantiate generics or enter aggregates",
                            );
                        }
                        self.use_handle(f, e.span, handle, state, false);
                    } else {
                        self.expr(f, e, state);
                    }
                }
            }
        }
        self.active = previous;
        if self.checked.call_type_arguments.iter().any(|(l, ts)| {
            key(l.file, l.span) == key(f, path.span)
                && ts.iter().any(|t| contains_handle(self.checked, t))
        }) {
            self.error(
                f,
                path.span,
                "PTN_HANDLE_ESCAPE",
                "handles cannot instantiate generic functions",
            );
        }
    }
}
fn join(out: &mut State, left: &State, right: &State) {
    for (id, mode) in out.iter_mut() {
        if left.get(id) != right.get(id) || left.get(id) == Some(&Ownership::Moved) {
            *mode = Ownership::Moved;
        }
    }
}
