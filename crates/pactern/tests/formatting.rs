use pactern::formatting::format;
use pactern::syntax::{self, FileKind, SourceFile, TokenKind};
use std::{fs, path::PathBuf};

fn token_stream(source: &SourceFile) -> Vec<(TokenKind, String)> {
    syntax::lex(source)
        .unwrap()
        .into_iter()
        .map(|token| (token.kind, token.text))
        .collect()
}

fn checked(text: &str, kind: FileKind) -> String {
    let source = SourceFile::new("test.ptn", text);
    let output = format(&source, kind).unwrap();
    let formatted = SourceFile::new("test.ptn", &output);
    syntax::parse(&formatted, kind).unwrap();
    assert_eq!(token_stream(&source), token_stream(&formatted));
    assert_eq!(output, format(&formatted, kind).unwrap(), "{text}");
    assert!(output.ends_with('\n'));
    output
}

#[test]
fn canonical_implementation_layout() {
    let input = "module demo ; import util :: math;pub struct Pair < A , B > {first:A,second:B,}enum Choice<T>{Empty,Value(T),} pub fn run<T>(x:List<Option<T>>,)->Unit effects{db.read,db.write,}{let mut n:I64=0;while(n<10){n=n+1;if(n==3){continue;}else if(n==4){break;}else{();}}return;}";
    assert_eq!(
        checked(input, FileKind::Implementation),
        "\
module demo;
import util::math;
pub struct Pair<A, B> {
    first: A,
    second: B,
}
enum Choice<T> {
    Empty,
    Value(T),
}
pub fn run<T>(x: List<Option<T>>,) -> Unit effects { db.read, db.write, } {
    let mut n: I64 = 0;
    while (n < 10) {
        n = n + 1;
        if (n == 3) {
            continue;
        } else if (n == 4) {
            break;
        } else {
            ();
        }
    }
    return;
}
"
    );
}

#[test]
fn canonical_contract_layout() {
    assert_eq!(
        checked(
            "module demo;import helper;pact positive for run{requires x>0;ensures result==x+1;}",
            FileKind::Contract
        ),
        "\
module demo;
import helper;
pact positive for run {
    requires x > 0;
    ensures result == x + 1;
}
"
    );
}

#[test]
fn generics_patterns_expressions_and_exact_literal_spellings() {
    let output = checked(
        r#"module m;
fn f<T,>(x: List<Option<T>>,) -> Unit effects {} {
let r = Pair { first: (), second: [true, false,], };
let s = "é🙂\u{00e9}\n\t\\\"";
let n = -9223372036854775808;
let y = a || b && c == d < e + f * -g;
let z = !-x + - -1;
let u = (1 < 2) == (3 > 2);
let g: List<T> = id::<List<T>,>([x,],);
let v = match (x) { E::Nested(Option::Some(binding), _,) => 1, -42 => 2, "\u{E9}" => 3, _ => 4, };
host::read(borrow txn, r.first[0],);
{ return; }
}"#,
        FileKind::Implementation,
    );
    assert!(output.contains("List<Option<T>>"));
    assert!(output.contains("id::<List<T>,>([x,],)"));
    assert!(output.contains("a || b && c == d < e + f * -g"));
    assert!(output.contains("let z = !-x + --1;"));
    assert!(output.contains(r#""é🙂\u{00e9}\n\t\\\"""#));
}

#[test]
fn comments_keep_exact_contents_and_relative_token_order() {
    let input = "// leading café🙂\r\nmodule/* path */m; // module tail\r\n/* before function\r\n  contents stay\r\n*/fn f(// parameter\r\nx:I64) -> Unit { // body\r\n// statement\r\nlet n = -/* sign */1;/* after statement */\r\nlet s = \"🙂\";// string tail\r\nreturn; // return tail\r\n// before close\r\n} /* eof */";
    let output = checked(input, FileKind::Implementation);
    for comment in [
        "// leading café🙂",
        "/* path */",
        "// module tail",
        "/* before function\r\n  contents stay\r\n*/",
        "// parameter",
        "// body",
        "// statement",
        "/* sign */",
        "/* after statement */",
        "// string tail",
        "// return tail",
        "// before close",
        "/* eof */",
    ] {
        assert!(output.contains(comment), "lost {comment:?}");
    }
    assert!(output.contains("return; // return tail\n    // before close\n}"));
    assert!(output.ends_with("} /* eof */\n"));
}

#[test]
fn comment_positions_stay_stable_at_empty_blocks_and_eof() {
    for text in [
        "module m;// eof",
        "module m;/* eof */",
        "module m;\n// eof\n",
        "module m; fn f() -> Unit {/* inline */}",
        "module m; fn f() -> Unit {\n/* standalone */\n}",
        "module m; fn f() -> Unit { /* first */ /* second */ return; }",
        "module m; fn f() -> Unit { let x = (/* comment */1/* comment */); }",
        "/* first *//* second */module m;",
    ] {
        checked(text, FileKind::Implementation);
    }
}

#[test]
fn newline_layout_is_lf_and_strings_are_not_decoded() {
    let output = checked(
        "module m;\r\nfn f() -> Unit {\rlet x = \"é\\u{1F642}\\r\\n\";\nreturn;\r\n}",
        FileKind::Implementation,
    );
    assert!(!output.contains('\r'));
    assert!(output.contains("\"é\\u{1F642}\\r\\n\""));
}

#[test]
fn every_parseable_corpus_source_preserves_tokens_and_is_idempotent() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/conformance");
    let mut count = 0;
    for directory in ["src", "contracts", "variants"] {
        for entry in fs::read_dir(root.join(directory)).unwrap() {
            let path = entry.unwrap().path();
            let text = fs::read_to_string(&path).unwrap();
            let kind = if directory == "contracts" {
                FileKind::Contract
            } else {
                FileKind::Implementation
            };
            let source = SourceFile::new(path.display().to_string(), &text);
            if syntax::parse(&source, kind).is_ok() {
                checked(&text, kind);
                count += 1;
            }
        }
    }
    assert!(count >= 30, "unexpectedly small corpus: {count}");
}

#[test]
fn invalid_inputs_return_original_syntax_errors_without_repair() {
    for (text, kind) in [
        ("module m; fn f() -> Unit {", FileKind::Implementation),
        (
            "module m; fn f() -> Unit { let x = 01; }",
            FileKind::Implementation,
        ),
        ("module m; /* open", FileKind::Implementation),
        (
            "module m; fn f() -> Unit { let x = \"\\q\"; }",
            FileKind::Implementation,
        ),
        ("module m; pact p for f {}", FileKind::Contract),
        ("module m; fn f() -> Unit {}", FileKind::Contract),
    ] {
        let source = SourceFile::new("bad.ptn", text);
        assert_eq!(
            format(&source, kind).unwrap_err(),
            syntax::parse(&source, kind).unwrap_err()
        );
    }
}

#[test]
fn indentation_expansion_cannot_exceed_the_lexer_source_budget() {
    let text = format!(
        "module m; fn f() -> Unit {{{}{}{}}}",
        "{".repeat(20),
        "();".repeat(15_000),
        "}".repeat(20),
    );
    let source = SourceFile::new("expansion.ptn", text);
    assert!(source.text().len() < syntax::MAX_SOURCE_BYTES);
    let tree = syntax::parse(&source, FileKind::Implementation).unwrap();
    let error = format(&source, FileKind::Implementation).unwrap_err();
    assert_eq!(error.code, "PTN_TOOL_FORMAT_LIMIT");
    assert_eq!(error.phase, "tool");
    assert_eq!(error.span, tree.span);
}
