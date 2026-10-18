use super::source::{Position, SourceFile, Span, SyntaxDiagnostic};

/// Maximum source size accepted by the lexer, measured in UTF-8 bytes.
pub const MAX_SOURCE_BYTES: usize = 1024 * 1024;
/// Maximum emitted token count. The EOF token is included in this budget.
pub const MAX_TOKENS: usize = 100_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Keyword {
    Module,
    Import,
    Pub,
    Fn,
    Struct,
    Enum,
    Effects,
    Let,
    Mut,
    Return,
    If,
    Else,
    While,
    Break,
    Continue,
    Match,
    True,
    False,
    Borrow,
    Pact,
    For,
    Requires,
    Ensures,
    Result,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Symbol {
    ColonColon,
    Arrow,
    FatArrow,
    EqualEqual,
    BangEqual,
    LessEqual,
    GreaterEqual,
    AndAnd,
    OrOr,
    LeftParen,
    RightParen,
    LeftBrace,
    RightBrace,
    LeftBracket,
    RightBracket,
    Comma,
    Colon,
    Semicolon,
    Dot,
    Less,
    Greater,
    Equal,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Bang,
    Underscore,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TokenKind {
    Identifier,
    Integer,
    String,
    Keyword(Keyword),
    Symbol(Symbol),
    Eof,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
    pub text: String,
}

pub fn lex(source: &SourceFile) -> Result<Vec<Token>, SyntaxDiagnostic> {
    if source.text().len() > MAX_SOURCE_BYTES {
        let at = Position {
            byte: 0,
            line: 1,
            column: 1,
        };
        return Err(SyntaxDiagnostic::new(
            "PTN_LEX_LIMIT_EXCEEDED",
            "lex",
            format!("source exceeds the {MAX_SOURCE_BYTES}-byte limit"),
            Span { start: at, end: at },
        ));
    }

    Scanner::new(source).scan()
}

struct Scanner<'a> {
    text: &'a str,
    byte: usize,
    position: Position,
    previous_was_cr: bool,
    tokens: Vec<Token>,
}

impl<'a> Scanner<'a> {
    fn new(source: &'a SourceFile) -> Self {
        Self {
            text: source.text(),
            byte: 0,
            position: Position {
                byte: 0,
                line: 1,
                column: 1,
            },
            previous_was_cr: false,
            tokens: Vec::new(),
        }
    }

    fn scan(mut self) -> Result<Vec<Token>, SyntaxDiagnostic> {
        while self.byte < self.text.len() {
            let ch = self.peek().expect("byte is before end of source");
            if is_whitespace(ch) {
                self.advance();
                continue;
            }
            if self.starts_with("//") {
                self.advance();
                self.advance();
                while let Some(ch) = self.peek() {
                    if ch == '\r' || ch == '\n' {
                        break;
                    }
                    self.advance();
                }
                continue;
            }
            if self.starts_with("/*") {
                self.scan_block_comment()?;
                continue;
            }

            let start = self.position;
            if is_ident_start(ch) {
                self.scan_identifier(start)?;
            } else if ch.is_ascii_digit() {
                self.scan_integer(start)?;
            } else if ch == '"' {
                self.scan_string(start)?;
            } else if let Some((symbol, width)) = self.scan_symbol() {
                for _ in 0..width {
                    self.advance();
                }
                self.push(TokenKind::Symbol(symbol), start, self.position)?;
            } else if !ch.is_ascii() {
                self.advance();
                return Err(self.diagnostic(
                    "PTN_LEX_INVALID_UNICODE_SCALAR",
                    "source contains a Unicode scalar that is not valid here",
                    start,
                    self.position,
                ));
            } else {
                self.advance();
                return Err(self.diagnostic(
                    "PTN_LEX_INVALID_CHARACTER",
                    format!("invalid character {ch:?}"),
                    start,
                    self.position,
                ));
            }
        }

        let at = self.position;
        self.push(TokenKind::Eof, at, at)?;
        Ok(self.tokens)
    }

    fn scan_identifier(&mut self, start: Position) -> Result<(), SyntaxDiagnostic> {
        self.advance();
        while self.peek().is_some_and(is_ident_continue) {
            self.advance();
        }
        let spelling = &self.text[start.byte..self.byte];
        let kind = if spelling == "_" {
            TokenKind::Symbol(Symbol::Underscore)
        } else {
            keyword(spelling)
                .map(TokenKind::Keyword)
                .unwrap_or(TokenKind::Identifier)
        };
        self.push(kind, start, self.position)
    }

    fn scan_integer(&mut self, start: Position) -> Result<(), SyntaxDiagnostic> {
        while self.peek().is_some_and(|ch| ch.is_ascii_digit()) {
            self.advance();
        }
        let spelling = &self.text[start.byte..self.byte];
        if spelling.len() > 1 && spelling.starts_with('0') {
            return Err(self.diagnostic(
                "PTN_LEX_INVALID_INTEGER",
                "integer literals cannot have leading zeroes",
                start,
                self.position,
            ));
        }
        self.push(TokenKind::Integer, start, self.position)
    }

    fn scan_string(&mut self, start: Position) -> Result<(), SyntaxDiagnostic> {
        self.advance(); // opening quote
        loop {
            let Some(ch) = self.peek() else {
                return Err(self.diagnostic(
                    "PTN_LEX_UNTERMINATED_STRING",
                    "unterminated string literal",
                    start,
                    self.position,
                ));
            };
            match ch {
                '"' => {
                    self.advance();
                    return self.push(TokenKind::String, start, self.position);
                }
                '\\' => self.scan_escape()?,
                '\r' | '\n' => {
                    let bad = self.position;
                    self.advance();
                    return Err(self.diagnostic(
                        "PTN_LEX_INVALID_STRING_CHARACTER",
                        "raw line breaks are not allowed in strings",
                        bad,
                        self.position,
                    ));
                }
                c if (c as u32) <= 0x1f => {
                    let bad = self.position;
                    self.advance();
                    return Err(self.diagnostic(
                        "PTN_LEX_INVALID_STRING_CHARACTER",
                        "raw control characters are not allowed in strings",
                        bad,
                        self.position,
                    ));
                }
                _ => {
                    self.advance();
                }
            }
        }
    }

    fn scan_escape(&mut self) -> Result<(), SyntaxDiagnostic> {
        let escape_start = self.position;
        self.advance(); // backslash
        let Some(ch) = self.peek() else {
            return Err(self.diagnostic(
                "PTN_LEX_INVALID_ESCAPE",
                "incomplete string escape",
                escape_start,
                self.position,
            ));
        };
        if matches!(ch, '"' | '\\' | 'n' | 'r' | 't') {
            self.advance();
            return Ok(());
        }
        if ch != 'u' || !self.text[self.byte..].starts_with("u{") {
            self.advance();
            return Err(self.diagnostic(
                "PTN_LEX_INVALID_ESCAPE",
                "unknown string escape",
                escape_start,
                self.position,
            ));
        }

        self.advance(); // u
        self.advance(); // {
        let mut digits = 0;
        let mut scalar: u32 = 0;
        while let Some(ch) = self.peek().filter(|ch| ch.is_ascii_hexdigit()) {
            if digits == 6 {
                self.advance();
                return Err(self.diagnostic(
                    "PTN_LEX_INVALID_ESCAPE",
                    "Unicode escapes allow at most six hexadecimal digits",
                    escape_start,
                    self.position,
                ));
            }
            scalar = scalar * 16 + ch.to_digit(16).expect("hex digit");
            digits += 1;
            self.advance();
        }
        if digits == 0 || self.peek() != Some('}') {
            let end = if self.peek().is_some() {
                self.advance();
                self.position
            } else {
                self.position
            };
            return Err(self.diagnostic(
                "PTN_LEX_INVALID_ESCAPE",
                "expected one to six hexadecimal digits followed by '}'",
                escape_start,
                end,
            ));
        }
        self.advance(); // }
        if char::from_u32(scalar).is_none() {
            return Err(self.diagnostic(
                "PTN_LEX_INVALID_UNICODE_SCALAR",
                "Unicode escape does not denote a Unicode scalar value",
                escape_start,
                self.position,
            ));
        }
        Ok(())
    }

    fn scan_block_comment(&mut self) -> Result<(), SyntaxDiagnostic> {
        let start = self.position;
        self.advance();
        self.advance();
        while self.byte < self.text.len() {
            if self.starts_with("*/") {
                self.advance();
                self.advance();
                return Ok(());
            }
            self.advance();
        }
        Err(self.diagnostic(
            "PTN_LEX_UNTERMINATED_COMMENT",
            "unterminated block comment",
            start,
            self.position,
        ))
    }

    fn scan_symbol(&self) -> Option<(Symbol, usize)> {
        const MULTI: &[(&str, Symbol)] = &[
            ("::", Symbol::ColonColon),
            ("->", Symbol::Arrow),
            ("=>", Symbol::FatArrow),
            ("==", Symbol::EqualEqual),
            ("!=", Symbol::BangEqual),
            ("<=", Symbol::LessEqual),
            (">=", Symbol::GreaterEqual),
            ("&&", Symbol::AndAnd),
            ("||", Symbol::OrOr),
        ];
        for (spelling, symbol) in MULTI {
            if self.starts_with(spelling) {
                return Some((*symbol, spelling.len()));
            }
        }
        Some((
            match self.peek()? {
                '(' => Symbol::LeftParen,
                ')' => Symbol::RightParen,
                '{' => Symbol::LeftBrace,
                '}' => Symbol::RightBrace,
                '[' => Symbol::LeftBracket,
                ']' => Symbol::RightBracket,
                ',' => Symbol::Comma,
                ':' => Symbol::Colon,
                ';' => Symbol::Semicolon,
                '.' => Symbol::Dot,
                '<' => Symbol::Less,
                '>' => Symbol::Greater,
                '=' => Symbol::Equal,
                '+' => Symbol::Plus,
                '-' => Symbol::Minus,
                '*' => Symbol::Star,
                '/' => Symbol::Slash,
                '%' => Symbol::Percent,
                '!' => Symbol::Bang,
                _ => return None,
            },
            1,
        ))
    }

    fn push(
        &mut self,
        kind: TokenKind,
        start: Position,
        end: Position,
    ) -> Result<(), SyntaxDiagnostic> {
        if self.tokens.len() >= MAX_TOKENS {
            return Err(self.diagnostic(
                "PTN_LEX_LIMIT_EXCEEDED",
                format!("token count exceeds the {MAX_TOKENS}-token limit"),
                start,
                end,
            ));
        }
        self.tokens.push(Token {
            kind,
            span: Span { start, end },
            text: self.text[start.byte..end.byte].to_owned(),
        });
        Ok(())
    }

    fn diagnostic(
        &self,
        code: &'static str,
        message: impl Into<String>,
        start: Position,
        end: Position,
    ) -> SyntaxDiagnostic {
        SyntaxDiagnostic::new(code, "lex", message, Span { start, end })
    }

    fn starts_with(&self, value: &str) -> bool {
        self.text[self.byte..].starts_with(value)
    }

    fn peek(&self) -> Option<char> {
        self.text[self.byte..].chars().next()
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.byte += ch.len_utf8();
        match ch {
            '\r' => {
                self.position.line += 1;
                self.position.column = 1;
                self.previous_was_cr = true;
            }
            '\n' => {
                if !self.previous_was_cr {
                    self.position.line += 1;
                    self.position.column = 1;
                }
                self.previous_was_cr = false;
            }
            _ => {
                self.position.column += 1;
                self.previous_was_cr = false;
            }
        }
        self.position.byte = self.byte;
        Some(ch)
    }
}

fn is_whitespace(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '\r' | '\n')
}

fn is_ident_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}

fn is_ident_continue(ch: char) -> bool {
    is_ident_start(ch) || ch.is_ascii_digit()
}

fn keyword(value: &str) -> Option<Keyword> {
    Some(match value {
        "module" => Keyword::Module,
        "import" => Keyword::Import,
        "pub" => Keyword::Pub,
        "fn" => Keyword::Fn,
        "struct" => Keyword::Struct,
        "enum" => Keyword::Enum,
        "effects" => Keyword::Effects,
        "let" => Keyword::Let,
        "mut" => Keyword::Mut,
        "return" => Keyword::Return,
        "if" => Keyword::If,
        "else" => Keyword::Else,
        "while" => Keyword::While,
        "break" => Keyword::Break,
        "continue" => Keyword::Continue,
        "match" => Keyword::Match,
        "true" => Keyword::True,
        "false" => Keyword::False,
        "borrow" => Keyword::Borrow,
        "pact" => Keyword::Pact,
        "for" => Keyword::For,
        "requires" => Keyword::Requires,
        "ensures" => Keyword::Ensures,
        "result" => Keyword::Result,
        _ => return None,
    })
}

/// Decodes the exact spelling of a string token that has already passed lexing.
pub(crate) fn decode_string(raw: &str) -> String {
    let body = raw
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .expect("raw string token must include quotes");
    let mut output = String::new();
    let mut chars = body.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            output.push(ch);
            continue;
        }
        match chars
            .next()
            .expect("validated escape has a following character")
        {
            '"' => output.push('"'),
            '\\' => output.push('\\'),
            'n' => output.push('\n'),
            'r' => output.push('\r'),
            't' => output.push('\t'),
            'u' => {
                assert_eq!(
                    chars.next(),
                    Some('{'),
                    "validated Unicode escape has an opening brace"
                );
                let mut value = 0u32;
                loop {
                    let digit = chars.next().expect("validated Unicode escape is closed");
                    if digit == '}' {
                        break;
                    }
                    value = value * 16 + digit.to_digit(16).expect("validated hexadecimal digit");
                }
                output.push(char::from_u32(value).expect("validated Unicode scalar"));
            }
            _ => unreachable!("lexer validated string escape"),
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex_text(text: &str) -> Result<Vec<Token>, SyntaxDiagnostic> {
        lex(&SourceFile::new("test", text))
    }

    #[test]
    fn lexes_keywords_identifiers_symbols_and_preserves_spelling() {
        let tokens = lex_text("module M::Thing; fn f<T>(x: T) -> T { x == true }").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::Keyword(Keyword::Module));
        assert_eq!(tokens[1].kind, TokenKind::Identifier);
        assert_eq!(tokens[2].kind, TokenKind::Symbol(Symbol::ColonColon));
        assert_eq!(tokens[4].text, ";");
        assert_eq!(tokens.last().unwrap().kind, TokenKind::Eof);
        assert_eq!(
            tokens.last().unwrap().span.start,
            tokens.last().unwrap().span.end
        );
    }

    #[test]
    fn maximal_munch_reserved_names_and_wildcard() {
        let tokens = lex_text("moduleX resultx _ _name").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::Identifier);
        assert_eq!(tokens[1].kind, TokenKind::Identifier);
        assert_eq!(tokens[2].kind, TokenKind::Symbol(Symbol::Underscore));
        assert_eq!(tokens[3].kind, TokenKind::Identifier);
    }

    #[test]
    fn validates_integer_and_string_rules() {
        assert_eq!(lex_text("01").unwrap_err().code, "PTN_LEX_INVALID_INTEGER");
        assert_eq!(
            lex_text("\"a\\u{1F642}\"").unwrap()[0].text,
            "\"a\\u{1F642}\""
        );
        assert_eq!(decode_string("\"a\\u{1F642}\\n\""), "a🙂\n");
        assert_eq!(
            lex_text("\"\\u{D800}\"").unwrap_err().code,
            "PTN_LEX_INVALID_UNICODE_SCALAR"
        );
        assert_eq!(
            lex_text("\"\\q\"").unwrap_err().code,
            "PTN_LEX_INVALID_ESCAPE"
        );
        assert_eq!(
            lex_text("\"raw\nline\"").unwrap_err().code,
            "PTN_LEX_INVALID_STRING_CHARACTER"
        );
        assert_eq!(
            lex_text("\"open").unwrap_err().code,
            "PTN_LEX_UNTERMINATED_STRING"
        );
    }

    #[test]
    fn handles_comments_and_tracks_crlf_in_token_spans() {
        let tokens = lex_text("/* non /* nesting */\r\nfn // tail\rname").unwrap();
        assert_eq!(tokens[0].span.start.line, 2);
        assert_eq!(tokens[1].span.start.line, 3);
        assert_eq!(
            lex_text("/* missing").unwrap_err().code,
            "PTN_LEX_UNTERMINATED_COMMENT"
        );
    }

    #[test]
    fn bom_is_rejected_as_source_but_unicode_is_allowed_in_strings_and_comments() {
        assert_eq!(
            lex_text("\u{feff}").unwrap_err().code,
            "PTN_LEX_INVALID_UNICODE_SCALAR"
        );
        assert!(lex_text("// \u{feff}\nname").is_ok());
        assert!(lex_text("/* \u{feff} */ name").is_ok());
        let literal = lex_text("\"\u{feff}\"").unwrap();
        let escaped = lex_text("\"\\u{FEFF}\"").unwrap();
        assert_eq!(
            decode_string(&literal[0].text),
            decode_string(&escaped[0].text)
        );
        assert_eq!(
            lex_text("café").unwrap_err().code,
            "PTN_LEX_INVALID_UNICODE_SCALAR"
        );
    }

    #[test]
    fn enforces_source_and_token_limits() {
        let oversized = SourceFile::new("test", " ".repeat(MAX_SOURCE_BYTES + 1));
        assert_eq!(lex(&oversized).unwrap_err().code, "PTN_LEX_LIMIT_EXCEEDED");
        let many_tokens = "a ".repeat(MAX_TOKENS);
        assert_eq!(
            lex_text(&many_tokens).unwrap_err().code,
            "PTN_LEX_LIMIT_EXCEEDED"
        );
    }
}
