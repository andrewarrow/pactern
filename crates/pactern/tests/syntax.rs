use pactern::syntax::*;
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

fn source_file(text: &str) -> SourceFile {
    SourceFile::new("test.ptn", text)
}

fn implementation(text: &str) -> SyntaxTree {
    parse(&source_file(text), FileKind::Implementation).unwrap()
}

fn expression(text: &str) -> Expr {
    let tree = implementation(&format!(
        "module test; fn run() -> I64 {{ return {text}; }}"
    ));
    let DeclarationKind::Function(function) = tree.declarations.into_iter().next().unwrap().kind
    else {
        panic!()
    };
    let StatementKind::Return(Some(expr)) =
        function.body.statements.into_iter().next().unwrap().value
    else {
        panic!()
    };
    expr
}

fn check_span(source: &SourceFile, span: Span) {
    assert!(span.start.byte <= span.end.byte);
    assert_eq!(source.position(span.start.byte), Some(span.start));
    assert_eq!(source.position(span.end.byte), Some(span.end));
}

fn check_error(source: &SourceFile, error: &SyntaxDiagnostic) {
    check_span(source, error.span);
    assert!(matches!(error.phase, "lex" | "parse"));
    assert!(error.code.starts_with("PTN_"));
    assert!(!error.message.is_empty());
    assert!(error.message.len() < 256);
}

#[test]
fn conformance_sources_match_the_recorded_syntax_outcomes() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/conformance");
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(root.join("manifest.json")).unwrap()).unwrap();
    let mut sources = BTreeSet::new();
    for case in manifest["cases"].as_array().unwrap() {
        let path = case["source"].as_str().unwrap();
        sources.insert(path.to_owned());
        let source = SourceFile::new(path, fs::read_to_string(root.join(path)).unwrap());
        let parsed = parse(&source, FileKind::Implementation);
        let expected = &case["expected"];
        if matches!(expected["phase"].as_str(), Some("lex" | "parse")) {
            let error = parsed.unwrap_err();
            assert_eq!(error.code, expected["code"].as_str().unwrap(), "{path}");
            assert_eq!(error.phase, expected["phase"].as_str().unwrap(), "{path}");
            check_error(&source, &error);
        } else {
            let tree = parsed.unwrap_or_else(|error| panic!("{path}: {error}"));
            check_tree(&source, &tree);
        }
        for path in case["contracts"].as_array().into_iter().flatten() {
            let path = path.as_str().unwrap();
            sources.insert(path.to_owned());
            let source = SourceFile::new(path, fs::read_to_string(root.join(path)).unwrap());
            let tree = parse(&source, FileKind::Contract)
                .unwrap_or_else(|error| panic!("{path}: {error}"));
            check_tree(&source, &tree);
        }
    }
    // Every source in the corpus is exercised, including candidates and contracts.
    for directory in ["src", "contracts", "variants"] {
        for entry in fs::read_dir(root.join(directory)).unwrap() {
            let name = format!(
                "{directory}/{}",
                entry.unwrap().file_name().to_str().unwrap()
            );
            assert!(sources.contains(&name), "uncovered source: {name}");
        }
    }
}

#[test]
fn covers_declarations_generics_effects_borrows_and_control_flow() {
    let text = r#"// leading trivia
module sample::main;
import util::math;
pub struct Pair<A, B,> { first: A, second: B, }
enum Choice<T> { Empty, Value(T,), Pair(T, T), }
struct Empty {}
pub fn run<T,>(txn: borrow host::quota::Txn, values: List<Option<T>>,) -> Unit
    effects { db.read, db.write, } {
    let mut count: I64 = 0;
    let empty = Empty {};
    let values = [];
    let record = Pair { first: (), second: [true, false,], };
    while (count < 10) {
        count = count + 1;
        if (count == 3) { continue; } else if (count == 4) { break; } else { () ; }
    }
    { util::math::identity::<List<T>,>(values,); }
    host::quota::read(borrow txn, count,);
    return;
}
fn pure() -> Unit effects {} { return (); }
"#;
    let source = source_file(text);
    let tree = parse(&source, FileKind::Implementation).unwrap();
    check_tree(&source, &tree);
    assert_eq!(tree.module.value.value.len(), 2);
    assert_eq!(tree.imports.len(), 1);
    assert_eq!(tree.declarations.len(), 5);
    let declaration = &tree.declarations[3];
    assert!(declaration.public);
    assert!(text[declaration.span.start.byte..declaration.span.end.byte].starts_with("pub fn"));
    assert!(text[declaration.span.start.byte..declaration.span.end.byte].ends_with('}'));
    let DeclarationKind::Function(f) = &declaration.kind else {
        panic!()
    };
    assert_eq!(f.type_parameters[0].value, "T");
    assert!(f.parameters[0].borrowed);
    assert_eq!(
        f.parameters[1].ty.arguments[0].arguments[0].path.value[0].value,
        "T"
    );
    assert_eq!(f.effects.as_ref().unwrap().value.len(), 2);
    let DeclarationKind::Function(f) = &tree.declarations[4].kind else {
        panic!()
    };
    assert!(f.effects.as_ref().unwrap().value.is_empty());
}

#[test]
fn preserves_operator_precedence_associativity_and_grouping() {
    fn shape(expr: &Expr) -> String {
        match &expr.kind {
            ExprKind::Integer(s) => s.clone(),
            ExprKind::Path(p) => p.value[0].value.clone(),
            ExprKind::Unary { operator, operand } => {
                format!("({:?} {})", operator.value, shape(operand))
            }
            ExprKind::Binary {
                left,
                operator,
                right,
            } => format!("({:?} {} {})", operator.value, shape(left), shape(right)),
            ExprKind::Parenthesized(e) => format!("(group {})", shape(e)),
            _ => panic!("unexpected expression: {expr:?}"),
        }
    }
    assert_eq!(
        shape(&expression("a || b && c == d < e + f * -g")),
        "(Or a (And b (Equal c (Less d (Add e (Multiply f (Negate g)))))))"
    );
    assert_eq!(
        shape(&expression("9 - 3 - 1")),
        "(Subtract (Subtract 9 3) 1)"
    );
    assert_eq!(
        shape(&expression("9 / 3 % 2")),
        "(Remainder (Divide 9 3) 2)"
    );
    assert_eq!(shape(&expression("!-x")), "(Not (Negate x))");
    assert_eq!(
        shape(&expression("(1 < 2) == (3 > 2)")),
        "(Equal (group (Less 1 2)) (group (Greater 3 2)))"
    );
    for text in ["1 < 2 < 3", "1 > 2 <= 3", "1 == 2 != 3", "1 != 2 == 3"] {
        let error = parse(
            &source_file(&format!("module m; fn f() -> Bool {{ return {text}; }}")),
            FileKind::Implementation,
        )
        .unwrap_err();
        assert_eq!(error.code, "PTN_PARSE_UNEXPECTED_TOKEN");
    }
    let ExprKind::Unary { operand, .. } = expression("- /* trivia */ 9223372036854775808").kind
    else {
        panic!()
    };
    assert!(matches!(operand.kind, ExprKind::Integer(_)));
    let ExprKind::Unary { operand, .. } = expression("-(9223372036854775808)").kind else {
        panic!()
    };
    assert!(matches!(operand.kind, ExprKind::Parenthesized(_)));
    let ExprKind::Field { base, name } = expression("f(1,)[0].item").kind else {
        panic!()
    };
    assert_eq!(name.value, "item");
    let ExprKind::Index { base, .. } = base.kind else {
        panic!()
    };
    assert!(matches!(base.kind, ExprKind::Call { .. }));
    // Permissive postfix syntax is intentional; call-target checks come later.
    expression("(f)(1)");
    expression("a < b && c < d || a == b && c != d");
}

#[test]
fn covers_match_patterns_and_decodes_strings_without_normalizing() {
    let expr = expression(
        r#"match (value) {
        E::Nested(Option::Some(binding), _,) => 1,
        true => 2, false => 3, - /* trivia */ 42 => 4,
        0 => 5, "\u{E9}" => 6, name => 7, E::Unit => 8,
    }"#,
    );
    let ExprKind::Match { arms, .. } = expr.kind else {
        panic!()
    };
    assert_eq!(arms.len(), 8);
    assert!(matches!(arms[0].pattern.value, PatternKind::Variant { .. }));
    assert_eq!(
        arms[3].pattern.value,
        PatternKind::Integer {
            negative: true,
            digits: "42".into()
        }
    );
    assert_eq!(arms[5].pattern.value, PatternKind::String("é".into()));
    assert_eq!(
        expression(r#""a\n\r\t\\\"\u{0}\u{10FFFF}""#).kind,
        ExprKind::String("a\n\r\t\\\"\0\u{10FFFF}".into())
    );
    assert_eq!(expression("\"é\"").kind, ExprKind::String("é".into()));
    assert_ne!(expression("\"é\"").kind, expression("\"e\u{301}\"").kind);
}

#[test]
fn enforces_file_kinds_and_nonempty_grammar_productions() {
    for (text, kind) in [
        (
            "module m; pact p for f { ensures true; }",
            FileKind::Implementation,
        ),
        ("module m; fn f() -> Unit {}", FileKind::Contract),
        ("module m;", FileKind::Contract),
        ("module m; pact p for f {}", FileKind::Contract),
        (
            "module m; pub pact p for f { ensures true; }",
            FileKind::Contract,
        ),
        ("module m; enum Empty {}", FileKind::Implementation),
        ("module m; enum E { V() }", FileKind::Implementation),
        ("module m; struct S<> {}", FileKind::Implementation),
        (
            "module m; fn f(x: List<>) -> Unit {}",
            FileKind::Implementation,
        ),
        (
            "module m; import a; import b; import c::;",
            FileKind::Implementation,
        ),
        ("module m; struct S {} import a;", FileKind::Implementation),
        ("module m; struct _ {}", FileKind::Implementation),
        ("module m; fn result() -> Unit {}", FileKind::Implementation),
        ("fn f() -> Unit {}", FileKind::Implementation),
        ("module m; ;", FileKind::Implementation),
    ] {
        let source = source_file(text);
        let error = parse(&source, kind).expect_err(text);
        assert_eq!(error.phase, "parse", "{text}");
        check_error(&source, &error);
    }
    for expr in [
        "match (x) {}",
        "match (x) { E::V() => 0 }",
        "match (x) { binding(x) => 0 }",
        "match (x) { -true => 0 }",
        "id::<>(1)",
        "id::<I64>",
        "id(borrow txn.field)",
        "_",
        "[1,,2]",
    ] {
        let source = source_file(&format!("module m; fn f() -> I64 {{ return {expr}; }}"));
        assert!(parse(&source, FileKind::Implementation).is_err(), "{expr}");
    }
    for statement in [
        "values[0] = 1;",
        "record.field = 1;",
        "if true {}",
        "if (true) {} else return;",
        "while true {}",
        "let x;",
        "let x = 1",
        "x = 2",
        ";",
    ] {
        let source = source_file(&format!("module m; fn f() -> Unit {{ {statement} }}"));
        assert!(
            parse(&source, FileKind::Implementation).is_err(),
            "{statement}"
        );
    }
    implementation("module m; // empty implementation");
    let tree = parse(&source_file("module m; import helper; pact p for f { ensures result == 1; requires true; } pact q for f { requires true; }"), FileKind::Contract).unwrap();
    assert_eq!(tree.declarations.len(), 2);
    // Reserved result scope is intentionally deferred to resolution.
    expression("result");
}

#[test]
fn diagnostics_have_exact_locations_after_unicode_and_mixed_newlines() {
    let text = "module m;\r\nfn f() -> Unit {\rlet label = \"é🙂\";\nreturn +;\r\n}";
    let source = source_file(text);
    let error = parse(&source, FileKind::Implementation).unwrap_err();
    let byte = text.find('+').unwrap();
    assert_eq!(error.span, source.span(byte, byte + 1).unwrap());
    assert_eq!((error.span.start.line, error.span.start.column), (4, 8));
    let source = source_file("module m; fn f() -> Unit {");
    let error = parse(&source, FileKind::Implementation).unwrap_err();
    assert_eq!(error.span.start.byte, source.text().len());
    assert_eq!(error.span.start, error.span.end);
}

#[test]
fn adversarial_nesting_and_iterative_chains_return_bounded_errors() {
    let n = 2000;
    for expr in [
        format!("{}1{}", "(".repeat(n), ")".repeat(n)),
        format!("{}1", "!".repeat(n)),
        format!("{}1{}", "[".repeat(n), "]".repeat(n)),
        format!("0{}", "+1".repeat(n)),
        format!("x{}", ".field".repeat(n)),
        format!("f{}", "()".repeat(n)),
        format!(
            "match (x) {{ {}_{} => 1 }}",
            "E::V(".repeat(n),
            ")".repeat(n)
        ),
    ] {
        let source = source_file(&format!("module m; fn f() -> I64 {{ return {expr}; }}"));
        let error = parse(&source, FileKind::Implementation).unwrap_err();
        assert_eq!(error.code, "PTN_PARSE_LIMIT_EXCEEDED");
        check_error(&source, &error);
    }
    for body in [
        format!("{}{}", "{".repeat(n), "}".repeat(n)),
        format!("{}{{}}", "if (true) {} else ".repeat(n)),
    ] {
        let source = source_file(&format!("module m; fn f() -> Unit {{ {body} }}"));
        let error = parse(&source, FileKind::Implementation).unwrap_err();
        assert_eq!(error.code, "PTN_PARSE_LIMIT_EXCEEDED");
    }
    let source = source_file(&format!(
        "module m; fn f(x: {}I64{}) -> Unit {{}}",
        "List<".repeat(n),
        ">".repeat(n)
    ));
    assert_eq!(
        parse(&source, FileKind::Implementation).unwrap_err().code,
        "PTN_PARSE_LIMIT_EXCEEDED"
    );
}

#[test]
fn deterministic_generated_and_mutated_inputs_never_panic() {
    let seeds = [
        "module m; pub fn f<T>(x: List<T>) -> T effects { db.read } { return x[0]; }",
        "module m; fn f(x: I64) -> Bool { return match (x) { -1 => true, _ => false }; }",
        "module m; pact p for f { requires x > 0; ensures result == x; }",
        "module m; struct S { x: I64, } enum E<T> { V(T), N }",
    ];
    let alphabet: Vec<char> = "abc_0129{}[]()<>:;,.=+-*/%!&|\"\\ \t\r\né🙂\0\u{feff}"
        .chars()
        .collect();
    let mut state = 0x5041_4354_4552_4e01u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state as usize
    };
    for iteration in 0..3000 {
        let text = if iteration % 2 == 0 {
            let seed = seeds[next() % seeds.len()];
            let mut chars: Vec<char> = seed.chars().collect();
            for _ in 0..1 + next() % 8 {
                let at = next() % (chars.len() + 1);
                if at < chars.len() && next() % 2 == 0 {
                    chars.remove(at);
                } else {
                    chars.insert(at, alphabet[next() % alphabet.len()]);
                }
            }
            chars.into_iter().collect::<String>()
        } else {
            (0..next() % 256)
                .map(|_| alphabet[next() % alphabet.len()])
                .collect()
        };
        let source = source_file(&text);
        for kind in [FileKind::Implementation, FileKind::Contract] {
            match parse(&source, kind) {
                Ok(tree) => check_tree(&source, &tree),
                Err(error) => check_error(&source, &error),
            }
        }
    }
    // Truncation at every scalar boundary exercises EOF in every grammar position.
    for seed in seeds {
        for (end, _) in seed
            .char_indices()
            .chain(std::iter::once((seed.len(), '\0')))
        {
            let source = source_file(&seed[..end]);
            for kind in [FileKind::Implementation, FileKind::Contract] {
                if let Err(error) = parse(&source, kind) {
                    check_error(&source, &error);
                }
            }
        }
    }
}

fn check_tree(source: &SourceFile, tree: &SyntaxTree) {
    check_span(source, tree.span);
    check_span(source, tree.module.span);
    check_path(source, &tree.module.value);
    for import in &tree.imports {
        check_span(source, import.span);
        check_path(source, &import.value);
    }
    for declaration in &tree.declarations {
        check_span(source, declaration.span);
        match &declaration.kind {
            DeclarationKind::Function(f) => {
                check_span(source, f.name.span);
                for name in &f.type_parameters {
                    check_span(source, name.span);
                }
                for param in &f.parameters {
                    check_span(source, param.span);
                    check_span(source, param.name.span);
                    check_type(source, &param.ty);
                }
                check_type(source, &f.return_type);
                if let Some(effects) = &f.effects {
                    check_span(source, effects.span);
                    for path in &effects.value {
                        check_path(source, path);
                    }
                }
                check_block(source, &f.body);
            }
            DeclarationKind::Struct(s) => {
                check_span(source, s.name.span);
                for name in &s.type_parameters {
                    check_span(source, name.span);
                }
                for field in &s.fields {
                    check_span(source, field.span);
                    check_span(source, field.name.span);
                    check_type(source, &field.ty);
                }
            }
            DeclarationKind::Enum(e) => {
                check_span(source, e.name.span);
                for name in &e.type_parameters {
                    check_span(source, name.span);
                }
                for variant in &e.variants {
                    check_span(source, variant.span);
                    check_span(source, variant.name.span);
                    for ty in &variant.payload {
                        check_type(source, ty);
                    }
                }
            }
            DeclarationKind::Pact(p) => {
                check_span(source, p.name.span);
                check_path(source, &p.target);
                for clause in &p.clauses {
                    check_span(source, clause.span);
                    let (PactClause::Requires(e) | PactClause::Ensures(e)) = &clause.value;
                    check_expr(source, e);
                }
            }
        }
    }
}

fn check_path(source: &SourceFile, path: &Path) {
    check_span(source, path.span);
    for segment in &path.value {
        check_span(source, segment.span);
    }
}
fn check_type(source: &SourceFile, ty: &Type) {
    check_span(source, ty.span);
    check_path(source, &ty.path);
    for ty in &ty.arguments {
        check_type(source, ty);
    }
}
fn check_block(source: &SourceFile, block: &Block) {
    check_span(source, block.span);
    for statement in &block.statements {
        check_statement(source, statement);
    }
}
fn check_statement(source: &SourceFile, statement: &Statement) {
    check_span(source, statement.span);
    match &statement.value {
        StatementKind::Let {
            name, ty, value, ..
        } => {
            check_span(source, name.span);
            if let Some(ty) = ty {
                check_type(source, ty);
            }
            check_expr(source, value);
        }
        StatementKind::Assign { name, value } => {
            check_span(source, name.span);
            check_expr(source, value);
        }
        StatementKind::Expression(e) | StatementKind::Return(Some(e)) => check_expr(source, e),
        StatementKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            check_expr(source, condition);
            check_block(source, then_branch);
            if let Some(branch) = else_branch {
                check_statement(source, branch);
            }
        }
        StatementKind::While { condition, body } => {
            check_expr(source, condition);
            check_block(source, body);
        }
        StatementKind::Block(block) => check_block(source, block),
        _ => {}
    }
}
fn check_expr(source: &SourceFile, expr: &Expr) {
    check_span(source, expr.span);
    match &expr.kind {
        ExprKind::List(items) => {
            for expr in items {
                check_expr(source, expr);
            }
        }
        ExprKind::Record { path, fields } => {
            check_path(source, path);
            for field in fields {
                check_span(source, field.span);
                check_span(source, field.name.span);
                check_expr(source, &field.value);
            }
        }
        ExprKind::Path(path) => check_path(source, path),
        ExprKind::Parenthesized(e) => check_expr(source, e),
        ExprKind::Unary { operator, operand } => {
            check_span(source, operator.span);
            check_expr(source, operand);
        }
        ExprKind::Binary {
            left,
            operator,
            right,
        } => {
            check_expr(source, left);
            check_span(source, operator.span);
            check_expr(source, right);
        }
        ExprKind::Call { callee, arguments } => {
            check_expr(source, callee);
            check_arguments(source, arguments);
        }
        ExprKind::GenericCall {
            path,
            type_arguments,
            arguments,
        } => {
            check_path(source, path);
            for ty in type_arguments {
                check_type(source, ty);
            }
            check_arguments(source, arguments);
        }
        ExprKind::Field { base, name } => {
            check_expr(source, base);
            check_span(source, name.span);
        }
        ExprKind::Index { base, index } => {
            check_expr(source, base);
            check_expr(source, index);
        }
        ExprKind::Match { scrutinee, arms } => {
            check_expr(source, scrutinee);
            for arm in arms {
                check_span(source, arm.span);
                check_pattern(source, &arm.pattern);
                check_expr(source, &arm.value);
            }
        }
        _ => {}
    }
}
fn check_arguments(source: &SourceFile, arguments: &[Argument]) {
    for argument in arguments {
        match argument {
            Argument::Expression(e) => check_expr(source, e),
            Argument::Borrow(name) => {
                check_span(source, name.span);
                check_span(source, name.value.span);
            }
        }
    }
}
fn check_pattern(source: &SourceFile, pattern: &Pattern) {
    check_span(source, pattern.span);
    match &pattern.value {
        PatternKind::Binding(name) => check_span(source, name.span),
        PatternKind::Variant { path, payload } => {
            check_path(source, path);
            for pattern in payload {
                check_pattern(source, pattern);
            }
        }
        _ => {}
    }
}
