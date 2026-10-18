use super::Span;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Spanned<T> {
    pub span: Span,
    pub value: T,
}

pub type Identifier = Spanned<String>;
pub type Path = Spanned<Vec<Identifier>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileKind {
    Implementation,
    Contract,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyntaxTree {
    pub span: Span,
    pub kind: FileKind,
    pub module: Spanned<Path>,
    pub imports: Vec<Spanned<Path>>,
    pub declarations: Vec<Declaration>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Declaration {
    /// Includes `pub` when present and ends immediately after the closing brace.
    pub span: Span,
    pub public: bool,
    pub kind: DeclarationKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeclarationKind {
    Function(Box<Function>),
    Struct(Struct),
    Enum(Enum),
    Pact(Pact),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Function {
    pub name: Identifier,
    pub type_parameters: Vec<Identifier>,
    pub parameters: Vec<Parameter>,
    pub return_type: Type,
    /// `None` preserves an omitted declaration, as distinct from `effects {}`.
    pub effects: Option<Spanned<Vec<Path>>>,
    pub body: Block,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Parameter {
    pub span: Span,
    pub name: Identifier,
    pub borrowed: bool,
    pub ty: Type,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Type {
    pub span: Span,
    pub path: Path,
    pub arguments: Vec<Type>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Struct {
    pub name: Identifier,
    pub type_parameters: Vec<Identifier>,
    pub fields: Vec<FieldDeclaration>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FieldDeclaration {
    pub span: Span,
    pub name: Identifier,
    pub ty: Type,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Enum {
    pub name: Identifier,
    pub type_parameters: Vec<Identifier>,
    pub variants: Vec<Variant>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Variant {
    pub span: Span,
    pub name: Identifier,
    pub payload: Vec<Type>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pact {
    pub name: Identifier,
    pub target: Path,
    pub clauses: Vec<Spanned<PactClause>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PactClause {
    Requires(Expr),
    Ensures(Expr),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Block {
    pub span: Span,
    pub statements: Vec<Statement>,
}

pub type Statement = Spanned<StatementKind>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StatementKind {
    Let {
        mutable: bool,
        name: Identifier,
        ty: Option<Type>,
        value: Expr,
    },
    Assign {
        name: Identifier,
        value: Expr,
    },
    Expression(Expr),
    Return(Option<Expr>),
    If {
        condition: Expr,
        then_branch: Block,
        else_branch: Option<Box<Statement>>,
    },
    While {
        condition: Expr,
        body: Block,
    },
    Break,
    Continue,
    Block(Block),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Expr {
    pub span: Span,
    pub kind: ExprKind,
    // Bounds even iterative operator/postfix chains and subsequent tree drops.
    pub(crate) depth: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExprKind {
    Bool(bool),
    /// Exact decimal magnitude; range checking belongs to the type checker.
    Integer(String),
    String(String),
    Unit,
    List(Vec<Expr>),
    Record {
        path: Path,
        fields: Vec<FieldValue>,
    },
    Result,
    Path(Path),
    /// Preserved so `-9223372036854775808` and `-(9223372036854775808)` differ.
    Parenthesized(Box<Expr>),
    Unary {
        operator: Spanned<UnaryOp>,
        operand: Box<Expr>,
    },
    Binary {
        left: Box<Expr>,
        operator: Spanned<BinaryOp>,
        right: Box<Expr>,
    },
    Call {
        callee: Box<Expr>,
        arguments: Vec<Argument>,
    },
    GenericCall {
        path: Path,
        type_arguments: Vec<Type>,
        arguments: Vec<Argument>,
    },
    Field {
        base: Box<Expr>,
        name: Identifier,
    },
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
    },
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnaryOp {
    Not,
    Negate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryOp {
    Or,
    And,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FieldValue {
    pub span: Span,
    pub name: Identifier,
    pub value: Expr,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Argument {
    Expression(Expr),
    Borrow(Spanned<Identifier>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MatchArm {
    pub span: Span,
    pub pattern: Pattern,
    pub value: Expr,
}

pub type Pattern = Spanned<PatternKind>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PatternKind {
    Wildcard,
    Binding(Identifier),
    Bool(bool),
    Integer { negative: bool, digits: String },
    String(String),
    Variant { path: Path, payload: Vec<Pattern> },
}
