use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// A zero-based UTF-8 byte offset and one-based line and Unicode scalar column.
/// CRLF, lone CR, and lone LF each advance the line once.
pub struct Position {
    pub byte: usize,
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// A half-open source range, with both endpoints expressed as [`Position`]s.
pub struct Span {
    pub start: Position,
    pub end: Position,
}

impl Span {
    pub fn join(self, other: Span) -> Span {
        Span {
            start: self.start,
            end: other.end,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Source text whose spans are meaningful only relative to this file's contents.
/// Byte offsets must not be reused against another source file.
pub struct SourceFile {
    name: String,
    text: String,
}

impl SourceFile {
    pub fn new(name: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            text: text.into(),
        }
    }

    pub fn from_bytes(name: impl Into<String>, bytes: &[u8]) -> Result<Self, SyntaxDiagnostic> {
        match std::str::from_utf8(bytes) {
            Ok(text) => Ok(Self::new(name, text)),
            Err(error) => {
                let prefix = std::str::from_utf8(&bytes[..error.valid_up_to()])
                    .expect("valid_up_to always identifies valid UTF-8");
                let position = position_in(prefix, prefix.len());
                let span = Span {
                    start: position,
                    end: position,
                };
                Err(SyntaxDiagnostic::new(
                    "PTN_LEX_INVALID_UTF8",
                    "lex",
                    "source is not valid UTF-8",
                    span,
                ))
            }
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn position(&self, byte: usize) -> Option<Position> {
        if byte > self.text.len() || !self.text.is_char_boundary(byte) {
            return None;
        }
        Some(position_in(&self.text, byte))
    }

    pub fn span(&self, start: usize, end: usize) -> Option<Span> {
        if start > end {
            return None;
        }
        Some(Span {
            start: self.position(start)?,
            end: self.position(end)?,
        })
    }
}

fn position_in(text: &str, byte: usize) -> Position {
    let mut line = 1;
    let mut column = 1;
    let mut previous_was_cr = false;
    for (offset, ch) in text.char_indices() {
        if offset >= byte {
            break;
        }
        match ch {
            '\r' => {
                line += 1;
                column = 1;
                previous_was_cr = true;
            }
            '\n' => {
                if !previous_was_cr {
                    line += 1;
                    column = 1;
                }
                previous_was_cr = false;
            }
            _ => {
                column += 1;
                previous_was_cr = false;
            }
        }
    }
    Position { byte, line, column }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyntaxDiagnostic {
    pub code: &'static str,
    pub phase: &'static str,
    pub message: String,
    pub span: Span,
}

impl SyntaxDiagnostic {
    pub fn new(
        code: &'static str,
        phase: &'static str,
        message: impl Into<String>,
        span: Span,
    ) -> Self {
        Self {
            code,
            phase,
            message: message.into(),
            span,
        }
    }
}

impl fmt::Display for SyntaxDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}: {}",
            self.code, self.span.start.line, self.span.start.column, self.message
        )
    }
}

impl std::error::Error for SyntaxDiagnostic {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_track_utf8_scalars_and_all_newline_forms() {
        let source = SourceFile::new("test", "é\r\nX\rY\nZ");
        assert_eq!(
            source.position(0),
            Some(Position {
                byte: 0,
                line: 1,
                column: 1
            })
        );
        assert_eq!(
            source.position(2),
            Some(Position {
                byte: 2,
                line: 1,
                column: 2
            })
        );
        assert_eq!(
            source.position(3),
            Some(Position {
                byte: 3,
                line: 2,
                column: 1
            })
        );
        assert_eq!(
            source.position(4),
            Some(Position {
                byte: 4,
                line: 2,
                column: 1
            })
        );
        assert_eq!(
            source.position(5),
            Some(Position {
                byte: 5,
                line: 2,
                column: 2
            })
        );
        assert_eq!(
            source.position(6),
            Some(Position {
                byte: 6,
                line: 3,
                column: 1
            })
        );
        assert_eq!(
            source.position(source.text().len()),
            Some(Position {
                byte: 9,
                line: 4,
                column: 2
            })
        );
        assert_eq!(source.position(1), None);
        assert_eq!(source.span(5, 4), None);
    }

    #[test]
    fn invalid_utf8_points_at_first_invalid_byte() {
        let error = SourceFile::from_bytes("test", b"a\r\n\xff").unwrap_err();
        assert_eq!(error.code, "PTN_LEX_INVALID_UTF8");
        assert_eq!(
            error.span.start,
            Position {
                byte: 3,
                line: 2,
                column: 1
            }
        );
    }
}
