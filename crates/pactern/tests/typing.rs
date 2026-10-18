use pactern::resolution::{FileId, ParsedFile, SymbolKind};
use pactern::syntax::{FileKind, SourceFile, parse};
use pactern::typing::{CheckResult, ValueType, check};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

fn file(path: &str, source: &str) -> ParsedFile {
    let kind = if path.ends_with(".pact.ptn") {
        FileKind::Contract
    } else {
        FileKind::Implementation
    };
    ParsedFile::new(path, parse(&SourceFile::new(path, source), kind).unwrap())
}
fn program(source: &str) -> CheckResult {
    let result = check(&[file("src/main.ptn", &format!("module main; {source}"))]);
    assert!(
        result.resolution.is_success(),
        "{:?}",
        result.resolution.diagnostics
    );
    result
}
fn accepts(source: &str) -> CheckResult {
    let result = program(source);
    assert!(result.is_success(), "{source}\n{:?}", result.diagnostics);
    result
}
fn rejects(source: &str, code: &str) -> CheckResult {
    let result = program(source);
    assert!(
        result.diagnostics.iter().any(|d| d.code == code),
        "{source}\nexpected {code}, got {:?}",
        result.diagnostics
    );
    result
}

#[test]
fn core_values_infer_through_annotations_calls_lists_records_and_variants() {
    let result = accepts(
        r#"
        struct Row { name: String, count: I64, active: Bool }
        enum Choice { Empty, Row(Row), Pair(Bool, Option<I64>) }
        fn echo(x: List<Option<Result<I64, String>>>) -> List<Option<Result<I64, String>>> { return x; }
        fn run() -> I64 {
            let values = [Option::None, Option::Some(Result::Err("é")), Option::Some(Result::Ok(7))];
            let copied = echo(values);
            let nested = [Option::Some([]), Option::Some([1])];
            let mut row = Row { active: true, count: 3, name: "bad" };
            return row.count;
        }
    "#,
    );
    assert!(!result.expression_types.is_empty());
}

#[test]
fn nominal_records_enum_payloads_and_binding_types_are_retained() {
    let result = accepts(
        r#"
        struct Row { name: String, count: I64, active: Bool }
        enum Choice { Empty, Row(Row) }
        fn run() -> I64 {
            let row = Row { active: true, count: 3, name: "é" };
            let choice = Choice::Row(row);
            let count = match (choice) { Choice::Empty => 0, Choice::Row(value) => value.count };
            return count;
        }
    "#,
    );
    let count = result
        .resolution
        .symbols
        .iter()
        .find(|s| s.name == "count" && s.kind == SymbolKind::Local)
        .unwrap();
    assert_eq!(result.symbol_types[&count.id], ValueType::I64);
    let row = result
        .resolution
        .symbols
        .iter()
        .find(|s| s.name == "row")
        .unwrap();
    let row_type = result
        .resolution
        .symbols
        .iter()
        .find(|s| s.name == "Row" && s.kind == SymbolKind::Struct)
        .unwrap();
    assert_eq!(result.symbol_types[&row.id], ValueType::Named(row_type.id));
    assert_eq!(
        result,
        program(
            r#"
        struct Row { name: String, count: I64, active: Bool }
        enum Choice { Empty, Row(Row) }
        fn run() -> I64 {
            let row = Row { active: true, count: 3, name: "é" };
            let choice = Choice::Row(row);
            let count = match (choice) { Choice::Empty => 0, Choice::Row(value) => value.count };
            return count;
        }
    "#
        )
    );
}

#[test]
fn integer_limits_distinguish_literals_from_well_typed_traps() {
    accepts(
        "fn f() -> I64 { let min = - /* trivia */ 9223372036854775808; return min / -1 + 9223372036854775807 + 1; }",
    );
    for literal in [
        "9223372036854775808",
        "-9223372036854775809",
        "-(9223372036854775808)",
        "99999999999999999999999999999999999999999999",
    ] {
        rejects(
            &format!("fn f() -> I64 {{ return {literal}; }}"),
            "PTN_TYPE_INTEGER_RANGE",
        );
    }
    rejects(
        "fn f(x: I64) -> I64 { return match (x) { -9223372036854775809 => 1, _ => 0 }; }",
        "PTN_TYPE_INTEGER_RANGE",
    );
}

#[test]
fn empty_lists_and_builtin_constructors_need_sufficient_context() {
    accepts(
        r#"
        struct Data { list: List<I64>, option: Option<String> }
        fn consume(x: List<I64>) -> Unit {}
        fn run() -> Result<List<I64>, String> {
            let explicit: List<Option<I64>> = [];
            let data = Data { list: [], option: Option::None };
            let sibling = [[], [1]];
            let options = [Option::None, Option::Some(1)];
            let results = [Result::Ok(1), Result::Err("error")];
            let nested = [Option::Some([]), Option::Some([1])];
            consume([]);
            return Result::Ok([]);
        }
    "#,
    );
    for value in [
        "[]",
        "Option::None",
        "Result::Ok(1)",
        "Result::Err(false)",
        "Option::Some([])",
    ] {
        rejects(
            &format!("fn f() -> Unit {{ let ambiguous = {value}; }}"),
            "PTN_TYPE_CANNOT_INFER",
        );
    }
    rejects(
        "fn f() -> Unit { let ambiguous = []; let later: List<I64> = ambiguous; }",
        "PTN_TYPE_CANNOT_INFER",
    );
    rejects(
        "fn f() -> List<I64> { return [1, true]; }",
        "PTN_TYPE_MISMATCH",
    );
}

#[test]
fn parameters_and_ordinary_locals_are_immutable_but_mutable_bindings_keep_their_type() {
    accepts("fn f(x: I64) -> I64 { let mut y = x; { y = 2; } return y; }");
    rejects(
        "fn f(x: I64) -> Unit { x = 2; }",
        "PTN_TYPE_IMMUTABLE_ASSIGNMENT",
    );
    let result = rejects(
        "fn f() -> Unit { let x = 1; x = 2; }",
        "PTN_TYPE_IMMUTABLE_ASSIGNMENT",
    );
    assert_eq!(result.diagnostics[0].related.len(), 1);
    rejects(
        "fn f() -> Unit { let mut x = 1; x = true; }",
        "PTN_TYPE_MISMATCH",
    );
}

#[test]
fn operators_conditions_indexes_and_expression_statements_enforce_core_rules() {
    accepts(
        "fn f() -> Bool { let x = !false && true || false; let y = () == (); return x && \"é\" != \"e\"; }",
    );
    for source in [
        "fn f() -> I64 { return true + 1; }",
        "fn f() -> Bool { return false < true; }",
        "fn f() -> Bool { return 1 == true; }",
        "fn f() -> Bool { return !1; }",
        "fn f() -> Unit { if (1) {} }",
        "fn f() -> Unit { while (1) {} }",
        "fn f() -> Unit { 42; }",
        "fn f() -> I64 { return [1][true]; }",
    ] {
        rejects(source, "PTN_TYPE_MISMATCH");
    }
    for values in [
        "[1] == [1]",
        "Option::Some(1) == Option::Some(1)",
        "R {} == R {}",
    ] {
        rejects(
            &format!("struct R {{}} fn f() -> Bool {{ return {values}; }}"),
            "PTN_TYPE_INVALID_EQUALITY",
        );
    }
    rejects(
        "fn f() -> I64 { return \"abc\"[0]; }",
        "PTN_TYPE_INVALID_INDEX",
    );
    // Bounds and arithmetic failures are runtime traps, not type errors.
    accepts("fn f() -> I64 { return [1][-1] / 0; }");
}

#[test]
fn record_construction_and_access_check_all_fields_exactly_once() {
    for (body, code) in [
        ("return R { a: 1 };", "PTN_TYPE_MISSING_FIELD"),
        (
            "return R { a: 1, b: true, c: 3 };",
            "PTN_TYPE_UNKNOWN_FIELD",
        ),
        (
            "return R { a: 1, b: true, a: 2 };",
            "PTN_TYPE_DUPLICATE_FIELD",
        ),
        ("return R { a: false, b: true };", "PTN_TYPE_MISMATCH"),
    ] {
        rejects(
            &format!("struct R {{ a: I64, b: Bool }} fn f() -> R {{ {body} }}"),
            code,
        );
    }
    rejects(
        "struct R {} fn f(x: R) -> I64 { return x.missing; }",
        "PTN_TYPE_UNKNOWN_FIELD",
    );
    rejects(
        "fn f() -> I64 { return 1.missing; }",
        "PTN_TYPE_UNKNOWN_FIELD",
    );
    rejects(
        "struct A {} struct B {} fn f() -> A { return B {}; }",
        "PTN_TYPE_MISMATCH",
    );
}

#[test]
fn function_calls_are_named_arity_checked_and_can_be_recursive() {
    accepts(
        "fn even(x: I64) -> Bool { if (x == 0) { return true; } return odd(x-1); } fn odd(x: I64) -> Bool { return even(x-1); }",
    );
    rejects(
        "fn g(x: I64) -> Unit {} fn f() -> Unit { g(); }",
        "PTN_TYPE_ARITY",
    );
    rejects(
        "fn g() -> Unit {} fn f() -> Unit { g(1); }",
        "PTN_TYPE_ARITY",
    );
    rejects(
        "fn g(x: I64) -> Unit {} fn f() -> Unit { g(true); }",
        "PTN_TYPE_MISMATCH",
    );
    rejects(
        "fn g() -> Unit {} fn f() -> Unit { let x = g; }",
        "PTN_TYPE_FUNCTION_VALUE",
    );
    rejects(
        "fn g() -> Unit {} fn f() -> Unit { (g)(); }",
        "PTN_TYPE_INVALID_CALLEE",
    );
    rejects(
        "enum E { A, B(I64) } fn f() -> E { return E::A(); }",
        "PTN_TYPE_UNIT_VARIANT_CALL",
    );
    rejects(
        "enum E { A, B(I64) } fn f() -> E { return E::B; }",
        "PTN_TYPE_ARITY",
    );
    rejects(
        "enum E { A, B(I64) } fn f() -> E { return E::B(true); }",
        "PTN_TYPE_MISMATCH",
    );
}

#[test]
fn return_analysis_checks_each_reachable_normal_exit_and_loop_controls() {
    accepts(
        "fn f(b: Bool) -> I64 { if (b) { return 1; } else if (!b) { return 2; } else { return 3; } }",
    );
    accepts("fn f() -> Unit { while (true) { if (true) { break; } else { continue; } } return; }");
    accepts("fn f() -> Unit { {} }");
    rejects(
        "fn f(b: Bool) -> I64 { if (b) { return 1; } }",
        "PTN_TYPE_MISSING_RETURN",
    );
    rejects(
        "fn f() -> I64 { while (true) { return 1; } }",
        "PTN_TYPE_MISSING_RETURN",
    );
    rejects("fn f() -> I64 { return; }", "PTN_TYPE_MISMATCH");
    rejects("fn f() -> Unit { return 1; }", "PTN_TYPE_MISMATCH");
    rejects("fn f() -> Unit { break; }", "PTN_TYPE_LOOP_CONTROL");
    rejects("fn f() -> Unit { continue; }", "PTN_TYPE_LOOP_CONTROL");
    rejects(
        "fn f() -> I64 { return 1; let x: I64 = true; }",
        "PTN_TYPE_MISMATCH",
    );
}

#[test]
fn match_checks_nested_payloads_and_correlations_across_columns() {
    accepts(
        r#"
        enum Pair { Both(Bool, Bool), Empty }
        fn f(x: Pair) -> I64 {
            return match (x) {
                Pair::Both(true, _) => 1,
                Pair::Both(false, true) => 2,
                Pair::Both(false, false) => 3,
                Pair::Empty => 0,
            };
        }
        fn g(x: Option<Option<Bool>>) -> I64 {
            return match (x) {
                Option::None => 0,
                Option::Some(Option::None) => 1,
                Option::Some(Option::Some(true)) => 2,
                Option::Some(Option::Some(false)) => 3,
            };
        }
    "#,
    );
    rejects(
        "enum Pair { Both(Bool, Bool) } fn f(x: Pair) -> I64 { return match (x) { Pair::Both(true, _) => 1, Pair::Both(_, true) => 2 }; }",
        "PTN_TYPE_NONEXHAUSTIVE_MATCH",
    );
    rejects(
        "fn f(x: Option<Bool>) -> I64 { return match (x) { Option::Some(true) => 1, Option::None => 0 }; }",
        "PTN_TYPE_NONEXHAUSTIVE_MATCH",
    );
    rejects(
        "fn f(x: Option<Bool>) -> I64 { return match (x) { Option::Some(_) => 1, Option::None => 0, Option::Some(true) => 2 }; }",
        "PTN_TYPE_UNREACHABLE_MATCH_ARM",
    );
    rejects(
        "fn f(x: Bool) -> I64 { return match (x) { true => 1, false => 0, _ => 2 }; }",
        "PTN_TYPE_UNREACHABLE_MATCH_ARM",
    );
    rejects(
        "fn f(x: I64) -> Bool { return match (x) { -0 => true, 0 => false, _ => false }; }",
        "PTN_TYPE_UNREACHABLE_MATCH_ARM",
    );
}

#[test]
fn patterns_check_types_arity_infinite_domains_and_decoded_strings() {
    accepts(
        "fn f(x: I64) -> I64 { return match (x) { 0 => 1, -9223372036854775808 => 2, value => value }; }",
    );
    rejects(
        "fn f(x: I64) -> Bool { return match (x) { 0 => true }; }",
        "PTN_TYPE_NONEXHAUSTIVE_MATCH",
    );
    rejects(
        "fn f(x: String) -> Bool { return match (x) { \"a\" => true }; }",
        "PTN_TYPE_NONEXHAUSTIVE_MATCH",
    );
    rejects(
        r#"fn f(x: String) -> Bool { return match (x) { "a" => true, "\u{61}" => false, _ => false }; }"#,
        "PTN_TYPE_UNREACHABLE_MATCH_ARM",
    );
    rejects(
        "fn f(x: I64) -> I64 { return match (x) { true => 1, _ => 0 }; }",
        "PTN_TYPE_MISMATCH",
    );
    rejects(
        "fn f(x: Option<I64>) -> I64 { return match (x) { Option::Some(a,b) => a, Option::None => 0 }; }",
        "PTN_TYPE_ARITY",
    );
    rejects(
        "fn f(x: Bool) -> I64 { return match (x) { true => 1, false => false }; }",
        "PTN_TYPE_MISMATCH",
    );
    accepts(
        "enum Tree { Leaf, Branch(Tree) } fn f(x: Tree) -> Bool { return match (x) { Tree::Leaf => true, Tree::Branch(_) => false }; }",
    );
}

#[test]
fn type_arity_and_handle_boundaries_are_explicit() {
    for ty in [
        "List",
        "Option<I64, Bool>",
        "Result<I64>",
        "Bool<I64>",
        "R<I64>",
    ] {
        rejects(
            &format!("struct R {{}} fn f(x: {ty}) -> Unit {{}}"),
            "PTN_TYPE_ARITY",
        );
    }
    rejects("fn f() -> I64 { return f::<I64>(); }", "PTN_TYPE_ARITY");
    rejects(
        "fn f(x: borrow I64) -> Unit {}",
        "PTN_TYPE_UNSUPPORTED_HANDLE",
    );
    rejects(
        "fn g(x: I64) -> Unit {} fn f(x: I64) -> Unit { g(borrow x); }",
        "PTN_TYPE_UNSUPPORTED_HANDLE",
    );
}

#[test]
fn imports_and_contracts_share_types_with_file_aware_diagnostics() {
    let files = [
        file(
            "src/main.ptn",
            "module main; import data; fn run(x: data::Row) -> I64 { return x.count; }",
        ),
        file("src/data.ptn", "module data; pub struct Row { count: I64 }"),
        file(
            "contracts/main.pact.ptn",
            "module main; pact safety for run { requires x.count >= 0; ensures result == x.count; }",
        ),
    ];
    let result = check(&files);
    assert!(result.is_success(), "{:?}", result.diagnostics);
    let mut bad = files.to_vec();
    bad[2] = file(
        "contracts/main.pact.ptn",
        "module main; pact safety for run { ensures result; }",
    );
    let result = check(&bad);
    assert_eq!(result.diagnostics[0].location.file, FileId(2));
    assert_eq!(result.diagnostics[0].code, "PTN_TYPE_MISMATCH");
    let result = check(&[file(
        "src/main.ptn",
        "module main; fn f() -> I64 { return missing; }",
    )]);
    assert!(!result.is_success());
    assert!(!result.resolution.is_success());
    assert!(result.diagnostics.is_empty());
}

#[test]
fn conformance_corpus_checks_all_recorded_type_outcomes_and_well_typed_runtime_cases() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/conformance");
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(root.join("manifest.json")).unwrap()).unwrap();
    let mut covered = BTreeSet::new();
    for case in manifest["cases"].as_array().unwrap() {
        let expected = &case["expected"];
        if matches!(
            expected["phase"].as_str(),
            Some("lex" | "parse" | "resolve")
        ) {
            continue;
        }
        let files: Vec<_> = std::iter::once(case["source"].as_str().unwrap())
            .chain(
                case["contracts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|v| v.as_str().unwrap()),
            )
            .map(|path| {
                let text = fs::read_to_string(root.join(path)).unwrap();
                let parsed = file(path, &text);
                let module = parsed
                    .tree
                    .module
                    .value
                    .value
                    .iter()
                    .map(|i| i.value.as_str())
                    .collect::<Vec<_>>()
                    .join("/");
                let identity = if path.ends_with(".pact.ptn") {
                    format!("contracts/{module}.pact.ptn")
                } else {
                    format!("src/{module}.ptn")
                };
                ParsedFile::new(identity, parsed.tree)
            })
            .collect();
        let result = check(&files);
        assert!(
            result.resolution.is_success(),
            "{}: {:?}",
            case["id"],
            result.resolution.diagnostics
        );
        if expected["phase"] == "type" {
            let code = expected["code"].as_str().unwrap();
            assert!(
                result.diagnostics.iter().any(|d| d.code == code),
                "{}: expected {code}, got {:?}",
                case["id"],
                result.diagnostics
            );
            covered.insert(code.to_string());
        } else {
            assert!(
                result.is_success(),
                "{}: {:?}",
                case["id"],
                result.diagnostics
            );
        }
        for d in &result.diagnostics {
            assert!(d.location.file.0 < files.len());
            for location in std::iter::once(d.location).chain(d.related.iter().map(|r| r.location))
            {
                let path = if location.file == FileId(0) {
                    &case["source"]
                } else {
                    &case["contracts"][location.file.0 - 1]
                };
                let source = SourceFile::new(
                    path.as_str().unwrap(),
                    fs::read_to_string(root.join(path.as_str().unwrap())).unwrap(),
                );
                assert_eq!(
                    source.span(location.span.start.byte, location.span.end.byte),
                    Some(location.span)
                );
            }
            assert_eq!(d.phase, "type");
        }
    }
    assert_eq!(covered.len(), 8);
}

#[test]
fn readme_scheduler_and_its_pact_pass_core_typing() {
    let readme = include_str!("../../../README.md");
    let mut snippets = readme
        .split("```text\n")
        .skip(1)
        .map(|s| s.split("```").next().unwrap());
    let files = [
        file("src/scheduler.ptn", snippets.next().unwrap()),
        file("contracts/scheduler.pact.ptn", snippets.next().unwrap()),
    ];
    let result = check(&files);
    assert!(
        result.is_success(),
        "{:?}\n{:?}",
        result.resolution.diagnostics,
        result.diagnostics
    );
}

#[test]
fn whole_expression_inference_precedes_nested_match_coverage() {
    accepts(
        r#"
        fn f() -> Unit {
            let values = [match (Option::None) {
                Option::Some(n) => n,
                Option::None => [],
            }, [1]];
        }
    "#,
    );
    rejects(
        "fn f() -> Unit { let x = match (Option::None) { Option::None => (), Option::Some(n) => () }; }",
        "PTN_TYPE_CANNOT_INFER",
    );
}

#[test]
fn boolean_product_coverage_agrees_with_an_independent_finite_domain_oracle() {
    // Each mask explicitly enumerates the four Pair values matched by a pattern.
    let patterns = [
        ("Pair::Both(true, true)", 0b0001u8),
        ("Pair::Both(true, false)", 0b0010),
        ("Pair::Both(false, true)", 0b0100),
        ("Pair::Both(false, false)", 0b1000),
        ("Pair::Both(true, _)", 0b0011),
        ("Pair::Both(false, _)", 0b1100),
        ("Pair::Both(_, true)", 0b0101),
        ("Pair::Both(_, false)", 0b1010),
        ("Pair::Both(_, _)", 0b1111),
        ("_", 0b1111),
    ];
    for (a, a_mask) in patterns {
        for (b, b_mask) in patterns {
            for (c, c_mask) in patterns {
                let source = format!(
                    "enum Pair {{ Both(Bool, Bool) }} fn f(x: Pair) -> I64 {{ return match (x) {{ {a} => 1, {b} => 2, {c} => 3 }}; }}"
                );
                let result = program(&source);
                let nonexhaustive = a_mask | b_mask | c_mask != 0b1111;
                let unreachable = usize::from(b_mask & !a_mask == 0)
                    + usize::from(c_mask & !(a_mask | b_mask) == 0);
                assert_eq!(
                    result
                        .diagnostics
                        .iter()
                        .any(|d| d.code == "PTN_TYPE_NONEXHAUSTIVE_MATCH"),
                    nonexhaustive,
                    "{source}"
                );
                assert_eq!(
                    result
                        .diagnostics
                        .iter()
                        .filter(|d| d.code == "PTN_TYPE_UNREACHABLE_MATCH_ARM")
                        .count(),
                    unreachable,
                    "{source}"
                );
                assert_eq!(
                    result.diagnostics.len(),
                    usize::from(nonexhaustive) + unreachable
                );
            }
        }
    }
}

#[test]
fn large_constructor_lists_do_not_create_recursive_inference_chains() {
    let values = std::iter::repeat_n("Option::None", 3000)
        .chain(std::iter::once("Option::Some(1)"))
        .collect::<Vec<_>>()
        .join(",");
    accepts(&format!("fn f() -> Unit {{ let values = [{values}]; }}"));
}

#[test]
fn types_and_pattern_analysis_have_explicit_resource_limits() {
    let mut depth = "fn f() -> Unit { let v0 = 1;".to_owned();
    for n in 1..100 {
        depth.push_str(&format!("let v{n} = [v{}];", n - 1));
    }
    depth.push('}');
    rejects(&depth, "PTN_TYPE_LIMIT");
    let mut size = "fn f() -> Unit { let v0 = 1;".to_owned();
    for n in 1..16 {
        size.push_str(&format!(
            "let v{n} = [Result::Ok(v{}), Result::Err(v{})];",
            n - 1,
            n - 1
        ));
    }
    size.push('}');
    rejects(&size, "PTN_TYPE_LIMIT");
    let arms = (0..300).map(|n| format!("{n} => {n},")).collect::<String>();
    rejects(
        &format!("fn f(x: I64) -> I64 {{ return match (x) {{ {arms} _ => 0 }}; }}"),
        "PTN_TYPE_MATCH_LIMIT",
    );
}

#[test]
fn generic_functions_infer_independently_and_check_bodies_parametrically() {
    accepts(
        r#"
        fn identity<T>(x: T) -> T { return x; }
        fn wrap<T>(x: T) -> List<T> { return [identity(x)]; }
        fn empty<T>() -> List<T> { return []; }
        fn run() -> Bool {
            let n = identity(1);
            let b = identity::<Bool>(true);
            let xs: List<String> = empty();
            let nested = wrap(Option::Some(n));
            return b;
        }
    "#,
    );
    for source in [
        "fn bad<T>(x: T) -> T { return x + 1; }",
        "fn bad<T>(x: T) -> I64 { return x; }",
        "fn bad<T, U>(x: T) -> U { return x; }",
        "fn id<T>(x: T) -> T { return x; } fn f() -> Bool { return id::<I64>(true); }",
    ] {
        rejects(source, "PTN_TYPE_MISMATCH");
    }
    rejects(
        "fn bad<T>(x: T) -> Bool { return x == x; }",
        "PTN_TYPE_INVALID_EQUALITY",
    );
    rejects(
        "fn empty<T>() -> List<T> { return []; } fn f() -> Unit { let x = empty(); }",
        "PTN_TYPE_CANNOT_INFER",
    );
    rejects(
        "fn unused<T>() -> Unit {} fn f() -> Unit { unused(); }",
        "PTN_TYPE_CANNOT_INFER",
    );
    rejects(
        "fn id<T>(x: T) -> T { return x; } fn f() -> I64 { return id::<I64, Bool>(1); }",
        "PTN_TYPE_ARITY",
    );
}

#[test]
fn generic_records_enums_fields_and_nested_coverage_substitute_parameters() {
    accepts(
        r#"
        struct Box<T> { value: T }
        enum Choice<T> { Empty, Value(T) }
        fn unbox<T>(x: Box<T>) -> T { return x.value; }
        fn choose<T>(x: T) -> Choice<Box<T>> { return Choice::Value(Box { value: x }); }
        fn run() -> Bool {
            let b = Box { value: true };
            let n = Box { value: 1 };
            let empty: Choice<String> = Choice::Empty;
            let xs: Box<List<I64>> = Box { value: [] };
            let choice = choose(unbox(b));
            return match (choice) { Choice::Empty => false, Choice::Value(v) => v.value };
        }
        fn coverage(x: Choice<Bool>) -> I64 {
            return match (x) { Choice::Empty => 0, Choice::Value(true) => 1, Choice::Value(false) => 2 };
        }
    "#,
    );
    rejects(
        "struct Box<T> { value: T } fn f(x: Box<Bool>) -> Box<I64> { return x; }",
        "PTN_TYPE_MISMATCH",
    );
    rejects(
        "enum E<T> { A(T), B } fn f(x: E<Bool>) -> I64 { return match (x) { E::A(true) => 1, E::B => 0 }; }",
        "PTN_TYPE_NONEXHAUSTIVE_MATCH",
    );
    rejects(
        "enum E<T> { A(T), B } fn f() -> Unit { let x = E::B; }",
        "PTN_TYPE_CANNOT_INFER",
    );
    rejects(
        "struct Empty<T> {} fn f() -> Unit { let x = Empty {}; }",
        "PTN_TYPE_CANNOT_INFER",
    );
    rejects(
        "struct Box<T> { value: T } fn f(x: Box) -> Unit {}",
        "PTN_TYPE_ARITY",
    );
    rejects("fn f<T>(x: T<I64>) -> Unit {}", "PTN_TYPE_ARITY");
}

#[test]
fn generic_recursion_is_rejected_even_through_nongeneric_helpers() {
    for source in [
        "fn f<T>(x: T) -> T { return f(x); }",
        "fn f<T>(x: T) -> Unit { g(x); } fn g<U>(x: U) -> Unit { f(x); }",
        "fn f<T>(x: T) -> Unit { g(); } fn g() -> Unit { f(1); }",
        "fn f<T>(x: T) -> Unit { f([x]); }",
    ] {
        rejects(source, "PTN_TYPE_GENERIC_RECURSION");
    }
}

#[test]
fn explicit_constructor_arguments_and_public_generic_imports() {
    accepts("enum E<T> { V(T) } fn f() -> E<I64> { return E::V::<I64>(1); }");
    accepts("fn f() -> Result<I64, String> { return Result::Ok::<I64, String>(1); }");
    rejects(
        "enum E<T> { V(T) } fn f() -> E<Bool> { return E::V::<I64>(true); }",
        "PTN_TYPE_MISMATCH",
    );
    rejects(
        "fn f() -> Option<I64> { return Option::Some::<I64, Bool>(1); }",
        "PTN_TYPE_ARITY",
    );
    let result = check(&[
        file(
            "src/data.ptn",
            "module data; pub struct Box<T> { value: T } pub fn box<T>(x: T) -> Box<T> { return Box { value: x }; }",
        ),
        file(
            "src/main.ptn",
            "module main; import data; fn f() -> Bool { let b: data::Box<Bool> = data::box::<Bool>(true); return b.value; }",
        ),
    ]);
    assert!(
        result.is_success(),
        "{:?} {:?}",
        result.resolution.diagnostics,
        result.diagnostics
    );
    assert!(
        result
            .symbol_types
            .values()
            .any(|t| matches!(t, ValueType::Applied(_, ts) if ts == &[ValueType::Bool]))
    );
    assert!(
        result
            .symbol_types
            .values()
            .any(|t| matches!(t, ValueType::Parameter(_)))
    );
}

#[test]
fn recursive_generic_data_and_type_growth_remain_bounded() {
    accepts(
        "enum Tree<T> { Leaf(T), Branch(Tree<T>) } fn f<T>(x: Tree<T>) -> Bool { return match (x) { Tree::Leaf(_) => true, Tree::Branch(_) => false }; }",
    );
    let mut source = "struct Box<T> { value: T } fn f() -> Unit { let v0 = 1;".to_owned();
    for n in 1..100 {
        source.push_str(&format!("let v{n} = Box {{ value: v{} }};", n - 1));
    }
    source.push('}');
    rejects(&source, "PTN_TYPE_LIMIT");
}
