use pactern::{
    resolution::ParsedFile,
    syntax::{FileKind, SourceFile, parse},
    typing,
};
fn check(text: &str) -> typing::CheckResult {
    let source = SourceFile::new("src/m.ptn", text);
    let tree = parse(&source, FileKind::Implementation).unwrap();
    typing::check(&[ParsedFile::new(source.name(), tree)])
}
fn rejects(text: &str, code: &str) {
    let c = check(text);
    assert!(!c.is_success(), "accepted {text}");
    assert!(c.diagnostics.iter().any(|e| e.code == code), "{:?}", c);
}
#[test]
fn declared_effects_are_upper_bounds_even_in_dead_code() {
    assert!(check("module m; fn a() -> Unit effects {db.read} {} fn b() -> Unit effects {db.read, db.write} {a();}").is_success());
    rejects(
        "module m; fn a() -> Unit effects {db.read} {} fn b() -> Unit {if(false){a();}}",
        "PTN_EFFECT_BOUND",
    );
    rejects(
        "module m; fn a() -> Unit effects {invented.label} {}",
        "PTN_EFFECT_UNKNOWN",
    );
    rejects(
        "module m; fn a() -> Unit effects {db.read,db.read} {}",
        "PTN_EFFECT_DUPLICATE",
    );
}
#[test]
fn ownership_moves_reborrows_and_rejects_reuse() {
    let good = "module m; fn peek(t: borrow host::quota::Txn) -> I64 effects {db.read} {return host::quota::read(borrow t);} fn f(t: host::quota::Txn) -> Unit effects {db.read} {let next=t;let a=peek(borrow next);let b=peek(borrow next);host::quota::rollback(next);}";
    let c = check(good);
    assert!(c.is_success(), "{c:?}");
    rejects(
        "module m; fn f(t: host::quota::Txn) -> Unit {let next=t;host::quota::rollback(t);}",
        "PTN_HANDLE_USE_AFTER_MOVE",
    );
    rejects(
        "module m; fn f(t: borrow host::quota::Txn) -> Unit {let next=t;}",
        "PTN_HANDLE_BORROW_MOVE",
    );
    rejects(
        "module m; fn f(t: host::quota::Txn) -> Unit {let mut next=t;}",
        "PTN_HANDLE_MUTABLE",
    );
    rejects(
        "module m; fn f(t: host::quota::Txn) -> host::quota::Txn {return t;}",
        "PTN_HANDLE_ESCAPE",
    );
    rejects(
        "module m; fn f(t: host::quota::Txn) -> Unit {let xs=[t];}",
        "PTN_HANDLE_ESCAPE",
    );
}
#[test]
fn exclusive_borrow_covers_later_argument_evaluation() {
    let header = "module m; fn pair(t: borrow host::quota::Txn,n:I64) -> Unit {} fn f(t: host::quota::Txn) -> Unit effects {db.read} ";
    rejects(
        &format!("{header} {{pair(borrow t,host::quota::read(borrow t));}}"),
        "PTN_HANDLE_BORROW_OVERLAP",
    );
    rejects(
        "module m; fn f(t:host::quota::Txn) -> I64 effects {db.read} {return host::quota::read(t);}",
        "PTN_HANDLE_BORROW_MODE",
    );
}
#[test]
fn joins_loops_generics_and_contracts_do_not_leak_handles_or_effects() {
    rejects(
        "module m; fn f(t:host::quota::Txn,b:Bool) -> Unit {if(b){host::quota::rollback(t);}host::quota::rollback(t);}",
        "PTN_HANDLE_USE_AFTER_MOVE",
    );
    rejects(
        "module m; fn f(t:host::quota::Txn,b:Bool) -> Unit {while(b){host::quota::rollback(t);}}",
        "PTN_HANDLE_LOOP_MOVE",
    );
    rejects(
        "module m; fn id<T>(x:T)->T{return x;} fn f(t:host::quota::Txn)->Unit{let copy=id(t);}",
        "PTN_HANDLE_ESCAPE",
    );
    rejects(
        "module m; enum E {Txn(host::quota::Txn),}",
        "PTN_HANDLE_ESCAPE",
    );
    let files=[("src/m.ptn",FileKind::Implementation,"module m; fn external()->Bool effects {db.read}{return true;} fn f()->Bool{return true;}"),("contracts/m.pact.ptn",FileKind::Contract,"module m; pact p for f {ensures external();}")].map(|(path,kind,text)|ParsedFile::new(path,parse(&SourceFile::new(path,text),kind).unwrap()));
    assert!(
        typing::check(&files)
            .diagnostics
            .iter()
            .any(|e| e.code == "PTN_EFFECT_BOUND")
    );
}
#[test]
fn imported_effect_bounds_are_preserved() {
    let files = [
        ("src/a.ptn", "module a;pub fn f()->Unit effects{db.write}{}"),
        ("src/b.ptn", "module b;import a;fn f()->Unit {a::f();}"),
    ]
    .map(|(path, text)| {
        ParsedFile::new(
            path,
            parse(&SourceFile::new(path, text), FileKind::Implementation).unwrap(),
        )
    });
    assert!(
        typing::check(&files)
            .diagnostics
            .iter()
            .any(|e| e.code == "PTN_EFFECT_BOUND")
    );
}

#[test]
fn aggregate_and_match_handle_paths_are_rejected_without_panics() {
    for text in [
        "module m; struct Wrap<T> { value:T } fn f(t:host::quota::Txn)->Unit {let x=Wrap {value:t};}",
        "module m; enum Wrap<T> { Value(T), } fn f(t:host::quota::Txn)->Unit {let x=Wrap::Value(t);}",
        "module m; fn f(t:host::quota::Txn)->Unit {match (t) {x => host::quota::rollback(x)};}",
        "module m; fn f(t:host::quota::Txn,b:Bool)->Unit {let x=match (b) {true=>t,false=>t};}",
        "module m; fn f(t:host::quota::Txn,b:Bool)->Unit {match (b) {true=>host::quota::rollback(t),false=>()};host::quota::rollback(t);}",
        "module m; fn empty<T>()->Unit {} fn f()->Unit {empty::<host::quota::Txn>();}",
        "module m; fn f(t:host::quota::Txn)->Unit {let x=Option::Some(t);}",
        "module m; struct Wrap { value:host::quota::Txn }",
    ] {
        let checked = check(text);
        assert!(!checked.is_success(), "accepted an affine escape: {text}");
        assert!(
            checked
                .diagnostics
                .iter()
                .any(|error| error.code.starts_with("PTN_HANDLE")),
            "{checked:?}"
        );
    }
}
