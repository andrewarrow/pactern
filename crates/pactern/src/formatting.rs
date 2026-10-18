//! Deterministic, syntax-only formatting that preserves tokens and comments.
//!
//! Layout uses four-space indentation and LF line endings. Comment contents and
//! literal spellings are copied verbatim, so embedded comment line endings are
//! deliberately left alone. Invalid input is returned as a syntax diagnostic.
//! Formatting also rejects output that would exceed the lexer source-size limit.

use crate::syntax::{self, *};
use std::collections::HashSet;

/// Format a complete implementation or contract without changing its tokens.
pub fn format(source: &SourceFile, kind: FileKind) -> Result<String, SyntaxDiagnostic> {
    let tree = syntax::parse(source, kind)?;
    let tokens = syntax::lex(source)?;
    let mut binary = HashSet::new();
    for declaration in &tree.declarations {
        match &declaration.kind {
            DeclarationKind::Function(function) => collect_block(&function.body, &mut binary),
            DeclarationKind::Pact(pact) => {
                for clause in &pact.clauses {
                    let (PactClause::Requires(expr) | PactClause::Ensures(expr)) = &clause.value;
                    collect_expr(expr, &mut binary);
                }
            }
            _ => {}
        }
    }

    let mut output = Writer::default();
    let mut frames: Vec<Frame> = Vec::new();
    let mut indent = 0;
    let mut previous_end = 0;
    let mut break_after = false;
    let mut closed_block = false;
    for (index, token) in tokens.iter().enumerate() {
        let previous = index.checked_sub(1).map(|index| &tokens[index]);
        let symbol = symbol(token);
        let closing_block = symbol == Some(Symbol::RightBrace)
            && frames.last().is_some_and(|frame| frame.multiline);
        let empty_braces = symbol == Some(Symbol::RightBrace)
            && previous.is_some_and(|token| self::symbol(token) == Some(Symbol::LeftBrace));
        let token_indent = indent - usize::from(closing_block);
        let newline = (break_after && !empty_braces)
            || (closing_block && !empty_braces)
            || (closed_block && !continues_block(token, &binary));
        let space = previous.is_some_and(|previous| needs_space(previous, token, &binary));
        output.gap(
            &source.text()[previous_end..token.span.start.byte],
            indent,
            newline || token.kind == TokenKind::Eof,
            space,
        );
        if token.kind == TokenKind::Eof {
            break;
        }
        output.push(&token.text, token_indent);
        if output.text.len() > MAX_SOURCE_BYTES {
            return Err(output_limit(tree.span));
        }
        previous_end = token.span.end.byte;
        break_after = false;
        closed_block = false;
        match symbol {
            Some(Symbol::LeftBrace) => {
                let multiline = !previous
                    .is_some_and(|token| token.kind == TokenKind::Keyword(Keyword::Effects));
                frames.push(Frame { multiline });
                indent += usize::from(multiline);
                break_after = multiline;
            }
            Some(Symbol::LeftParen | Symbol::LeftBracket) => {
                frames.push(Frame { multiline: false });
            }
            Some(Symbol::Less) if !binary.contains(&token.span.start.byte) => {
                frames.push(Frame { multiline: false });
            }
            Some(Symbol::RightBrace | Symbol::RightParen | Symbol::RightBracket) => {
                let frame = frames.pop().expect("parsed delimiters are balanced");
                indent -= usize::from(frame.multiline);
                closed_block = frame.multiline;
            }
            Some(Symbol::Greater) if !binary.contains(&token.span.start.byte) => {
                frames.pop().expect("parsed type arguments are balanced");
            }
            Some(Symbol::Semicolon) => break_after = true,
            Some(Symbol::Comma) => {
                break_after = frames.last().is_some_and(|frame| frame.multiline);
            }
            _ => {}
        }
    }
    output.newline();
    if output.text.len() > MAX_SOURCE_BYTES {
        return Err(output_limit(tree.span));
    }
    Ok(output.text)
}

fn output_limit(span: Span) -> SyntaxDiagnostic {
    SyntaxDiagnostic::new(
        "PTN_TOOL_FORMAT_LIMIT",
        "tool",
        format!("formatted source exceeds the {MAX_SOURCE_BYTES}-byte limit"),
        span,
    )
}

struct Frame {
    multiline: bool,
}

fn symbol(token: &Token) -> Option<Symbol> {
    match token.kind {
        TokenKind::Symbol(symbol) => Some(symbol),
        _ => None,
    }
}

fn continues_block(token: &Token, binary: &HashSet<usize>) -> bool {
    binary.contains(&token.span.start.byte)
        || token.kind == TokenKind::Eof
        || token.kind == TokenKind::Keyword(Keyword::Else)
        || matches!(
            symbol(token),
            Some(
                Symbol::Semicolon
                    | Symbol::Comma
                    | Symbol::RightParen
                    | Symbol::RightBracket
                    | Symbol::RightBrace
                    | Symbol::Dot
                    | Symbol::LeftParen
                    | Symbol::LeftBracket
            )
        )
}

fn needs_space(previous: &Token, current: &Token, binary: &HashSet<usize>) -> bool {
    use Symbol::*;
    let left = symbol(previous);
    let right = symbol(current);
    // Never let whitespace removal create a different maximal-munch token or
    // turn two operators into a comment opener.
    if matches!(
        (previous.text.as_str(), current.text.as_str()),
        (":", ":")
            | ("-", ">")
            | ("=", ">")
            | ("=", "=")
            | ("!", "=")
            | ("<", "=")
            | (">", "=")
            | ("/", "/")
            | ("/", "*")
    ) {
        return true;
    }
    if matches!(
        right,
        Some(Comma | Semicolon | RightParen | RightBracket | Dot | ColonColon | Colon)
    ) || matches!(left, Some(LeftParen | LeftBracket | Dot | ColonColon))
    {
        return false;
    }
    if binary.contains(&previous.span.start.byte) || binary.contains(&current.span.start.byte) {
        return true;
    }
    if matches!(right, Some(Less | Greater)) || left == Some(Less) {
        return false;
    }
    if matches!(left, Some(Bang | Minus)) {
        return false;
    }
    if left == Some(LeftBrace) && right == Some(RightBrace) {
        return false;
    }
    if right == Some(LeftParen) {
        return !matches!(previous.kind, TokenKind::Identifier)
            && !matches!(left, Some(RightParen | RightBracket | RightBrace | Greater));
    }
    if right == Some(LeftBracket) {
        return !matches!(
            previous.kind,
            TokenKind::Identifier | TokenKind::Integer | TokenKind::String
        ) && !matches!(left, Some(RightParen | RightBracket | RightBrace | Greater));
    }
    true
}

#[derive(Default)]
struct Writer {
    text: String,
}

impl Writer {
    fn push(&mut self, text: &str, indent: usize) {
        if self.text.is_empty() || self.text.ends_with('\n') {
            self.text.push_str(&"    ".repeat(indent));
        }
        self.text.push_str(text);
    }

    fn space(&mut self) {
        if !self.text.is_empty() && !self.text.ends_with([' ', '\n', '\r']) {
            self.text.push(' ');
        }
    }

    fn newline(&mut self) {
        if !self.text.is_empty() && !self.text.ends_with('\n') {
            self.text.push('\n');
        }
    }

    fn gap(&mut self, gap: &str, comment_indent: usize, newline: bool, space: bool) {
        let mut remaining = gap;
        let mut has_comment = false;
        let mut line_comment = false;
        while let Some(start) = remaining.find('/') {
            let before = &remaining[..start];
            remaining = &remaining[start..];
            if line_comment || before.contains(['\r', '\n']) {
                self.newline();
            } else {
                self.space();
            }
            let (end, is_line) = if remaining.starts_with("//") {
                (
                    remaining.find(['\r', '\n']).unwrap_or(remaining.len()),
                    true,
                )
            } else {
                // The lexer has already validated every gap as trivia.
                (
                    remaining.find("*/").expect("validated block comment") + 2,
                    false,
                )
            };
            self.push(&remaining[..end], comment_indent);
            remaining = &remaining[end..];
            has_comment = true;
            line_comment = is_line;
        }
        if newline || line_comment || (has_comment && remaining.contains(['\r', '\n'])) {
            self.newline();
        } else if space || has_comment {
            self.space();
        }
    }
}

fn collect_block(block: &Block, binary: &mut HashSet<usize>) {
    for statement in &block.statements {
        collect_statement(statement, binary);
    }
}

fn collect_statement(statement: &Statement, binary: &mut HashSet<usize>) {
    match &statement.value {
        StatementKind::Let { value, .. }
        | StatementKind::Assign { value, .. }
        | StatementKind::Expression(value)
        | StatementKind::Return(Some(value)) => collect_expr(value, binary),
        StatementKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_expr(condition, binary);
            collect_block(then_branch, binary);
            if let Some(branch) = else_branch {
                collect_statement(branch, binary);
            }
        }
        StatementKind::While { condition, body } => {
            collect_expr(condition, binary);
            collect_block(body, binary);
        }
        StatementKind::Block(block) => collect_block(block, binary),
        _ => {}
    }
}

fn collect_expr(expr: &Expr, binary: &mut HashSet<usize>) {
    match &expr.kind {
        ExprKind::Binary {
            left,
            operator,
            right,
        } => {
            binary.insert(operator.span.start.byte);
            collect_expr(left, binary);
            collect_expr(right, binary);
        }
        ExprKind::Parenthesized(expr)
        | ExprKind::Unary { operand: expr, .. }
        | ExprKind::Field { base: expr, .. } => collect_expr(expr, binary),
        ExprKind::Index { base, index } => {
            collect_expr(base, binary);
            collect_expr(index, binary);
        }
        ExprKind::List(items) => {
            for item in items {
                collect_expr(item, binary);
            }
        }
        ExprKind::Record { fields, .. } => {
            for field in fields {
                collect_expr(&field.value, binary);
            }
        }
        ExprKind::Call { callee, arguments } => {
            collect_expr(callee, binary);
            collect_arguments(arguments, binary);
        }
        ExprKind::GenericCall { arguments, .. } => collect_arguments(arguments, binary),
        ExprKind::Match { scrutinee, arms } => {
            collect_expr(scrutinee, binary);
            for arm in arms {
                collect_expr(&arm.value, binary);
            }
        }
        _ => {}
    }
}

fn collect_arguments(arguments: &[Argument], binary: &mut HashSet<usize>) {
    for argument in arguments {
        if let Argument::Expression(expr) = argument {
            collect_expr(expr, binary);
        }
    }
}
