use pactern::{
    core::{self, DeclarationKind, Program},
    interpreter::{self, Limits, TrapKind, Value},
    resolution::{ParsedFile, SymbolId, SymbolKind},
    syntax::{FileKind, SourceFile, parse},
    typing::ValueType,
};
use serde_json::{Value as Json, json};
fn program(path: &str, source: &str) -> Program {
    core::lower(&[ParsedFile::new(
        path,
        parse(&SourceFile::new(path, source), FileKind::Implementation).unwrap(),
    )])
    .unwrap()
}
fn id(p: &Program, name: &str) -> SymbolId {
    p.resolution()
        .symbols
        .iter()
        .find(|s| s.name == name && s.kind == SymbolKind::Function)
        .unwrap()
        .id
}
fn run(source: &str) -> Result<Value, interpreter::Trap> {
    let p = program("src/main.ptn", &format!("module main; {source}"));
    interpreter::execute(&p, id(&p, "run"), vec![], Limits::default())
}
fn decode(p: &Program, t: &ValueType, j: &Json) -> Value {
    match t {
        ValueType::I64 => Value::I64(j.as_str().unwrap().parse().unwrap()),
        ValueType::String => Value::String(j.as_str().unwrap().into()),
        ValueType::Bool => Value::Bool(j.as_bool().unwrap()),
        ValueType::List(t) => Value::List(
            j.as_array()
                .unwrap()
                .iter()
                .map(|j| decode(p, t, j))
                .collect(),
        ),
        ValueType::Option(t) => {
            let name = j["tag"].as_str().unwrap();
            let variant = p
                .resolution()
                .symbols
                .iter()
                .find(|s| s.kind == SymbolKind::BuiltinVariant && s.name == name)
                .unwrap()
                .id;
            Value::Variant {
                variant,
                payload: if name == "Some" {
                    vec![decode(p, t, &j["value"])]
                } else {
                    vec![]
                },
            }
        }
        _ => panic!("unhandled fixture input {t:?}"),
    }
}
fn encode(p: &Program, v: Value) -> Json {
    match v {
        Value::Bool(v) => json!(v),
        Value::I64(v) => json!(v.to_string()),
        Value::String(v) => json!(v),
        Value::Unit => Json::Null,
        Value::List(vs) => Json::Array(vs.into_iter().map(|v| encode(p, v)).collect()),
        Value::Record(fields) => Json::Object(
            fields
                .into_iter()
                .map(|(id, v)| (p.resolution().symbols[id.0].name.clone(), encode(p, v)))
                .collect(),
        ),
        Value::Variant {
            variant,
            mut payload,
        } => {
            let name = &p.resolution().symbols[variant.0].name;
            if payload.is_empty() {
                json!(name)
            } else {
                let value = encode(p, payload.remove(0));
                if name == "Err" {
                    json!({"tag":name,"error":value})
                } else {
                    json!({"tag":name,"value":value})
                }
            }
        }
    }
}
#[test]
fn recorded_pure_execution_cases() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/conformance");
    let manifest: Json =
        serde_json::from_str(&std::fs::read_to_string(root.join("manifest.json")).unwrap())
            .unwrap();
    let mut count = 0;
    for case in manifest["cases"].as_array().unwrap() {
        let expected = &case["expected"];
        let trap = match expected["category"].as_str() {
            Some("bounds") => Some(TrapKind::IndexOutOfBounds),
            Some("integer_overflow") => Some(TrapKind::Overflow),
            Some("division_by_zero") => Some(TrapKind::DivisionByZero),
            _ => None,
        };
        if expected["kind"] != "return" && trap.is_none() {
            continue;
        }
        let source = std::fs::read_to_string(root.join(case["source"].as_str().unwrap())).unwrap();
        let module = source
            .split(';')
            .next()
            .unwrap()
            .strip_prefix("module ")
            .unwrap();
        let p = program(&format!("src/{}.ptn", module.replace("::", "/")), &source);
        let name = case["entry"].as_str().unwrap().rsplit("::").next().unwrap();
        let function = id(&p, name);
        let DeclarationKind::Function { parameters, .. } = &p
            .declarations()
            .iter()
            .find(|d| d.symbol == function)
            .unwrap()
            .kind
        else {
            panic!()
        };
        let args = parameters
            .iter()
            .zip(case["args"].as_array().unwrap())
            .map(|(id, j)| decode(&p, &p.symbol_types()[id], j))
            .collect();
        let result = interpreter::execute(&p, function, args, Limits::default());
        if let Some(kind) = trap {
            let error = result.unwrap_err();
            assert_eq!(error.kind, kind, "{}", case["id"]);
            assert!(error.origin.is_some());
        } else {
            assert_eq!(
                encode(&p, result.unwrap()),
                expected["value"],
                "{}",
                case["id"]
            );
        }
        count += 1;
    }
    assert_eq!(count, 48);
}
#[test]
fn loops_blocks_returns_and_generic_helpers() {
    assert_eq!(
        run("fn identity<T>(v:T)->T {return v;} fn run()->I64 {
        let mut total=0; let mut i=0;
        while (i<10) {i=i+1; if (i==2) {continue;} else if (i==5) {break;}
        {let n=identity(i); total=total+n;}}
        return total;
    }")
        .unwrap(),
        Value::I64(8)
    );
    assert_eq!(
        run("fn run()->I64 {while (true) {{return 42;}} return 0;}").unwrap(),
        Value::I64(42)
    );
    assert_eq!(run("fn run()->Unit {}").unwrap(), Value::Unit);
}
#[test]
fn generic_data_nested_patterns_and_value_semantics() {
    assert_eq!(
        run("enum E<T>{V(Option<T>),Empty} struct Box<T>{value:T}
        fn make<T>(v:T)->E<T>{return E::V(Option::Some(v));}
        fn run()->I64 {let mut x=Box {value:make(7)};let y=x;
        x=Box {value:E::Empty};
        return match (y.value) {E::V(Option::Some(v))=>v,E::V(Option::None)=>0,E::Empty=>1};}")
        .unwrap(),
        Value::I64(7)
    );
}
#[test]
fn recursion_and_limits_are_located() {
    assert_eq!(run("fn fib(n:I64)->I64 {if (n<2) {return n;} return fib(n-1)+fib(n-2);} fn run()->I64{return fib(10);}").unwrap(),Value::I64(55));
    for source in [
        "fn run()->Unit {while (true) {}}",
        "fn run()->Unit {run();}",
    ] {
        let error = run(source).unwrap_err();
        assert_eq!(error.kind, TrapKind::ExecutionLimit);
        assert!(error.origin.is_some());
    }
    assert_eq!(run("enum Tree {Leaf, Node(Tree,Tree)} fn run()->Unit {let mut t=Tree::Leaf; while (true) {t=Tree::Node(t,t);}}").unwrap_err().kind,TrapKind::ExecutionLimit);
}
#[test]
fn invocations_validate_ids_arity_and_nested_values() {
    let p = program(
        "src/main.ptn",
        "module main; fn f(x:Option<I64>)->I64 {return 1;} fn generic<T>(x:T)->T{return x;}",
    );
    for (function, args) in [
        (SymbolId(usize::MAX), vec![]),
        (id(&p, "f"), vec![]),
        (id(&p, "f"), vec![Value::Bool(true)]),
        (id(&p, "generic"), vec![Value::I64(1)]),
    ] {
        assert_eq!(
            interpreter::execute(&p, function, args, Limits::default())
                .unwrap_err()
                .kind,
            TrapKind::InvalidInvocation
        );
    }
    let ok = p
        .resolution()
        .symbols
        .iter()
        .find(|s| s.name == "Ok" && s.kind == SymbolKind::BuiltinVariant)
        .unwrap()
        .id;
    assert_eq!(
        interpreter::execute(
            &p,
            id(&p, "f"),
            vec![Value::Variant {
                variant: ok,
                payload: vec![Value::I64(1)]
            }],
            Limits::default()
        )
        .unwrap_err()
        .kind,
        TrapKind::InvalidInvocation
    );
}
#[test]
fn record_source_order_and_unselected_arms() {
    assert_eq!(run("struct R {a:I64,b:I64} fn run()->I64 {let r=R {b:1/0,a:9223372036854775807+1};return r.a;}").unwrap_err().kind,TrapKind::DivisionByZero);
    assert_eq!(
        run("fn run()->I64 {return match (true) {true=>3,false=>1/0};}").unwrap(),
        Value::I64(3)
    );
    assert_eq!(
        run("fn run()->I64 {return -9223372036854775808-1;}")
            .unwrap_err()
            .kind,
        TrapKind::Overflow
    );
    assert_eq!(
        run("fn run()->I64 {return 9223372036854775807*2;}")
            .unwrap_err()
            .kind,
        TrapKind::Overflow
    );
}

#[test]
fn aggregate_arguments_and_explicit_limits() {
    let p = program(
        "src/main.ptn",
        "module main; struct Box<T>{value:T} enum E<T>{V(T)}
        fn run(x:Box<E<I64>>, y:Result<I64,String>)->I64 {
            return match (x.value) {E::V(n)=>n};
        }",
    );
    let field = p
        .resolution()
        .symbols
        .iter()
        .find(|s| s.name == "value")
        .unwrap()
        .id;
    let variant = p
        .resolution()
        .symbols
        .iter()
        .find(|s| s.name == "V")
        .unwrap()
        .id;
    let err = p
        .resolution()
        .symbols
        .iter()
        .find(|s| s.name == "Err" && s.kind == SymbolKind::BuiltinVariant)
        .unwrap()
        .id;
    let args = vec![
        Value::Record(
            [(
                field,
                Value::Variant {
                    variant,
                    payload: vec![Value::I64(9)],
                },
            )]
            .into(),
        ),
        Value::Variant {
            variant: err,
            payload: vec![Value::String("recoverable".into())],
        },
    ];
    assert_eq!(
        interpreter::execute(&p, id(&p, "run"), args.clone(), Limits::default()).unwrap(),
        Value::I64(9)
    );
    for limits in [
        Limits {
            fuel: 0,
            call_depth: 64,
        },
        Limits {
            fuel: 1000,
            call_depth: 0,
        },
    ] {
        assert_eq!(
            interpreter::execute(&p, id(&p, "run"), args.clone(), limits)
                .unwrap_err()
                .kind,
            TrapKind::ExecutionLimit
        );
    }
    let mut bad = args;
    bad[0] = Value::Record(
        [(
            field,
            Value::Variant {
                variant,
                payload: vec![Value::Bool(true)],
            },
        )]
        .into(),
    );
    assert_eq!(
        interpreter::execute(&p, id(&p, "run"), bad, Limits::default())
            .unwrap_err()
            .kind,
        TrapKind::InvalidInvocation
    );
    assert_eq!(run("enum Chain {End, Next(Chain)} fn run()->Unit {let mut x=Chain::End;while (true) {x=Chain::Next(x);}}").unwrap_err().kind,TrapKind::ExecutionLimit);
}
