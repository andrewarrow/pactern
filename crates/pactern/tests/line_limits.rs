use pactern::{
    formatting,
    syntax::{self, FileKind, MAX_SOURCE_LINES, SourceFile},
};

#[test]
fn limit_counts_comments_blanks_and_all_newline_forms() {
    assert_eq!(MAX_SOURCE_LINES, 1000);
    for newline in ["\n", "\r\n", "\r"] {
        let text = format!("module m;{newline}{}", format!("// é{newline}").repeat(999));
        let source = SourceFile::new("src/m.ptn", &text);
        syntax::parse(&source, FileKind::Implementation).unwrap();
        for extra in ["// excess", newline] {
            let source = SourceFile::new("src/m.ptn", format!("{text}{extra}"));
            let error = syntax::parse(&source, FileKind::Implementation).unwrap_err();
            assert_eq!(error.code, "PTN_LEX_LINE_LIMIT");
            assert_eq!(error.span.start.line, 1001);
            assert_eq!(error.span.start.byte, text.len());
            assert_eq!(error.span.start.column, 1);
        }
        let without_terminator = text.strip_suffix(newline).unwrap();
        syntax::parse(
            &SourceFile::new("src/m.ptn", without_terminator),
            FileKind::Implementation,
        )
        .unwrap();
    }
}

#[test]
fn contracts_and_block_comments_obey_the_same_limit() {
    let text = format!("module m;\n/*{}*/", "\n".repeat(999));
    let error = syntax::parse(
        &SourceFile::new("contracts/m.pact.ptn", text),
        FileKind::Contract,
    )
    .unwrap_err();
    assert_eq!(error.code, "PTN_LEX_LINE_LIMIT");
    syntax::lex(&SourceFile::new("empty", "")).unwrap();
    syntax::lex(&SourceFile::new("blanks", "\n".repeat(1000))).unwrap();
    assert!(syntax::lex(&SourceFile::new("blanks", "\n".repeat(1001))).is_err());
}

#[test]
fn formatter_cannot_expand_a_valid_file_past_the_limit() {
    let text = format!("module m; fn f() -> Unit {{{}}}", "();".repeat(1000));
    let source = SourceFile::new("src/m.ptn", text);
    syntax::parse(&source, FileKind::Implementation).unwrap();
    let error = formatting::format(&source, FileKind::Implementation).unwrap_err();
    assert_eq!(error.code, "PTN_TOOL_FORMAT_LINE_LIMIT");
    assert!(error.span.end.byte <= source.text().len());
}
