use pactern::frontend::{SourceInput, SourceSnapshot};
use pactern::syntax::FileKind;
use serde_json::{Value, to_value};

fn input(path: &str, text: &str) -> SourceInput {
    SourceInput {
        path: path.into(),
        bytes: text.as_bytes().to_vec(),
        kind: FileKind::Implementation,
    }
}

#[test]
fn revisions_bind_exact_bytes_paths_kinds_and_closed_source_set() {
    let a = input("src/a.ptn", "module a;");
    let b = input("src/b.ptn", "module b;");
    let revision = SourceSnapshot::new(vec![a.clone(), b.clone()])
        .unwrap()
        .revision()
        .to_owned();
    assert_eq!(
        revision,
        SourceSnapshot::new(vec![b.clone(), a.clone()])
            .unwrap()
            .revision()
    );
    assert_ne!(
        revision,
        SourceSnapshot::new(vec![a.clone()]).unwrap().revision()
    );
    let mut changed = a.clone();
    changed.bytes.push(b' ');
    assert_ne!(
        revision,
        SourceSnapshot::new(vec![changed, b.clone()])
            .unwrap()
            .revision()
    );
    let mut changed = a.clone();
    changed.kind = FileKind::Contract;
    assert_ne!(
        revision,
        SourceSnapshot::new(vec![changed, b.clone()])
            .unwrap()
            .revision()
    );
    let mut changed = a;
    changed.path = "src/renamed.ptn".into();
    assert_ne!(
        revision,
        SourceSnapshot::new(vec![changed, b]).unwrap().revision()
    );
    assert!(revision.starts_with("sha256:"));
    assert_eq!(revision.len(), 71);
}

#[test]
fn invalid_and_duplicate_paths_are_rejected_before_compilation() {
    for path in [
        "",
        "/a.ptn",
        "a/../b.ptn",
        "./a.ptn",
        "a//b.ptn",
        "a/",
        "a\\b.ptn",
        "C:a.ptn",
        "a\n.ptn",
    ] {
        let error = SourceSnapshot::new(vec![input(path, "module a;")]).unwrap_err();
        assert_eq!(error.code(), "PTN_TOOL_INVALID_ARGUMENTS", "{path:?}");
    }
    assert!(
        SourceSnapshot::new(vec![
            input("src/a.ptn", "module a;"),
            input("src/a.ptn", "module a;")
        ])
        .is_err()
    );
    assert!(SourceSnapshot::new(vec![input("src/é.ptn", "module a;")]).is_ok());
}

#[test]
fn syntax_errors_are_revision_bound_and_use_utf8_byte_offsets() {
    let text = "module main; // é\r\nfn run() -> Unit { return @; }";
    let snapshot = SourceSnapshot::new(vec![input("src/main.ptn", text)]).unwrap();
    let diagnostics = snapshot.check();
    assert_eq!(diagnostics.len(), 1);
    let json = to_value(&diagnostics[0]).unwrap();
    assert_eq!(json["code"], "PTN_LEX_INVALID_CHARACTER");
    assert_eq!(json["phase"], "lex");
    assert_eq!(json["source_revision"], snapshot.revision());
    let at = text.find('@').unwrap();
    assert_eq!(json["span"]["path"], "src/main.ptn");
    assert_eq!(json["span"]["start_byte"], at);
    assert_eq!(json["span"]["end_byte"], at + 1);
    // A partial package must never generate cascaded resolution errors.
    let snapshot = SourceSnapshot::new(vec![
        input("src/a.ptn", "module a; import missing;"),
        input("src/b.ptn", "module b; fn"),
    ])
    .unwrap();
    assert_eq!(snapshot.check().len(), 1);
    assert_eq!(to_value(&snapshot.check()[0]).unwrap()["phase"], "parse");
}

#[test]
fn resolution_and_typing_preserve_related_file_locations() {
    let snapshot = SourceSnapshot::new(vec![input(
        "src/main.ptn",
        "module main; fn same()->Unit {} fn same()->Unit {}",
    )])
    .unwrap();
    let json = to_value(&snapshot.check()[0]).unwrap();
    assert_eq!(json["code"], "PTN_RESOLVE_DUPLICATE_DECLARATION");
    assert_eq!(json["phase"], "resolve");
    assert_eq!(json["related"][0]["span"]["path"], "src/main.ptn");

    let snapshot = SourceSnapshot::new(vec![
        input(
            "src/main.ptn",
            "module main; import helper; fn run()->I64 {return helper::take(true);}",
        ),
        input(
            "src/helper.ptn",
            "module helper; pub fn take(x:I64)->I64 {return x;}",
        ),
    ])
    .unwrap();
    let errors: Vec<Value> = snapshot
        .check()
        .iter()
        .map(|error| to_value(error).unwrap())
        .collect();
    let mismatch = errors
        .iter()
        .find(|error| error["code"] == "PTN_TYPE_MISMATCH")
        .unwrap();
    assert_eq!(mismatch["span"]["path"], "src/main.ptn");
    assert_eq!(mismatch["phase"], "type");
    assert!(errors.iter().all(|error| error.get("symbol_id").is_none()));
    for error in errors {
        assert_eq!(error["protocol_version"], "0.1.0");
        assert_eq!(error["kind"], "diagnostic");
        assert_eq!(error["source_revision"], snapshot.revision());
    }
}

#[test]
fn invalid_utf8_has_no_misleading_scalar_span_and_binds_raw_bytes() {
    let mut source = input("src/main.ptn", "module main; // ");
    source.bytes.push(255);
    let snapshot = SourceSnapshot::new(vec![source]).unwrap();
    let json = to_value(&snapshot.check()[0]).unwrap();
    assert_eq!(json["code"], "PTN_LEX_INVALID_UTF8");
    assert_eq!(json["phase"], "lex");
    assert!(json.get("span").is_none());
    assert_eq!(json["source_revision"], snapshot.revision());
    assert!(json["notes"][0].as_str().unwrap().contains("src/main.ptn"));
}

#[test]
fn successful_check_is_core_value_typing_only() {
    let snapshot = SourceSnapshot::new(vec![input(
        "src/main.ptn",
        "module main; fn answer()->I64 {return 42;}",
    )])
    .unwrap();
    assert!(snapshot.check().is_empty());
}
