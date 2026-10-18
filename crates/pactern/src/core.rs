//! Typed, structured core IR for the value language.
//!
//! [`lower`] checks parser-produced files before lowering. Programs own their
//! result-local symbol table and expose only shared access: consumers cannot
//! detach annotations from the checked program. This is a generic (not
//! monomorphized) IR. Effects are retained as declarations, not certified.
//!
//! ```
//! use pactern::{core, resolution::ParsedFile, syntax::{parse, FileKind, SourceFile}};
//! let source = SourceFile::new("src/main.ptn", "module main; fn answer() -> I64 { return 42; }");
//! let tree = parse(&source, FileKind::Implementation)?;
//! let program = core::lower(&[ParsedFile::new(source.name(), tree)])?;
//! assert_eq!(program.declarations().len(), 1);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::collections::BTreeMap;

use crate::resolution::{FileId, Location, ParsedFile, Resolution, SymbolId, SymbolKind};
use crate::syntax::{self as ast, Span};
use crate::typing::{self, CheckResult, ValueType};

type Key = (usize, usize, usize);
fn key(location: Location) -> Key {
    (
        location.file.0,
        location.span.start.byte,
        location.span.end.byte,
    )
}

/// A checked package. IDs are meaningful only with this program's resolution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Program {
    resolution: Resolution,
    declarations: Vec<Declaration>,
    symbol_types: BTreeMap<SymbolId, ValueType>,
}
impl Program {
    pub fn resolution(&self) -> &Resolution {
        &self.resolution
    }
    pub fn declarations(&self) -> &[Declaration] {
        &self.declarations
    }
    pub fn symbol_types(&self) -> &BTreeMap<SymbolId, ValueType> {
        &self.symbol_types
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Declaration {
    pub origin: Location,
    pub symbol: SymbolId,
    pub kind: DeclarationKind,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeclarationKind {
    Function {
        type_parameters: Vec<SymbolId>,
        parameters: Vec<SymbolId>,
        result: ValueType,
        /// Omission and an explicit empty bound remain distinct. Effect checking
        /// belongs to a later phase; these names do not confer authority.
        effects: Option<Vec<String>>,
        body: Block,
    },
    Struct {
        type_parameters: Vec<SymbolId>,
        fields: Vec<SymbolId>,
    },
    Enum {
        type_parameters: Vec<SymbolId>,
        variants: Vec<Variant>,
    },
    Pact {
        target: SymbolId,
        clauses: Vec<Clause>,
    },
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Variant {
    pub origin: Location,
    pub symbol: SymbolId,
    pub payload: Vec<ValueType>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Clause {
    pub origin: Location,
    pub requires: bool,
    pub condition: Expression,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Block {
    pub origin: Location,
    pub statements: Vec<Statement>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Statement {
    pub origin: Location,
    pub kind: StatementKind,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StatementKind {
    Let {
        symbol: SymbolId,
        mutable: bool,
        value: Expression,
    },
    Assign {
        symbol: SymbolId,
        value: Expression,
    },
    Expression(Expression),
    Return(Option<Expression>),
    If {
        condition: Expression,
        then_branch: Block,
        else_branch: Option<Box<Statement>>,
    },
    While {
        condition: Expression,
        body: Block,
    },
    Break,
    Continue,
    Block(Block),
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Expression {
    pub origin: Location,
    pub ty: ValueType,
    pub kind: ExpressionKind,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExpressionKind {
    Bool(bool),
    I64(i64),
    String(String),
    Unit,
    List(Vec<Expression>),
    Read(SymbolId),
    /// Also represents nullary constructors, with an empty argument vector.
    Construct {
        variant: SymbolId,
        arguments: Vec<Expression>,
    },
    /// Fields stay in source evaluation order, independent of declaration order.
    Record {
        fields: Vec<(SymbolId, Expression)>,
    },
    Unary {
        operator: ast::UnaryOp,
        operand: Box<Expression>,
    },
    /// And/Or remain short-circuit operations, never eager calls.
    Binary {
        left: Box<Expression>,
        operator: ast::BinaryOp,
        right: Box<Expression>,
    },
    Call {
        function: SymbolId,
        type_arguments: Vec<ValueType>,
        arguments: Vec<Expression>,
    },
    Field {
        base: Box<Expression>,
        field: SymbolId,
    },
    Index {
        base: Box<Expression>,
        index: Box<Expression>,
    },
    Match {
        scrutinee: Box<Expression>,
        arms: Vec<Arm>,
    },
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Arm {
    pub origin: Location,
    pub pattern: Pattern,
    pub value: Expression,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pattern {
    pub origin: Location,
    pub ty: ValueType,
    pub kind: PatternKind,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PatternKind {
    Wildcard,
    Binding(SymbolId),
    Bool(bool),
    I64(i64),
    String(String),
    Variant {
        variant: SymbolId,
        payload: Vec<Pattern>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LowerError {
    /// Resolution or value typing failed. No partial IR is returned.
    Check(Box<CheckResult>),
    /// A compiler invariant failed. This is not a user type diagnostic.
    Invariant {
        location: Location,
        message: &'static str,
    },
}
impl std::fmt::Display for LowerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Check(_) => write!(f, "core lowering requires successful resolution and typing"),
            Self::Invariant { location, message } => {
                write!(f, "core invariant at {location:?}: {message}")
            }
        }
    }
}
impl std::error::Error for LowerError {}
type Result<T> = std::result::Result<T, LowerError>;

/// Lower a closed package of parser-produced files without I/O.
///
/// Checks types, control flow, mutability, pattern coverage and generic rules;
/// then requires complete annotations and resolved references while constructing
/// every IR node. Invalid inputs never produce a partially usable program.
/// As with resolution/typing, manually forged ASTs are outside this API's contract.
pub fn lower(files: &[ParsedFile]) -> Result<Program> {
    let checked = typing::check(files);
    if !checked.is_success() {
        return Err(LowerError::Check(Box::new(checked)));
    }
    let builder = Builder::new(&checked, files)?;
    let mut declarations = vec![];
    for (i, file) in files.iter().enumerate() {
        for declaration in &file.tree.declarations {
            declarations.push(builder.declaration(FileId(i), declaration)?);
        }
    }
    Ok(Program {
        resolution: checked.resolution,
        symbol_types: checked.symbol_types,
        declarations,
    })
}

struct Builder<'a> {
    checked: &'a CheckResult,
    declarations: BTreeMap<Key, SymbolId>,
    references: BTreeMap<Key, SymbolId>,
    expressions: BTreeMap<Key, &'a ValueType>,
    calls: BTreeMap<Key, &'a Vec<ValueType>>,
    members: BTreeMap<(SymbolId, String), SymbolId>,
    variants: BTreeMap<SymbolId, (Vec<SymbolId>, Vec<ValueType>)>,
}
impl<'a> Builder<'a> {
    fn new(checked: &'a CheckResult, files: &[ParsedFile]) -> Result<Self> {
        let mut out = Self {
            checked,
            declarations: checked
                .resolution
                .symbols
                .iter()
                .filter_map(|s| s.declaration.map(|l| (key(l), s.id)))
                .collect(),
            references: checked
                .resolution
                .references
                .iter()
                .map(|r| (key(r.location), r.symbol))
                .collect(),
            expressions: checked
                .expression_types
                .iter()
                .map(|e| (key(e.location), &e.ty))
                .collect(),
            calls: checked
                .call_type_arguments
                .iter()
                .map(|(l, ts)| (key(*l), ts))
                .collect(),
            members: checked
                .resolution
                .symbols
                .iter()
                .filter_map(|s| s.parent.map(|p| ((p, s.name.clone()), s.id)))
                .collect(),
            variants: BTreeMap::new(),
        };
        // Payload templates are needed for typed nested patterns. Any missing
        // template is reported during lowering, rather than indexing/panicking.
        for (i, file) in files.iter().enumerate() {
            let f = FileId(i);
            for d in &file.tree.declarations {
                if let ast::DeclarationKind::Enum(e) = &d.kind {
                    for v in &e.variants {
                        let id = out.decl(f, &v.name)?;
                        let parameters = out.parameters(f, &e.type_parameters)?;
                        let payload = v
                            .payload
                            .iter()
                            .map(|t| out.annotation(f, t))
                            .collect::<Result<Vec<_>>>()?;
                        out.variants.insert(id, (parameters, payload));
                    }
                }
            }
        }
        Ok(out)
    }
    fn origin(&self, file: FileId, span: Span) -> Location {
        Location { file, span }
    }
    fn required<T>(
        &self,
        value: Option<T>,
        file: FileId,
        span: Span,
        message: &'static str,
    ) -> Result<T> {
        value.ok_or(LowerError::Invariant {
            location: self.origin(file, span),
            message,
        })
    }
    fn decl(&self, f: FileId, n: &ast::Identifier) -> Result<SymbolId> {
        self.required(
            self.declarations.get(&key(self.origin(f, n.span))).copied(),
            f,
            n.span,
            "missing declaration",
        )
    }
    fn reference(&self, f: FileId, span: Span) -> Result<SymbolId> {
        self.required(
            self.references.get(&key(self.origin(f, span))).copied(),
            f,
            span,
            "missing reference",
        )
    }
    fn parameters(&self, f: FileId, names: &[ast::Identifier]) -> Result<Vec<SymbolId>> {
        names.iter().map(|n| self.decl(f, n)).collect()
    }
    fn member(&self, f: FileId, parent: SymbolId, n: &ast::Identifier) -> Result<SymbolId> {
        self.required(
            self.members.get(&(parent, n.value.clone())).copied(),
            f,
            n.span,
            "missing field",
        )
    }
    fn nominal(&self, f: FileId, span: Span, ty: &ValueType) -> Result<SymbolId> {
        self.required(
            match ty {
                ValueType::Named(id) | ValueType::Applied(id, _) => Some(*id),
                _ => None,
            },
            f,
            span,
            "expected nominal type",
        )
    }
    fn annotation(&self, f: FileId, t: &ast::Type) -> Result<ValueType> {
        let id = self.reference(f, t.path.span)?;
        let args = t
            .arguments
            .iter()
            .map(|t| self.annotation(f, t))
            .collect::<Result<Vec<_>>>()?;
        let symbol = &self.checked.resolution.symbols[id.0];
        Ok(if symbol.kind == SymbolKind::BuiltinType {
            match (symbol.name.as_str(), args.as_slice()) {
                ("Bool", []) => ValueType::Bool,
                ("I64", []) => ValueType::I64,
                ("String", []) => ValueType::String,
                ("Unit", []) => ValueType::Unit,
                ("List", [t]) => ValueType::List(Box::new(t.clone())),
                ("Option", [t]) => ValueType::Option(Box::new(t.clone())),
                ("Result", [t, e]) => ValueType::Result(Box::new(t.clone()), Box::new(e.clone())),
                _ => return self.required(None, f, t.span, "invalid builtin annotation"),
            }
        } else if symbol.kind == SymbolKind::TypeParameter {
            ValueType::Parameter(id)
        } else if args.is_empty() {
            ValueType::Named(id)
        } else {
            ValueType::Applied(id, args)
        })
    }
    fn declaration(&self, f: FileId, d: &ast::Declaration) -> Result<Declaration> {
        let (name, kind) = match &d.kind {
            ast::DeclarationKind::Function(fun) => (
                &fun.name,
                DeclarationKind::Function {
                    type_parameters: self.parameters(f, &fun.type_parameters)?,
                    parameters: fun
                        .parameters
                        .iter()
                        .map(|p| self.decl(f, &p.name))
                        .collect::<Result<_>>()?,
                    result: self.annotation(f, &fun.return_type)?,
                    effects: fun.effects.as_ref().map(|e| {
                        e.value
                            .iter()
                            .map(|p| {
                                p.value
                                    .iter()
                                    .map(|n| n.value.as_str())
                                    .collect::<Vec<_>>()
                                    .join("::")
                            })
                            .collect()
                    }),
                    body: self.block(f, &fun.body)?,
                },
            ),
            ast::DeclarationKind::Struct(s) => (
                &s.name,
                DeclarationKind::Struct {
                    type_parameters: self.parameters(f, &s.type_parameters)?,
                    fields: s
                        .fields
                        .iter()
                        .map(|field| self.decl(f, &field.name))
                        .collect::<Result<_>>()?,
                },
            ),
            ast::DeclarationKind::Enum(e) => (
                &e.name,
                DeclarationKind::Enum {
                    type_parameters: self.parameters(f, &e.type_parameters)?,
                    variants: e
                        .variants
                        .iter()
                        .map(|v| {
                            Ok(Variant {
                                origin: self.origin(f, v.span),
                                symbol: self.decl(f, &v.name)?,
                                payload: v
                                    .payload
                                    .iter()
                                    .map(|t| self.annotation(f, t))
                                    .collect::<Result<_>>()?,
                            })
                        })
                        .collect::<Result<_>>()?,
                },
            ),
            ast::DeclarationKind::Pact(p) => (
                &p.name,
                DeclarationKind::Pact {
                    target: self.reference(f, p.target.span)?,
                    clauses: p
                        .clauses
                        .iter()
                        .map(|c| {
                            let (requires, e) = match &c.value {
                                ast::PactClause::Requires(e) => (true, e),
                                ast::PactClause::Ensures(e) => (false, e),
                            };
                            Ok(Clause {
                                origin: self.origin(f, c.span),
                                requires,
                                condition: self.expr(f, e)?,
                            })
                        })
                        .collect::<Result<_>>()?,
                },
            ),
        };
        Ok(Declaration {
            origin: self.origin(f, d.span),
            symbol: self.decl(f, name)?,
            kind,
        })
    }
    fn block(&self, f: FileId, block: &ast::Block) -> Result<Block> {
        Ok(Block {
            origin: self.origin(f, block.span),
            statements: block
                .statements
                .iter()
                .map(|s| self.statement(f, s))
                .collect::<Result<_>>()?,
        })
    }
    fn statement(&self, f: FileId, s: &ast::Statement) -> Result<Statement> {
        use ast::StatementKind as A;
        let kind = match &s.value {
            A::Let {
                mutable,
                name,
                value,
                ..
            } => StatementKind::Let {
                symbol: self.decl(f, name)?,
                mutable: *mutable,
                value: self.expr(f, value)?,
            },
            A::Assign { name, value } => StatementKind::Assign {
                symbol: self.reference(f, name.span)?,
                value: self.expr(f, value)?,
            },
            A::Expression(e) => StatementKind::Expression(self.expr(f, e)?),
            A::Return(e) => StatementKind::Return(e.as_ref().map(|e| self.expr(f, e)).transpose()?),
            A::If {
                condition,
                then_branch,
                else_branch,
            } => StatementKind::If {
                condition: self.expr(f, condition)?,
                then_branch: self.block(f, then_branch)?,
                else_branch: else_branch
                    .as_ref()
                    .map(|s| self.statement(f, s).map(Box::new))
                    .transpose()?,
            },
            A::While { condition, body } => StatementKind::While {
                condition: self.expr(f, condition)?,
                body: self.block(f, body)?,
            },
            A::Break => StatementKind::Break,
            A::Continue => StatementKind::Continue,
            A::Block(b) => StatementKind::Block(self.block(f, b)?),
        };
        Ok(Statement {
            origin: self.origin(f, s.span),
            kind,
        })
    }
    fn arguments(&self, f: FileId, args: &[ast::Argument]) -> Result<Vec<Expression>> {
        args.iter()
            .map(|a| match a {
                ast::Argument::Expression(e) => self.expr(f, e),
                ast::Argument::Borrow(b) => {
                    self.required(None, f, b.span, "borrow survived value checking")
                }
            })
            .collect()
    }
    fn call(&self, f: FileId, path: &ast::Path, args: &[ast::Argument]) -> Result<ExpressionKind> {
        let id = self.reference(f, path.span)?;
        let arguments = self.arguments(f, args)?;
        Ok(
            if self.checked.resolution.symbols[id.0].kind == SymbolKind::Function {
                let ts = self.required(
                    self.calls.get(&key(self.origin(f, path.span))),
                    f,
                    path.span,
                    "missing call specialization",
                )?;
                ExpressionKind::Call {
                    function: id,
                    type_arguments: (*ts).clone(),
                    arguments,
                }
            } else {
                ExpressionKind::Construct {
                    variant: id,
                    arguments,
                }
            },
        )
    }
    fn expr(&self, f: FileId, e: &ast::Expr) -> Result<Expression> {
        use ast::ExprKind as A;
        let ty = self
            .required(
                self.expressions.get(&key(self.origin(f, e.span))).copied(),
                f,
                e.span,
                "missing expression type",
            )?
            .clone();
        let kind = match &e.kind {
            A::Bool(b) => ExpressionKind::Bool(*b),
            A::Integer(s) => ExpressionKind::I64(self.integer(f, e.span, s, false)?),
            A::String(s) => ExpressionKind::String(s.clone()),
            A::Unit => ExpressionKind::Unit,
            A::List(es) => {
                ExpressionKind::List(es.iter().map(|e| self.expr(f, e)).collect::<Result<_>>()?)
            }
            A::Result => ExpressionKind::Read(self.reference(f, e.span)?),
            A::Path(p) => {
                let id = self.reference(f, p.span)?;
                match self.checked.resolution.symbols[id.0].kind {
                    SymbolKind::Variant | SymbolKind::BuiltinVariant => ExpressionKind::Construct {
                        variant: id,
                        arguments: vec![],
                    },
                    _ => ExpressionKind::Read(id),
                }
            }
            A::Parenthesized(inner) => self.expr(f, inner)?.kind,
            A::Unary { operator, operand } => {
                if operator.value == ast::UnaryOp::Negate
                    && let A::Integer(digits) = &operand.kind
                {
                    ExpressionKind::I64(self.integer(f, e.span, digits, true)?)
                } else {
                    ExpressionKind::Unary {
                        operator: operator.value,
                        operand: Box::new(self.expr(f, operand)?),
                    }
                }
            }
            A::Binary {
                left,
                operator,
                right,
            } => ExpressionKind::Binary {
                left: Box::new(self.expr(f, left)?),
                operator: operator.value,
                right: Box::new(self.expr(f, right)?),
            },
            A::Call { callee, arguments } => match &callee.kind {
                A::Path(path) => self.call(f, path, arguments)?,
                _ => return self.required(None, f, callee.span, "non-symbol callee"),
            },
            A::GenericCall {
                path, arguments, ..
            } => self.call(f, path, arguments)?,
            A::Record { fields, .. } => {
                let parent = self.nominal(f, e.span, &ty)?;
                ExpressionKind::Record {
                    fields: fields
                        .iter()
                        .map(|v| Ok((self.member(f, parent, &v.name)?, self.expr(f, &v.value)?)))
                        .collect::<Result<_>>()?,
                }
            }
            A::Field { base, name } => {
                let base = self.expr(f, base)?;
                let parent = self.nominal(f, e.span, &base.ty)?;
                ExpressionKind::Field {
                    field: self.member(f, parent, name)?,
                    base: Box::new(base),
                }
            }
            A::Index { base, index } => ExpressionKind::Index {
                base: Box::new(self.expr(f, base)?),
                index: Box::new(self.expr(f, index)?),
            },
            A::Match { scrutinee, arms } => {
                let scrutinee = self.expr(f, scrutinee)?;
                let arms = arms
                    .iter()
                    .map(|a| {
                        Ok(Arm {
                            origin: self.origin(f, a.span),
                            pattern: self.pattern(f, &a.pattern, &scrutinee.ty)?,
                            value: self.expr(f, &a.value)?,
                        })
                    })
                    .collect::<Result<_>>()?;
                ExpressionKind::Match {
                    scrutinee: Box::new(scrutinee),
                    arms,
                }
            }
        };
        Ok(Expression {
            origin: self.origin(f, e.span),
            ty,
            kind,
        })
    }
    fn integer(&self, f: FileId, span: Span, digits: &str, negative: bool) -> Result<i64> {
        let text = if negative {
            format!("-{digits}")
        } else {
            digits.to_owned()
        };
        self.required(text.parse().ok(), f, span, "invalid checked integer")
    }
    fn pattern(&self, f: FileId, p: &ast::Pattern, ty: &ValueType) -> Result<Pattern> {
        use ast::PatternKind as A;
        let kind = match &p.value {
            A::Wildcard => PatternKind::Wildcard,
            A::Binding(n) => PatternKind::Binding(self.decl(f, n)?),
            A::Bool(b) => PatternKind::Bool(*b),
            A::String(s) => PatternKind::String(s.clone()),
            A::Integer { negative, digits } => {
                PatternKind::I64(self.integer(f, p.span, digits, *negative)?)
            }
            A::Variant { path, payload } => {
                let id = self.reference(f, path.span)?;
                let name = &self.checked.resolution.symbols[id.0].name;
                let types = match ty {
                    ValueType::Option(t) if name == "Some" => vec![*t.clone()],
                    ValueType::Option(_) if name == "None" => vec![],
                    ValueType::Result(t, _) if name == "Ok" => vec![*t.clone()],
                    ValueType::Result(_, e) if name == "Err" => vec![*e.clone()],
                    _ => {
                        let (parameters, templates) = self.required(
                            self.variants.get(&id),
                            f,
                            p.span,
                            "missing variant template",
                        )?;
                        let args = match ty {
                            ValueType::Applied(_, args) => args.as_slice(),
                            _ => &[],
                        };
                        let substitutions = parameters
                            .iter()
                            .copied()
                            .zip(args.iter().cloned())
                            .collect();
                        templates
                            .iter()
                            .map(|t| substitute(t, &substitutions))
                            .collect()
                    }
                };
                if payload.len() != types.len() {
                    return self.required(None, f, p.span, "pattern arity mismatch");
                }
                PatternKind::Variant {
                    variant: id,
                    payload: payload
                        .iter()
                        .zip(types)
                        .map(|(p, t)| self.pattern(f, p, &t))
                        .collect::<Result<_>>()?,
                }
            }
        };
        Ok(Pattern {
            origin: self.origin(f, p.span),
            ty: ty.clone(),
            kind,
        })
    }
}
pub(crate) fn substitute(ty: &ValueType, args: &BTreeMap<SymbolId, ValueType>) -> ValueType {
    match ty {
        ValueType::Parameter(id) => args.get(id).cloned().unwrap_or_else(|| ty.clone()),
        ValueType::List(t) => ValueType::List(Box::new(substitute(t, args))),
        ValueType::Option(t) => ValueType::Option(Box::new(substitute(t, args))),
        ValueType::Result(t, e) => {
            ValueType::Result(Box::new(substitute(t, args)), Box::new(substitute(e, args)))
        }
        ValueType::Applied(id, ts) => {
            ValueType::Applied(*id, ts.iter().map(|t| substitute(t, args)).collect())
        }
        _ => ty.clone(),
    }
}
