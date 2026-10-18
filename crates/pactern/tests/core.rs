use pactern::core::{
    self, DeclarationKind, ExpressionKind, LowerError, PatternKind, StatementKind,
};
use pactern::resolution::{FileId, ParsedFile, SymbolKind};
use pactern::syntax::{FileKind, SourceFile, parse};
use pactern::typing::ValueType;

fn file(path: &str, text: &str) -> ParsedFile {
    let kind = if path.ends_with(".pact.ptn") {
        FileKind::Contract
    } else {
        FileKind::Implementation
    };
    ParsedFile::new(path, parse(&SourceFile::new(path, text), kind).unwrap())
}
fn program(text: &str) -> core::Program {
    core::lower(&[file("src/main.ptn", &format!("module main; {text}"))]).unwrap()
}
fn body<'a>(p: &'a core::Program, name: &str) -> &'a core::Block {
    p.declarations()
        .iter()
        .find_map(|d| {
            if p.resolution().symbols[d.symbol.0].name == name
                && let DeclarationKind::Function { body, .. } = &d.kind
            {
                Some(body)
            } else {
                None
            }
        })
        .unwrap()
}
fn returned(block: &core::Block) -> &core::Expression {
    match &block.statements.last().unwrap().kind {
        StatementKind::Return(Some(e)) => e,
        other => panic!("{other:?}"),
    }
}

#[test]
fn literals_are_machine_values_and_origins_are_utf8_file_aware() {
    let text = "module main; fn value() -> I64 { let s = \"é🦀\"; return (-9223372036854775808); }";
    let source = SourceFile::new("src/main.ptn", text);
    let files = [file(source.name(), text)];
    let p = core::lower(&files).unwrap();
    let e = returned(body(&p, "value"));
    assert_eq!(e.kind, ExpressionKind::I64(i64::MIN));
    assert_eq!(e.ty, ValueType::I64);
    assert_eq!(e.origin.file, FileId(0));
    assert_eq!(
        source.span(e.origin.span.start.byte, e.origin.span.end.byte),
        Some(e.origin.span)
    );
    assert_eq!(
        &text[e.origin.span.start.byte..e.origin.span.end.byte],
        "(-9223372036854775808)"
    );
    assert_eq!(p, core::lower(&files).unwrap());
}

#[test]
fn generic_calls_keep_ordered_inferred_and_unused_explicit_arguments() {
    let p = program(
        "fn first<A, B>(a: A, b: B) -> A { return a; } fn unused<T>() -> Unit {} fn run() -> Bool { unused::<String>(); return first(true, 7); }",
    );
    let b = body(&p, "run");
    let StatementKind::Expression(e) = &b.statements[0].kind else {
        panic!()
    };
    let ExpressionKind::Call { type_arguments, .. } = &e.kind else {
        panic!()
    };
    assert_eq!(type_arguments, &[ValueType::String]);
    let ExpressionKind::Call {
        function,
        type_arguments,
        arguments,
    } = &returned(b).kind
    else {
        panic!()
    };
    assert_eq!(p.resolution().symbols[function.0].name, "first");
    assert_eq!(type_arguments, &[ValueType::Bool, ValueType::I64]);
    assert_eq!(arguments[0].kind, ExpressionKind::Bool(true));
    assert_eq!(arguments[1].kind, ExpressionKind::I64(7));
}

#[test]
fn record_fields_keep_source_order_and_member_access_is_resolved() {
    let p = program(
        "struct Row { a: I64, b: I64 } fn run() -> I64 { let r = Row { b: 2, a: 1 }; return r.a; }",
    );
    let b = body(&p, "run");
    let StatementKind::Let { value, symbol, .. } = &b.statements[0].kind else {
        panic!()
    };
    let ExpressionKind::Record { fields } = &value.kind else {
        panic!()
    };
    assert_eq!(
        fields
            .iter()
            .map(|(id, _)| p.resolution().symbols[id.0].name.as_str())
            .collect::<Vec<_>>(),
        ["b", "a"]
    );
    assert_eq!(p.symbol_types()[symbol], value.ty);
    let ExpressionKind::Field { base, field } = &returned(b).kind else {
        panic!()
    };
    assert_eq!(base.kind, ExpressionKind::Read(*symbol));
    assert_eq!(*field, fields[1].0);
}

#[test]
fn nested_generic_patterns_have_substituted_payload_types() {
    let p = program(
        "enum E<T> { V(Option<T>), Empty } fn run(x: E<Bool>) -> Bool { return match (x) { E::V(Option::Some(v)) => v, E::V(Option::None) => false, E::Empty => true }; }",
    );
    let ExpressionKind::Match { arms, .. } = &returned(body(&p, "run")).kind else {
        panic!()
    };
    let PatternKind::Variant { payload, .. } = &arms[0].pattern.kind else {
        panic!()
    };
    assert_eq!(payload[0].ty, ValueType::Option(Box::new(ValueType::Bool)));
    let PatternKind::Variant { payload, .. } = &payload[0].kind else {
        panic!()
    };
    assert_eq!(payload[0].ty, ValueType::Bool);
    let PatternKind::Binding(id) = payload[0].kind else {
        panic!()
    };
    assert_eq!(arms[0].value.kind, ExpressionKind::Read(id));
    assert_eq!(p.symbol_types()[&id], ValueType::Bool);
}

#[test]
fn contracts_and_imports_use_the_same_program_symbol_table() {
    let files = [
        file(
            "src/util.ptn",
            "module util; pub fn id<T>(x: T) -> T { return x; }",
        ),
        file(
            "src/main.ptn",
            "module main; import util; pub fn run(x: I64) -> I64 effects {} { return util::id(x); }",
        ),
        file(
            "contracts/main.pact.ptn",
            "module main; pact positive for run { requires x > 0; ensures result > 0; }",
        ),
    ];
    let p = core::lower(&files).unwrap();
    let pact = p.declarations().last().unwrap();
    let DeclarationKind::Pact { target, clauses } = &pact.kind else {
        panic!()
    };
    assert_eq!(p.resolution().symbols[target.0].name, "run");
    assert_eq!(pact.origin.file, FileId(2));
    assert!(clauses[0].requires);
    assert!(!clauses[1].requires);
    let ExpressionKind::Binary { left, .. } = &clauses[1].condition.kind else {
        panic!()
    };
    let ExpressionKind::Read(id) = left.kind else {
        panic!()
    };
    assert_eq!(p.resolution().symbols[id.0].kind, SymbolKind::Result);
    assert_eq!(p.symbol_types()[&id], ValueType::I64);
}

#[test]
fn invalid_programs_return_diagnostics_instead_of_partial_ir() {
    for source in [
        "fn f() -> I64 { return missing; }",
        "fn f() -> I64 { return true; }",
        "fn f() -> I64 { return 9223372036854775808; }",
        "fn f() -> I64 { return -(9223372036854775808); }",
        "fn f() -> Unit { let x = 1; x = 2; }",
        "fn f(x: Bool) -> I64 { return match (x) { true => 1 }; }",
    ] {
        let result = core::lower(&[file("src/main.ptn", &format!("module main; {source}"))]);
        let Err(LowerError::Check(checked)) = result else {
            panic!("{source}: {result:?}")
        };
        assert!(!checked.is_success());
    }
}

#[test]
fn structured_control_flow_and_short_circuiting_remain_explicit() {
    let p = program(
        "fn run(b: Bool) -> Bool { let mut x = 0; while (x < 5) { x = x + 1; if (x == 2) { continue; } else if (x == 4) { break; } { let y = (); } } return b && (10 / x > 0) || false; }",
    );
    let b = body(&p, "run");
    let StatementKind::While { body, .. } = &b.statements[1].kind else {
        panic!()
    };
    assert!(matches!(
        body.statements[0].kind,
        StatementKind::Assign { .. }
    ));
    assert!(matches!(body.statements[1].kind, StatementKind::If { .. }));
    assert!(matches!(body.statements[2].kind, StatementKind::Block(_)));
    let ExpressionKind::Binary { left, operator, .. } = &returned(b).kind else {
        panic!()
    };
    assert_eq!(*operator, pactern::syntax::BinaryOp::Or);
    assert!(matches!(
        left.kind,
        ExpressionKind::Binary {
            operator: pactern::syntax::BinaryOp::And,
            ..
        }
    ));
}
