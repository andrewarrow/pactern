use pactern::resolution::{ParsedFile, resolve};
use pactern::syntax::{FileKind, SourceFile, parse};

fn file(path: &str, text: &str) -> ParsedFile {
    let kind = if path.starts_with("contracts/") {
        FileKind::Contract
    } else {
        FileKind::Implementation
    };
    ParsedFile {
        path: path.into(),
        tree: parse(&SourceFile::new(path, text), kind).unwrap(),
    }
}

#[test]
fn imports_do_not_leak_between_implementation_and_contract_files() {
    let helper = || {
        file(
            "src/util.ptn",
            "module util; pub fn good() -> Bool { return true; }",
        )
    };
    let implementation = || {
        file(
            "src/main.ptn",
            "module main; import util; fn run() -> Unit { util::good(); }",
        )
    };
    let contract = |import: &str| {
        file(
            "contracts/main.pact.ptn",
            &format!("module main; {import} pact rule for run {{ requires util::good(); }}"),
        )
    };
    let unresolved = resolve(&[helper(), implementation(), contract("")]);
    assert!(!unresolved.diagnostics.is_empty());
    let resolved = resolve(&[helper(), implementation(), contract("import util;")]);
    assert!(
        resolved.diagnostics.is_empty(),
        "{:?}",
        resolved.diagnostics
    );
}

#[test]
fn local_bindings_begin_after_their_initializer_and_hide_module_values() {
    let resolved = resolve(&[file(
        "src/main.ptn",
        r#"
        module main;
        fn seed() -> I64 { return 1; }
        fn run() -> I64 { let seed = seed(); return seed; }
    "#,
    )]);
    assert!(
        resolved.diagnostics.is_empty(),
        "{:?}",
        resolved.diagnostics
    );
    for body in [
        "let fresh = fresh; return fresh;",
        "let seed = 1; return seed();",
    ] {
        let result = resolve(&[file(
            "src/main.ptn",
            &format!("module main; fn seed() -> I64 {{ return 1; }} fn run() -> I64 {{ {body} }}"),
        )]);
        assert!(!result.diagnostics.is_empty(), "accepted {body}");
    }
}

#[test]
fn match_bindings_do_not_leak_to_sibling_arms_or_later_statements() {
    for body in [
        "return match (value) { Option::Some(inner) => inner, Option::None => inner };",
        "let x = match (value) { Option::Some(inner) => inner, Option::None => 0 }; return inner;",
    ] {
        let result = resolve(&[file(
            "src/main.ptn",
            &format!("module main; fn run(value: Option<I64>) -> I64 {{ {body} }}"),
        )]);
        assert!(!result.diagnostics.is_empty(), "accepted {body}");
    }
}

#[test]
fn long_module_chains_and_a_closing_cycle_do_not_recurse_on_the_host_stack() {
    let count = 2048;
    let mut files: Vec<_> = (0..count)
        .map(|index| {
            let import = if index + 1 < count {
                format!("import m{};", index + 1)
            } else {
                String::new()
            };
            file(
                &format!("src/m{index}.ptn"),
                &format!("module m{index}; {import}"),
            )
        })
        .collect();
    let result = resolve(&files);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    files[count - 1] = file(
        &format!("src/m{}.ptn", count - 1),
        &format!("module m{}; import m0;", count - 1),
    );
    let result = resolve(&files);
    assert!(!result.diagnostics.is_empty());
}

#[test]
fn privacy_diagnostics_point_to_exact_uses_and_declarations_across_files() {
    let caller =
        "// café\r\nmodule main;\r\nimport secret;\r\nfn run() -> I64 { return secret::hidden(); }";
    let callee = "// 隠す\r\nmodule secret;\r\nfn hidden() -> I64 { return 1; }";
    let sources = [
        SourceFile::new("src/main.ptn", caller),
        SourceFile::new("src/secret.ptn", callee),
    ];
    let result = resolve(&[
        file(sources[0].name(), sources[0].text()),
        file(sources[1].name(), sources[1].text()),
    ]);
    assert_eq!(result.diagnostics.len(), 1);
    let diagnostic = &result.diagnostics[0];
    assert_eq!(diagnostic.code, "PTN_RESOLVE_PRIVATE_SYMBOL");
    for (location, expected_file, expected_text) in [
        (diagnostic.location, 0, "secret::hidden"),
        (diagnostic.related[0].location, 1, "hidden"),
    ] {
        assert_eq!(location.file.0, expected_file);
        let source = &sources[location.file.0];
        let span = location.span;
        assert_eq!(source.span(span.start.byte, span.end.byte), Some(span));
        assert_eq!(
            &source.text()[span.start.byte..span.end.byte],
            expected_text
        );
    }
}
