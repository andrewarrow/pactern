use pactern::resolution::*;
use pactern::syntax::{FileKind, SourceFile, parse};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

fn file(path: &str, text: &str) -> ParsedFile {
    let kind = if path.ends_with(".pact.ptn") {
        FileKind::Contract
    } else {
        FileKind::Implementation
    };
    ParsedFile::new(path, parse(&SourceFile::new(path, text), kind).unwrap())
}

fn codes(resolution: &Resolution) -> Vec<&str> {
    resolution.diagnostics.iter().map(|d| d.code).collect()
}

fn assert_success(files: &[ParsedFile]) -> Resolution {
    let resolution = resolve(files);
    assert!(resolution.is_success(), "{:#?}", resolution.diagnostics);
    resolution
}

#[test]
fn forward_references_and_imported_generics_constructors_link_to_declarations() {
    let files = [
        file(
            "src/app.ptn",
            r#"module app; import util::values;
            fn run() -> I64 {
                let a = later(1);
                let b = util::values::Box { value: a };
                let c = util::values::Choice::Value(b);
                return match (c) { util::values::Choice::Value(item) => item.value };
            }
            fn later(n: I64) -> I64 { return n; }
        "#,
        ),
        file(
            "src/util/values.ptn",
            r#"module util::values;
            pub struct Box<T> { value: T }
            pub enum Choice<T> { Empty, Value(T) }
        "#,
        ),
    ];
    let resolution = assert_success(&files);
    for name in ["later", "Box", "Choice", "Value", "item", "value"] {
        assert!(
            resolution
                .references
                .iter()
                .any(|r| resolution.symbols[r.symbol.0].name == name),
            "{name}"
        );
    }
    assert!(
        resolution
            .deferred
            .iter()
            .any(|u| u.name == "value" && u.kind == DeferredUseKind::FieldAccess)
    );
    assert_eq!(
        resolution,
        resolve(&files),
        "IDs/results must be deterministic"
    );
}

#[test]
fn builtin_types_and_enum_constructors_resolve_without_user_declarations() {
    assert_success(&[file(
        "src/main.ptn",
        r#"module main;
        fn value<T>(x: T) -> Option<Result<T, String>> {
            let none: Option<I64> = Option::None;
            return Option::Some(Result::Ok(x));
        }
        fn inspect(x: Option<I64>) -> I64 {
            return match (x) { Option::Some(n) => n, Option::None => 0 };
        }
    "#,
    )]);
}

#[test]
fn lexical_scopes_reject_active_shadowing_and_allow_reuse_after_scope_exit() {
    let resolution = resolve(&[file(
        "src/main.ptn",
        r#"module main;
        fn run(parameter: I64) -> Unit {
            { let item = parameter; }
            { let item = parameter; }
            let value = parameter;
            { let value = 2; }
            let parameter = 3;
        }
    "#,
    )]);
    assert_eq!(
        codes(&resolution),
        ["PTN_RESOLVE_DUPLICATE_LOCAL", "PTN_RESOLVE_DUPLICATE_LOCAL"]
    );
    assert!(resolution.diagnostics.iter().all(|d| d.related.len() == 1));
}

#[test]
fn initializer_is_resolved_before_binding_and_branch_names_do_not_escape() {
    let resolution = resolve(&[file(
        "src/main.ptn",
        r#"module main;
        fn run() -> I64 {
            let self_ref = self_ref;
            if (true) { let branch = 1; }
            return branch;
        }
    "#,
    )]);
    assert_eq!(
        codes(&resolution),
        ["PTN_RESOLVE_UNRESOLVED_NAME", "PTN_RESOLVE_UNRESOLVED_NAME"]
    );
    assert!(resolution.diagnostics[0].message.contains("self_ref"));
    assert!(resolution.diagnostics[1].message.contains("branch"));
}

#[test]
fn match_arm_bindings_are_local_and_nested_patterns_reject_duplicate_names() {
    let resolution = resolve(&[file(
        "src/main.ptn",
        r#"module main;
        enum Pair { Two(I64, I64), One(I64) }
        fn run(input: Pair) -> I64 {
            let output = match (input) { Pair::Two(x, x) => x, Pair::One(x) => x };
            return x;
        }
    "#,
    )]);
    assert_eq!(
        codes(&resolution),
        ["PTN_RESOLVE_DUPLICATE_LOCAL", "PTN_RESOLVE_UNRESOLVED_NAME"]
    );
}

#[test]
fn module_declarations_pacts_fields_and_variants_have_their_specified_namespaces() {
    let resolution = resolve(&[
        file(
            "src/main.ptn",
            r#"module main;
            struct Record { value: I64, value: String }
            enum Choice { Value, Value }
            fn duplicate() -> I64 { return 1; }
            struct duplicate {}
        "#,
        ),
        file(
            "contracts/main.pact.ptn",
            "module main; pact Record for duplicate { ensures result == 1; }",
        ),
    ]);
    let codes = codes(&resolution);
    assert!(codes.contains(&"PTN_RESOLVE_DUPLICATE_FIELD"));
    assert!(codes.contains(&"PTN_RESOLVE_DUPLICATE_VARIANT"));
    assert_eq!(
        codes
            .iter()
            .filter(|code| **code == "PTN_RESOLVE_DUPLICATE_DECLARATION")
            .count(),
        2
    );
}

#[test]
fn imports_require_exact_module_and_public_declarations() {
    let resolution = resolve(&[
        file(
            "src/main.ptn",
            r#"module main; import util;
            fn run() -> I64 { return util::math::answer(); }
        "#,
        ),
        file("src/util.ptn", "module util;"),
        file(
            "src/util/math.ptn",
            "module util::math; pub fn answer() -> I64 { return 1; }",
        ),
    ]);
    assert_eq!(codes(&resolution), ["PTN_RESOLVE_UNRESOLVED_NAME"]);
    let resolution = resolve(&[
        file(
            "src/main.ptn",
            "module main; import util; fn run() -> I64 { return util::secret(); }",
        ),
        file(
            "src/util.ptn",
            "module util; fn secret() -> I64 { return 1; }",
        ),
    ]);
    assert_eq!(codes(&resolution), ["PTN_RESOLVE_PRIVATE_SYMBOL"]);
    let diagnostic = &resolution.diagnostics[0];
    assert_eq!(diagnostic.location.file, FileId(0));
    assert_eq!(diagnostic.related[0].location.file, FileId(1));
}

#[test]
fn imported_names_do_not_become_unqualified_and_own_names_must_be_unqualified() {
    let resolution = resolve(&[
        file(
            "src/main.ptn",
            r#"module main; import util;
            fn local() -> I64 { return 1; }
            fn run() -> I64 { let x = answer(); return main::local(); }
        "#,
        ),
        file(
            "src/util.ptn",
            "module util; pub fn answer() -> I64 { return 1; }",
        ),
    ]);
    assert_eq!(
        codes(&resolution),
        [
            "PTN_RESOLVE_UNRESOLVED_NAME",
            "PTN_RESOLVE_QUALIFIED_OWN_MODULE"
        ]
    );
}

#[test]
fn contract_imports_are_separate_and_target_parameter_result_links_are_preserved() {
    let files = [
        file(
            "src/main.ptn",
            "module main; import util; fn increment(value: I64) -> I64 { return util::add(value); }",
        ),
        file(
            "contracts/main.pact.ptn",
            r#"module main; import util;
            pact safety for increment { requires value > 0; ensures result == util::add(value); }
        "#,
        ),
        file(
            "src/util.ptn",
            "module util; pub fn add(value: I64) -> I64 { return value + 1; }",
        ),
    ];
    let resolution = assert_success(&files);
    let parameter = resolution
        .symbols
        .iter()
        .find(|s| s.name == "value" && s.declaration.unwrap().file == FileId(0))
        .unwrap();
    assert_eq!(
        resolution
            .references
            .iter()
            .filter(|r| r.location.file == FileId(1) && r.symbol == parameter.id)
            .count(),
        2
    );
    assert!(
        resolution
            .references
            .iter()
            .any(|r| resolution.symbols[r.symbol.0].kind == SymbolKind::Result)
    );
    let mut no_import = files.to_vec();
    no_import[1].tree.imports.clear();
    assert_eq!(codes(&resolve(&no_import)), ["PTN_RESOLVE_UNRESOLVED_NAME"]);
}

#[test]
fn invalid_pact_targets_and_result_scope_are_located_errors() {
    let resolution = resolve(&[
        file(
            "src/main.ptn",
            "module main; fn generic<T>(x: T) -> T { return x; } fn ordinary() -> I64 { return result; }",
        ),
        file(
            "contracts/main.pact.ptn",
            r#"module main;
            pact generic_rule for generic { ensures true; }
            pact missing_rule for missing { requires true; }
            pact bad_result for ordinary { requires result == 1; ensures result == 1; }
        "#,
        ),
    ]);
    assert_eq!(
        codes(&resolution),
        [
            "PTN_RESOLVE_RESULT_OUTSIDE_POSTCONDITION",
            "PTN_RESOLVE_GENERIC_PACT_TARGET",
            "PTN_RESOLVE_UNRESOLVED_NAME",
            "PTN_RESOLVE_RESULT_OUTSIDE_POSTCONDITION",
        ]
    );
}

#[test]
fn public_function_cannot_expose_private_types_inside_generic_arguments() {
    let resolution = resolve(&[file(
        "src/main.ptn",
        r#"module main;
        struct Hidden {}
        pub struct Visible {}
        pub fn leak(input: List<Option<Hidden>>) -> Result<Visible, Hidden> { return Result::Ok(Visible {}); }
    "#,
    )]);
    assert_eq!(
        codes(&resolution),
        [
            "PTN_RESOLVE_PRIVATE_TYPE_EXPOSURE",
            "PTN_RESOLVE_PRIVATE_TYPE_EXPOSURE"
        ]
    );
    assert!(resolution.diagnostics.iter().all(|d| d.related.len() == 1));
}

#[test]
fn cycles_include_contract_imports_and_self_imports() {
    let resolution = resolve(&[
        file("src/a.ptn", "module a; import b;"),
        file("src/b.ptn", "module b; fn target() -> Unit {}"),
        file(
            "contracts/b.pact.ptn",
            "module b; import a; pact rule for target { ensures true; }",
        ),
    ]);
    assert_eq!(codes(&resolution), ["PTN_RESOLVE_IMPORT_CYCLE"]);
    assert_eq!(resolution.diagnostics[0].related.len(), 2);
    let resolution = resolve(&[file(
        "src/self_import.ptn",
        "module self_import; import self_import;",
    )]);
    assert_eq!(codes(&resolution), ["PTN_RESOLVE_IMPORT_CYCLE"]);
}

#[test]
fn source_identities_and_reserved_names_are_checked_without_io() {
    let resolution = resolve(&[
        file("src/wrong.ptn", "module actual;"),
        file(
            "contracts/orphan.pact.ptn",
            "module orphan; pact rule for missing { ensures true; }",
        ),
        file("src/host/forged.ptn", "module host::forged;"),
        file("src/std.ptn", "module std;"),
        file(
            "src/types.ptn",
            "module types; struct I64 {} fn generic<Bool>() -> Unit {}",
        ),
    ]);
    assert_eq!(
        codes(&resolution),
        [
            "PTN_RESOLVE_MODULE_PATH",
            "PTN_RESOLVE_RESERVED_MODULE",
            "PTN_RESOLVE_RESERVED_MODULE",
            "PTN_RESOLVE_MISSING_IMPLEMENTATION",
            "PTN_RESOLVE_RESERVED_TYPE",
            "PTN_RESOLVE_RESERVED_TYPE",
            "PTN_RESOLVE_UNRESOLVED_NAME",
        ]
    );
}

#[test]
fn registered_host_symbols_resolve_and_effect_labels_are_deferred() {
    let resolution = resolve(&[file(
        "src/main.ptn",
        r#"module main; import host::quota;
        fn run(txn: host::quota::Txn) -> Unit effects { db.read, invented.label } {
            host::quota::read(borrow txn);
        }
    "#,
    )]);
    assert!(resolution.is_success(), "{:?}", resolution.diagnostics);
    assert_eq!(
        resolution
            .deferred
            .iter()
            .filter(|u| u.kind == DeferredUseKind::EffectLabel)
            .count(),
        2
    );
}

#[test]
fn local_binding_hides_function_for_calls_and_missing_borrow_bindings_are_rejected() {
    let resolution = resolve(&[file(
        "src/main.ptn",
        r#"module main;
        fn helper() -> Unit {}
        fn run() -> Unit { let helper = 1; helper(); missing(borrow absent); }
    "#,
    )]);
    assert_eq!(
        codes(&resolution),
        [
            "PTN_RESOLVE_WRONG_SYMBOL_KIND",
            "PTN_RESOLVE_UNRESOLVED_NAME",
            "PTN_RESOLVE_UNRESOLVED_NAME"
        ]
    );
}

#[test]
fn conformance_sources_match_resolution_outcomes_without_claiming_later_phases() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/conformance");
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(root.join("manifest.json")).unwrap()).unwrap();
    let mut covered = BTreeSet::new();
    for case in manifest["cases"].as_array().unwrap() {
        let expected = &case["expected"];
        if matches!(expected["phase"].as_str(), Some("lex" | "parse")) {
            continue;
        }
        let source_path = case["source"].as_str().unwrap();
        let mut files = Vec::new();
        for (path, kind) in std::iter::once((source_path, FileKind::Implementation)).chain(
            case["contracts"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|p| (p.as_str().unwrap(), FileKind::Contract)),
        ) {
            let tree = parse(
                &SourceFile::new(path, fs::read_to_string(root.join(path)).unwrap()),
                kind,
            )
            .unwrap();
            let module = tree
                .module
                .value
                .value
                .iter()
                .map(|i| i.value.as_str())
                .collect::<Vec<_>>()
                .join("/");
            let identity = match kind {
                FileKind::Implementation => format!("src/{module}.ptn"),
                FileKind::Contract => format!("contracts/{module}.pact.ptn"),
            };
            files.push(ParsedFile::new(identity, tree));
        }
        let resolution = resolve(&files);
        if expected["phase"] == "resolve" {
            let code = expected["code"].as_str().unwrap();
            assert!(
                codes(&resolution).contains(&code),
                "{source_path}: {:?}",
                resolution.diagnostics
            );
            covered.insert(code.to_owned());
        } else {
            assert!(
                resolution.is_success(),
                "{source_path}: {:?}",
                resolution.diagnostics
            );
        }
        // File-aware spans always point into their corresponding input source.
        for diagnostic in &resolution.diagnostics {
            assert!(diagnostic.location.file.0 < files.len());
            assert!(diagnostic.location.span.start.byte <= diagnostic.location.span.end.byte);
        }
    }
    assert_eq!(covered.len(), 2);
}

#[test]
fn lexical_type_parameters_hide_module_enum_constructor_names() {
    let resolution = resolve(&[file(
        "src/main.ptn",
        "module main; enum Choice { A } fn run<Choice>() -> Unit { Choice::A; }",
    )]);
    assert_eq!(codes(&resolution), ["PTN_RESOLVE_UNRESOLVED_NAME"]);
}

#[test]
fn function_names_can_match_prelude_type_names_without_hiding_type_annotations() {
    assert_success(&[file(
        "src/main.ptn",
        "module main; fn I64(value: I64) -> I64 { return value; } fn run() -> I64 { return I64(1); }",
    )]);
}

#[test]
fn borrow_arguments_resolve_to_active_binding_symbols() {
    let resolution = assert_success(&[file(
        "src/main.ptn",
        "module main; fn sink(value: I64) -> Unit {} fn run(value: I64) -> Unit { sink(borrow value); }",
    )]);
    let run_parameter = resolution
        .symbols
        .iter()
        .find(|s| {
            s.kind == SymbolKind::Parameter
                && s.parent
                    .is_some_and(|p| resolution.symbols[p.0].name == "run")
        })
        .unwrap();
    assert!(
        resolution
            .references
            .iter()
            .any(|r| r.symbol == run_parameter.id)
    );
}
