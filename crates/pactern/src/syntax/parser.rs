use super::ast::*;
use super::lexer::{Keyword as K, Symbol as S, Token, TokenKind, decode_string, lex};
use super::{SourceFile, Span, SyntaxDiagnostic};

/// Maximum recursive parser nesting and expression tree depth. Wide lists and
/// blocks are bounded by the token budget rather than this nesting budget.
pub const MAX_NESTING: usize = 64;

/// Parse one implementation or contract file, requiring complete input.
///
/// A syntax tree does not imply that a program resolves or type-checks. The
/// first lexical or syntactic error is returned, with no recovery or partial AST.
pub fn parse(
    source: &SourceFile,
    kind: FileKind,
) -> std::result::Result<SyntaxTree, SyntaxDiagnostic> {
    let tokens = lex(source)?;
    let mut parser = Parser {
        tokens,
        cursor: 0,
        nesting: 0,
    };
    let mut tree = parser.file(kind)?;
    // The file span includes trivia; declaration spans deliberately exclude it.
    tree.span = source
        .span(0, source.text().len())
        .expect("valid source boundaries");
    Ok(tree)
}

type Result<T> = std::result::Result<T, SyntaxDiagnostic>;

struct Parser {
    tokens: Vec<Token>,
    cursor: usize,
    nesting: usize,
}

impl Parser {
    fn current(&self) -> &Token {
        &self.tokens[self.cursor]
    }
    fn previous(&self) -> Span {
        self.tokens[self.cursor - 1].span
    }
    fn at(&self, symbol: S) -> bool {
        self.current().kind == TokenKind::Symbol(symbol)
    }
    fn keyword(&self, keyword: K) -> bool {
        self.current().kind == TokenKind::Keyword(keyword)
    }
    fn eof(&self) -> bool {
        self.current().kind == TokenKind::Eof
    }
    fn bump(&mut self) -> Token {
        let token = self.current().clone();
        if !self.eof() {
            self.cursor += 1;
        }
        token
    }
    fn eat(&mut self, symbol: S) -> bool {
        if self.at(symbol) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn eat_keyword(&mut self, keyword: K) -> bool {
        if self.keyword(keyword) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn unexpected(&self, expected: &str) -> SyntaxDiagnostic {
        // Do not echo arbitrary-length source text into diagnostics.
        SyntaxDiagnostic::new(
            "PTN_PARSE_UNEXPECTED_TOKEN",
            "parse",
            format!("Expected {expected}."),
            self.current().span,
        )
    }
    fn expect(&mut self, symbol: S) -> Result<Span> {
        if self.at(symbol) {
            Ok(self.bump().span)
        } else {
            Err(self.unexpected(&format!("{symbol:?}")))
        }
    }
    fn expect_keyword(&mut self, keyword: K) -> Result<Span> {
        if self.keyword(keyword) {
            Ok(self.bump().span)
        } else {
            Err(self.unexpected(&format!("{keyword:?}")))
        }
    }
    fn identifier(&mut self) -> Result<Identifier> {
        if self.current().kind != TokenKind::Identifier {
            return Err(self.unexpected("an identifier"));
        }
        let token = self.bump();
        Ok(Spanned {
            span: token.span,
            value: token.text,
        })
    }
    fn nested<T>(&mut self, parse: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        if self.nesting >= MAX_NESTING {
            return Err(SyntaxDiagnostic::new(
                "PTN_PARSE_LIMIT_EXCEEDED",
                "parse",
                "Source exceeds the parser nesting limit.",
                self.current().span,
            ));
        }
        self.nesting += 1;
        let result = parse(self);
        self.nesting -= 1;
        result
    }
    /// Parse a comma-separated sequence after its opening delimiter.
    fn sequence<T>(
        &mut self,
        end: S,
        empty: bool,
        mut item: impl FnMut(&mut Self) -> Result<T>,
    ) -> Result<Vec<T>> {
        let mut items = Vec::new();
        if empty && self.eat(end) {
            return Ok(items);
        }
        loop {
            items.push(item(self)?);
            if self.eat(end) {
                break;
            }
            self.expect(S::Comma)?;
            if self.eat(end) {
                break;
            }
        }
        Ok(items)
    }
    fn path(&mut self) -> Result<Path> {
        let first = self.identifier()?;
        let start = first.span;
        let mut segments = vec![first];
        while self.at(S::ColonColon)
            && self
                .tokens
                .get(self.cursor + 1)
                .is_some_and(|t| t.kind == TokenKind::Identifier)
        {
            self.bump();
            segments.push(self.identifier()?);
        }
        Ok(Spanned {
            span: start.join(self.previous()),
            value: segments,
        })
    }
    fn file(&mut self, kind: FileKind) -> Result<SyntaxTree> {
        let start = self.expect_keyword(K::Module)?;
        let path = self.path()?;
        let module = Spanned {
            span: start.join(self.expect(S::Semicolon)?),
            value: path,
        };
        let mut imports = Vec::new();
        while self.keyword(K::Import) {
            let start = self.bump().span;
            let path = self.path()?;
            imports.push(Spanned {
                span: start.join(self.expect(S::Semicolon)?),
                value: path,
            });
        }
        let mut declarations = Vec::new();
        while !self.eof() {
            declarations.push(self.declaration(kind)?);
        }
        if kind == FileKind::Contract && declarations.is_empty() {
            return Err(self.unexpected("at least one pact declaration"));
        }
        Ok(SyntaxTree {
            span: start.join(self.current().span),
            kind,
            module,
            imports,
            declarations,
        })
    }
    fn declaration(&mut self, kind: FileKind) -> Result<Declaration> {
        let start = self.current().span;
        let public = kind == FileKind::Implementation && self.eat_keyword(K::Pub);
        let declaration = if kind == FileKind::Contract {
            self.expect_keyword(K::Pact)?;
            DeclarationKind::Pact(self.pact()?)
        } else if self.eat_keyword(K::Fn) {
            DeclarationKind::Function(Box::new(self.function()?))
        } else if self.eat_keyword(K::Struct) {
            let name = self.identifier()?;
            let type_parameters = self.type_parameters()?;
            self.expect(S::LeftBrace)?;
            let fields = self.sequence(S::RightBrace, true, |p| {
                let name = p.identifier()?;
                p.expect(S::Colon)?;
                let ty = p.ty()?;
                Ok(FieldDeclaration {
                    span: name.span.join(ty.span),
                    name,
                    ty,
                })
            })?;
            DeclarationKind::Struct(Struct {
                name,
                type_parameters,
                fields,
            })
        } else if self.eat_keyword(K::Enum) {
            let name = self.identifier()?;
            let type_parameters = self.type_parameters()?;
            self.expect(S::LeftBrace)?;
            let variants = self.sequence(S::RightBrace, false, |p| {
                let name = p.identifier()?;
                let payload = if p.eat(S::LeftParen) {
                    p.sequence(S::RightParen, false, Self::ty)?
                } else {
                    Vec::new()
                };
                Ok(Variant {
                    span: name.span.join(p.previous()),
                    name,
                    payload,
                })
            })?;
            DeclarationKind::Enum(Enum {
                name,
                type_parameters,
                variants,
            })
        } else {
            return Err(self.unexpected("a function, struct, or enum declaration"));
        };
        Ok(Declaration {
            span: start.join(self.previous()),
            public,
            kind: declaration,
        })
    }
    fn type_parameters(&mut self) -> Result<Vec<Identifier>> {
        if self.eat(S::Less) {
            self.sequence(S::Greater, false, Self::identifier)
        } else {
            Ok(Vec::new())
        }
    }
    fn type_arguments(&mut self) -> Result<Vec<Type>> {
        self.expect(S::Less)?;
        self.sequence(S::Greater, false, Self::ty)
    }
    fn ty(&mut self) -> Result<Type> {
        self.nested(|p| {
            let path = p.path()?;
            let arguments = if p.at(S::Less) {
                p.type_arguments()?
            } else {
                Vec::new()
            };
            Ok(Type {
                span: path.span.join(p.previous()),
                path,
                arguments,
            })
        })
    }
    fn function(&mut self) -> Result<Function> {
        let name = self.identifier()?;
        let type_parameters = self.type_parameters()?;
        self.expect(S::LeftParen)?;
        let parameters = self.sequence(S::RightParen, true, |p| {
            let name = p.identifier()?;
            p.expect(S::Colon)?;
            let borrowed = p.eat_keyword(K::Borrow);
            let ty = p.ty()?;
            Ok(Parameter {
                span: name.span.join(ty.span),
                name,
                borrowed,
                ty,
            })
        })?;
        self.expect(S::Arrow)?;
        let return_type = self.ty()?;
        let effects = if self.keyword(K::Effects) {
            let start = self.bump().span;
            self.expect(S::LeftBrace)?;
            let names = self.sequence(S::RightBrace, true, |p| {
                let first = p.identifier()?;
                let start = first.span;
                let mut segments = vec![first];
                while p.eat(S::Dot) {
                    segments.push(p.identifier()?);
                }
                Ok(Spanned {
                    span: start.join(p.previous()),
                    value: segments,
                })
            })?;
            Some(Spanned {
                span: start.join(self.previous()),
                value: names,
            })
        } else {
            None
        };
        let body = self.block()?;
        Ok(Function {
            name,
            type_parameters,
            parameters,
            return_type,
            effects,
            body,
        })
    }
    fn pact(&mut self) -> Result<Pact> {
        let name = self.identifier()?;
        self.expect_keyword(K::For)?;
        let target = self.path()?;
        self.expect(S::LeftBrace)?;
        let mut clauses = Vec::new();
        loop {
            let start = self.current().span;
            let requires = if self.eat_keyword(K::Requires) {
                true
            } else if self.eat_keyword(K::Ensures) {
                false
            } else {
                return Err(self.unexpected("requires or ensures"));
            };
            let expression = self.expr()?;
            let span = start.join(self.expect(S::Semicolon)?);
            let value = if requires {
                PactClause::Requires(expression)
            } else {
                PactClause::Ensures(expression)
            };
            clauses.push(Spanned { span, value });
            if self.eat(S::RightBrace) {
                break;
            }
        }
        Ok(Pact {
            name,
            target,
            clauses,
        })
    }
    fn block(&mut self) -> Result<Block> {
        self.nested(|p| {
            let start = p.expect(S::LeftBrace)?;
            let mut statements = Vec::new();
            while !p.at(S::RightBrace) {
                if p.eof() {
                    return Err(p.unexpected("a statement or closing brace"));
                }
                statements.push(p.statement()?);
            }
            Ok(Block {
                span: start.join(p.expect(S::RightBrace)?),
                statements,
            })
        })
    }
    fn statement(&mut self) -> Result<Statement> {
        self.nested(|p| p.statement_inner())
    }
    fn statement_inner(&mut self) -> Result<Statement> {
        let start = self.current().span;
        let value = if self.eat_keyword(K::Let) {
            let mutable = self.eat_keyword(K::Mut);
            let name = self.identifier()?;
            let ty = if self.eat(S::Colon) {
                Some(self.ty()?)
            } else {
                None
            };
            self.expect(S::Equal)?;
            let value = self.expr()?;
            self.expect(S::Semicolon)?;
            StatementKind::Let {
                mutable,
                name,
                ty,
                value,
            }
        } else if self.eat_keyword(K::Return) {
            let value = if self.at(S::Semicolon) {
                None
            } else {
                Some(self.expr()?)
            };
            self.expect(S::Semicolon)?;
            StatementKind::Return(value)
        } else if self.eat_keyword(K::If) {
            self.expect(S::LeftParen)?;
            let condition = self.expr()?;
            self.expect(S::RightParen)?;
            let then_branch = self.block()?;
            let else_branch = if self.eat_keyword(K::Else) {
                if !self.at(S::LeftBrace) && !self.keyword(K::If) {
                    return Err(self.unexpected("a block or if statement"));
                }
                Some(Box::new(self.statement()?))
            } else {
                None
            };
            StatementKind::If {
                condition,
                then_branch,
                else_branch,
            }
        } else if self.eat_keyword(K::While) {
            self.expect(S::LeftParen)?;
            let condition = self.expr()?;
            self.expect(S::RightParen)?;
            StatementKind::While {
                condition,
                body: self.block()?,
            }
        } else if self.eat_keyword(K::Break) {
            self.expect(S::Semicolon)?;
            StatementKind::Break
        } else if self.eat_keyword(K::Continue) {
            self.expect(S::Semicolon)?;
            StatementKind::Continue
        } else if self.at(S::LeftBrace) {
            StatementKind::Block(self.block()?)
        } else if self.current().kind == TokenKind::Identifier
            && self
                .tokens
                .get(self.cursor + 1)
                .is_some_and(|t| t.kind == TokenKind::Symbol(S::Equal))
        {
            let name = self.identifier()?;
            self.expect(S::Equal)?;
            let value = self.expr()?;
            self.expect(S::Semicolon)?;
            StatementKind::Assign { name, value }
        } else {
            let value = self.expr()?;
            self.expect(S::Semicolon)?;
            StatementKind::Expression(value)
        };
        Ok(Spanned {
            span: start.join(self.previous()),
            value,
        })
    }
    fn expr(&mut self) -> Result<Expr> {
        self.expr_bp(1)
    }
    fn expr_bp(&mut self, minimum: u8) -> Result<Expr> {
        self.nested(|p| p.expr_inner(minimum))
    }
    fn expr_inner(&mut self, minimum: u8) -> Result<Expr> {
        let start = self.current().span;
        let mut left = if self.at(S::Bang) || self.at(S::Minus) {
            let token = self.bump();
            let value = if token.kind == TokenKind::Symbol(S::Bang) {
                UnaryOp::Not
            } else {
                UnaryOp::Negate
            };
            let operand = self.expr_bp(7)?;
            let span = start.join(operand.span);
            self.expr_node(
                span,
                ExprKind::Unary {
                    operator: Spanned {
                        span: token.span,
                        value,
                    },
                    operand: Box::new(operand),
                },
            )?
        } else {
            self.primary()?
        };
        let mut equality = false;
        let mut ordering = false;
        loop {
            // Postfix binds more tightly than prefix or any binary operator.
            if self.at(S::LeftParen) {
                let arguments = self.arguments()?;
                left = self.expr_node(
                    start.join(self.previous()),
                    ExprKind::Call {
                        callee: Box::new(left),
                        arguments,
                    },
                )?;
                continue;
            }
            if self.eat(S::Dot) {
                let name = self.identifier()?;
                left = self.expr_node(
                    start.join(name.span),
                    ExprKind::Field {
                        base: Box::new(left),
                        name,
                    },
                )?;
                continue;
            }
            if self.eat(S::LeftBracket) {
                let index = self.expr()?;
                let end = self.expect(S::RightBracket)?;
                left = self.expr_node(
                    start.join(end),
                    ExprKind::Index {
                        base: Box::new(left),
                        index: Box::new(index),
                    },
                )?;
                continue;
            }
            let Some((precedence, value)) = self.binary_operator() else {
                break;
            };
            if precedence < minimum {
                break;
            }
            if (precedence == 3 && equality) || (precedence == 4 && ordering) {
                return Err(self.unexpected("parentheses around chained comparisons"));
            }
            equality |= precedence == 3;
            ordering |= precedence == 4;
            let operator = Spanned {
                span: self.bump().span,
                value,
            };
            let right = self.expr_bp(precedence + 1)?;
            let span = left.span.join(right.span);
            left = self.expr_node(
                span,
                ExprKind::Binary {
                    left: Box::new(left),
                    operator,
                    right: Box::new(right),
                },
            )?;
        }
        Ok(left)
    }
    fn binary_operator(&self) -> Option<(u8, BinaryOp)> {
        let TokenKind::Symbol(symbol) = self.current().kind else {
            return None;
        };
        Some(match symbol {
            S::OrOr => (1, BinaryOp::Or),
            S::AndAnd => (2, BinaryOp::And),
            S::EqualEqual => (3, BinaryOp::Equal),
            S::BangEqual => (3, BinaryOp::NotEqual),
            S::Less => (4, BinaryOp::Less),
            S::LessEqual => (4, BinaryOp::LessEqual),
            S::Greater => (4, BinaryOp::Greater),
            S::GreaterEqual => (4, BinaryOp::GreaterEqual),
            S::Plus => (5, BinaryOp::Add),
            S::Minus => (5, BinaryOp::Subtract),
            S::Star => (6, BinaryOp::Multiply),
            S::Slash => (6, BinaryOp::Divide),
            S::Percent => (6, BinaryOp::Remainder),
            _ => return None,
        })
    }
    fn arguments(&mut self) -> Result<Vec<Argument>> {
        self.expect(S::LeftParen)?;
        self.sequence(S::RightParen, true, |p| {
            if p.keyword(K::Borrow) {
                let start = p.bump().span;
                let name = p.identifier()?;
                Ok(Argument::Borrow(Spanned {
                    span: start.join(name.span),
                    value: name,
                }))
            } else {
                Ok(Argument::Expression(p.expr()?))
            }
        })
    }
    fn primary(&mut self) -> Result<Expr> {
        let start = self.current().span;
        let kind = match self.current().kind {
            TokenKind::Integer => ExprKind::Integer(self.bump().text),
            TokenKind::String => ExprKind::String(decode_string(&self.bump().text)),
            TokenKind::Keyword(K::True | K::False) => {
                ExprKind::Bool(self.bump().kind == TokenKind::Keyword(K::True))
            }
            TokenKind::Keyword(K::Result) => {
                self.bump();
                ExprKind::Result
            }
            TokenKind::Identifier => {
                let path = self.path()?;
                if self.eat(S::ColonColon) {
                    let type_arguments = self.type_arguments()?;
                    let arguments = self.arguments()?;
                    ExprKind::GenericCall {
                        path,
                        type_arguments,
                        arguments,
                    }
                } else if self.eat(S::LeftBrace) {
                    let fields = self.sequence(S::RightBrace, true, |p| {
                        let name = p.identifier()?;
                        p.expect(S::Colon)?;
                        let value = p.expr()?;
                        Ok(FieldValue {
                            span: name.span.join(value.span),
                            name,
                            value,
                        })
                    })?;
                    ExprKind::Record { path, fields }
                } else {
                    ExprKind::Path(path)
                }
            }
            TokenKind::Symbol(S::LeftParen) => {
                self.bump();
                if self.eat(S::RightParen) {
                    ExprKind::Unit
                } else {
                    let inner = self.expr()?;
                    self.expect(S::RightParen)?;
                    ExprKind::Parenthesized(Box::new(inner))
                }
            }
            TokenKind::Symbol(S::LeftBracket) => {
                self.bump();
                ExprKind::List(self.sequence(S::RightBracket, true, Self::expr)?)
            }
            TokenKind::Keyword(K::Match) => {
                self.bump();
                self.expect(S::LeftParen)?;
                let scrutinee = Box::new(self.expr()?);
                self.expect(S::RightParen)?;
                self.expect(S::LeftBrace)?;
                let arms = self.sequence(S::RightBrace, false, |p| {
                    let pattern = p.pattern()?;
                    p.expect(S::FatArrow)?;
                    let value = p.expr()?;
                    Ok(MatchArm {
                        span: pattern.span.join(value.span),
                        pattern,
                        value,
                    })
                })?;
                ExprKind::Match { scrutinee, arms }
            }
            _ => return Err(self.unexpected("an expression")),
        };
        self.expr_node(start.join(self.previous()), kind)
    }
    fn pattern(&mut self) -> Result<Pattern> {
        self.nested(|p| {
            let start = p.current().span;
            let value = match p.current().kind {
                TokenKind::Symbol(S::Underscore) => {
                    p.bump();
                    PatternKind::Wildcard
                }
                TokenKind::Keyword(K::True | K::False) => {
                    PatternKind::Bool(p.bump().kind == TokenKind::Keyword(K::True))
                }
                TokenKind::Integer | TokenKind::Symbol(S::Minus) => {
                    let negative = p.eat(S::Minus);
                    if p.current().kind != TokenKind::Integer {
                        return Err(p.unexpected("an integer pattern"));
                    }
                    PatternKind::Integer {
                        negative,
                        digits: p.bump().text,
                    }
                }
                TokenKind::String => PatternKind::String(decode_string(&p.bump().text)),
                TokenKind::Identifier => {
                    let path = p.path()?;
                    if path.value.len() == 1 {
                        PatternKind::Binding(path.value.into_iter().next().expect("nonempty path"))
                    } else {
                        let payload = if p.eat(S::LeftParen) {
                            p.sequence(S::RightParen, false, Self::pattern)?
                        } else {
                            Vec::new()
                        };
                        PatternKind::Variant { path, payload }
                    }
                }
                _ => return Err(p.unexpected("a pattern")),
            };
            Ok(Spanned {
                span: start.join(p.previous()),
                value,
            })
        })
    }
    fn expr_node(&self, span: Span, kind: ExprKind) -> Result<Expr> {
        let argument_depth = |argument: &Argument| match argument {
            Argument::Expression(e) => e.depth,
            Argument::Borrow(_) => 0,
        };
        let child_depth = match &kind {
            ExprKind::Parenthesized(e)
            | ExprKind::Unary { operand: e, .. }
            | ExprKind::Field { base: e, .. } => e.depth,
            ExprKind::Binary { left, right, .. } => left.depth.max(right.depth),
            ExprKind::Index { base, index } => base.depth.max(index.depth),
            ExprKind::List(items) => items.iter().map(|e| e.depth).max().unwrap_or(0),
            ExprKind::Record { fields, .. } => {
                fields.iter().map(|f| f.value.depth).max().unwrap_or(0)
            }
            ExprKind::Call { callee, arguments } => callee
                .depth
                .max(arguments.iter().map(argument_depth).max().unwrap_or(0)),
            ExprKind::GenericCall { arguments, .. } => {
                arguments.iter().map(argument_depth).max().unwrap_or(0)
            }
            ExprKind::Match { scrutinee, arms } => scrutinee
                .depth
                .max(arms.iter().map(|a| a.value.depth).max().unwrap_or(0)),
            _ => 0,
        };
        let depth = child_depth + 1;
        if depth > MAX_NESTING {
            return Err(SyntaxDiagnostic::new(
                "PTN_PARSE_LIMIT_EXCEEDED",
                "parse",
                "Expression exceeds the syntax tree depth limit.",
                span,
            ));
        }
        Ok(Expr { span, kind, depth })
    }
}
