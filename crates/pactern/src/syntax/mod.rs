//! Syntax-only frontend for the 0.1.0 grammar.
//!
//! Parsing stops at the first located error; it never returns a partial tree as
//! a valid program. Names, types, integer ranges, and pact scope are checked by
//! later phases. These local diagnostics are not revision-bound protocol messages.
//!
//! ```
//! use pactern::syntax::{parse, FileKind, SourceFile};
//! let source = SourceFile::new("src/example.ptn",
//!     "module example; pub fn answer() -> I64 { return 42; }");
//! let tree = parse(&source, FileKind::Implementation)?;
//! assert_eq!(tree.declarations.len(), 1);
//! # Ok::<(), pactern::syntax::SyntaxDiagnostic>(())
//! ```

mod ast;
mod lexer;
mod parser;
mod source;

pub use ast::*;
pub use lexer::{Keyword, MAX_SOURCE_BYTES, MAX_TOKENS, Symbol, Token, TokenKind, lex};
pub use parser::{MAX_NESTING, parse};
pub use source::{Position, SourceFile, Span, SyntaxDiagnostic};
