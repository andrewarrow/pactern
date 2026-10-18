use pactern::{
    backend::{self, Execution, HostLimits},
    core::{self, DeclarationKind, Program},
    interpreter::{self, Limits, TrapKind, Value},
    resolution::{ParsedFile, SymbolId, SymbolKind},
    syntax::{FileKind, SourceFile, parse},
    typing::ValueType,
};
use serde_json::Value as Json;
fn program(path: &str, text: &str) -> Program {
    core::lower(&[ParsedFile::new(
        path,
        parse(&SourceFile::new(path, text), FileKind::Implementation).unwrap(),
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
fn expected(result: Result<Value, interpreter::Trap>) -> Execution {
    match result {
        Ok(v) => Execution::Value(v),
        Err(t) => Execution::Trap {
            kind: t.kind,
            cause: t.cause.map(|t| t.kind),
        },
    }
}
fn compiled(p: &Program, function: SymbolId, args: &[Value]) -> Execution {
    let c = backend::emit_c(p, function, args, Limits::default()).unwrap();
    let wasm = backend::compile_wasm(&c).unwrap();
    backend::run_wasm(&wasm, HostLimits::default()).unwrap()
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
            Value::Variant {
                variant: p
                    .resolution()
                    .symbols
                    .iter()
                    .find(|s| s.kind == SymbolKind::BuiltinVariant && s.name == name)
                    .unwrap()
                    .id,
                payload: if name == "Some" {
                    vec![decode(p, t, &j["value"])]
                } else {
                    vec![]
                },
            }
        }
        _ => panic!("unsupported fixture input {t:?}"),
    }
}
#[test]
fn all_48_recorded_pure_cases_agree_with_wasm() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/conformance");
    let manifest: Json =
        serde_json::from_str(&std::fs::read_to_string(root.join("manifest.json")).unwrap())
            .unwrap();
    let mut count = 0;
    for case in manifest["cases"].as_array().unwrap() {
        let category = case["expected"]["category"].as_str();
        if case["expected"]["kind"] != "return"
            && !matches!(
                category,
                Some("bounds" | "integer_overflow" | "division_by_zero")
            )
        {
            continue;
        }
        let text = std::fs::read_to_string(root.join(case["source"].as_str().unwrap())).unwrap();
        let module = text
            .split(';')
            .next()
            .unwrap()
            .strip_prefix("module ")
            .unwrap();
        let p = program(&format!("src/{}.ptn", module.replace("::", "/")), &text);
        let function = id(
            &p,
            case["entry"].as_str().unwrap().rsplit("::").next().unwrap(),
        );
        let DeclarationKind::Function { parameters, .. } = &p
            .declarations()
            .iter()
            .find(|d| d.symbol == function)
            .unwrap()
            .kind
        else {
            panic!()
        };
        let args: Vec<_> = parameters
            .iter()
            .zip(case["args"].as_array().unwrap())
            .map(|(id, j)| decode(&p, &p.symbol_types()[id], j))
            .collect();
        assert_eq!(
            compiled(&p, function, &args),
            expected(interpreter::execute(&p, function, args, Limits::default())),
            "{}",
            case["id"]
        );
        count += 1;
    }
    assert_eq!(count, 48);
}
#[test]
fn loops_generic_records_strings_patterns_and_order() {
    let text = r#"module main; struct Box<T>{value:T} enum Choice { Pair(I64,String), Empty }
    fn identity<T>(x:T)->T{return x;}
    fn run()->I64{
       let mut i=0;let mut sum=0;
       while(i<9){i=i+1;if(i==2){continue;}else if(i==6){break;}{let x=identity(i);sum=sum+x;}}
       let b=Box{value:["é\u{1f600}","a\u{0}b"]};let c=Choice::Pair(sum,b.value[0]);
       return match(c){Choice::Pair(n,"é\u{1f600}")=>n,Choice::Pair(_,_)=>0,Choice::Empty=>-1};
    }"#;
    let p = program("src/main.ptn", text);
    let f = id(&p, "run");
    assert_eq!(
        compiled(&p, f, &[]),
        expected(interpreter::execute(&p, f, vec![], Limits::default()))
    );
}
#[test]
fn seeded_arithmetic_boundary_differential() {
    let p = program(
        "src/main.ptn",
        "module main;fn run(a:I64,b:I64)->I64{if(b==0){return a;}return (a*b)+(a/b)-(a%b);}",
    );
    let f = id(&p, "run");
    let mut seed = 0x5eed_u64;
    for i in 0..24 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let a = if i % 3 == 0 {
            seed as i64
        } else {
            (seed % 10000) as i64 - 5000
        };
        seed = seed.rotate_left(17);
        let b = (seed % 101) as i64 - 50;
        let args = vec![Value::I64(a), Value::I64(b)];
        assert_eq!(
            compiled(&p, f, &args),
            expected(interpreter::execute(&p, f, args, Limits::default())),
            "seed iteration {i}"
        );
    }
}
#[test]
fn ambient_imports_and_memory_are_rejected_and_fuel_bounds_execution() {
    // One unused function import is still an authority request and must be denied.
    let imported = b"\0asm\x01\0\0\0\x01\x04\x01\x60\0\0\x02\x0a\x01\x03env\x02io\0\0";
    assert!(
        backend::run_wasm(imported, HostLimits::default())
            .unwrap_err()
            .to_string()
            .contains("ambient import denied")
    );
    let p = program(
        "src/main.ptn",
        "module main;fn run()->I64{while(true){}return 0;}",
    );
    let c = backend::emit_c(
        &p,
        id(&p, "run"),
        &[],
        Limits {
            fuel: u64::MAX,
            call_depth: 64,
        },
    )
    .unwrap();
    let wasm = backend::compile_wasm(&c).unwrap();
    assert!(
        backend::run_wasm(
            &wasm,
            HostLimits {
                memory_bytes: 65536,
                ..HostLimits::default()
            }
        )
        .is_err()
    );
    assert_eq!(
        backend::run_wasm(
            &wasm,
            HostLimits {
                fuel: 10000,
                ..HostLimits::default()
            }
        )
        .unwrap(),
        Execution::Trap {
            kind: TrapKind::ExecutionLimit,
            cause: None
        }
    );
}
#[test]
fn precise_gc_reclaims_dead_loop_values_and_preserves_live_roots() {
    let p = program(
        "src/main.ptn",
        "module main;fn run()->List<I64>{let keep=[41,42];let mut i=0;while(i<150){let waste=[i,i,i,i,i,i,i,i];i=i+1;}return keep;}",
    );
    let c = backend::emit_c(&p, id(&p, "run"), &[], Limits::default()).unwrap();
    let c = format!("#define PT_HEAP_SIZE 2048\n{c}");
    let wasm = backend::compile_wasm(&c).unwrap();
    assert_eq!(
        backend::run_wasm(&wasm, HostLimits::default()).unwrap(),
        Execution::Value(Value::List(vec![Value::I64(41), Value::I64(42)]))
    );
}
#[test]
fn malformed_frontend_boundaries_never_panic() {
    let atoms = [
        "module main;",
        "fn",
        "{",
        "}",
        "return",
        "0",
        "-9223372036854775808",
        "Some",
        "::",
        "<",
        ">",
        ",",
        "\"é\"",
        "/*",
        "*/",
    ];
    let mut seed = 73u64;
    for _ in 0..500 {
        let mut text = String::new();
        for _ in 0..16 {
            seed = seed
                .wrapping_mul(2862933555777941757)
                .wrapping_add(3037000493);
            text.push_str(atoms[seed as usize % atoms.len()]);
            text.push(' ');
        }
        let source = SourceFile::new("src/main.ptn", text);
        if let Ok(tree) = parse(&source, FileKind::Implementation) {
            let _ = core::lower(&[ParsedFile::new(source.name(), tree)]);
        }
    }
}

#[test]
fn logical_fuel_and_minimum_integer_multiplication_regressions() {
    let p = program(
        "src/main.ptn",
        "module main;fn run(a:I64,b:I64)->I64{return a*b;}",
    );
    let f = id(&p, "run");
    for (a, b) in [
        (i64::MIN, -1),
        (i64::MIN, 1),
        (i64::MIN, 0),
        (i64::MAX, 2),
        (-5, -6),
    ] {
        let args = vec![Value::I64(a), Value::I64(b)];
        assert_eq!(
            compiled(&p, f, &args),
            expected(interpreter::execute(&p, f, args, Limits::default()))
        );
    }
    let p = program(
        "src/main.ptn",
        "module main;fn run()->I64{let a=[1,2];return a[1];}",
    );
    let f = id(&p, "run");
    for fuel in [0, 1, 5, 10, 16, 20, 30, 100] {
        let limits = Limits {
            fuel,
            call_depth: 64,
        };
        let c = backend::emit_c(&p, f, &[], limits).unwrap();
        let wasm = backend::compile_wasm(&c).unwrap();
        assert_eq!(
            backend::run_wasm(&wasm, HostLimits::default()).unwrap(),
            expected(interpreter::execute(&p, f, vec![], limits)),
            "fuel {fuel}"
        );
    }
}

#[test]
fn native_runtime_allocation_roots_and_cleanup_under_sanitizers() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runtime");
    let binary =
        std::env::temp_dir().join(format!("pactern-native-runtime-{}", std::process::id()));
    // Native sanitizers must match the host SDK (Apple Clang on macOS).
    let native_compiler = std::env::var("PACTERN_NATIVE_CLANG").unwrap_or_else(|_| "clang".into());
    let compile = std::process::Command::new(native_compiler)
        .args(["-std=c11", "-O1", "-g", "-fsanitize=address,undefined"])
        .arg(root.join("tests.c"))
        .arg("-o")
        .arg(&binary)
        .output()
        .unwrap();
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let mut child = std::process::Command::new(&binary)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while child.try_wait().unwrap().is_none() {
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let run = child.wait_with_output().unwrap();
    let _ = std::fs::remove_file(binary);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn host_rejects_malformed_modules_and_oversized_output_without_panicking() {
    let mut seed = 31u64;
    for n in 0..256 {
        let mut bytes = Vec::new();
        for _ in 0..n {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            bytes.push(seed as u8);
        }
        assert!(backend::run_wasm(&bytes, HostLimits::default()).is_err());
    }
    let malicious = "unsigned pt_run(void){return 0;}unsigned pt_abi_version(void){return 1;}unsigned pt_output_ptr(void){return 0;}unsigned pt_output_len(void){return 4294967295u;}unsigned pt_trap_cause(void){return 0;}unsigned pt_collections(void){return 0;}";
    let wasm = backend::compile_wasm(malicious).unwrap();
    assert!(
        backend::run_wasm(&wasm, HostLimits::default())
            .unwrap_err()
            .to_string()
            .contains("output limit")
    );
}

#[test]
fn identical_checked_invocations_produce_identical_wasm_bytes() {
    let p = program(
        "src/main.ptn",
        "module main;fn run(x:I64)->I64{return x+1;}",
    );
    let c = backend::emit_c(&p, id(&p, "run"), &[Value::I64(41)], Limits::default()).unwrap();
    assert_eq!(
        backend::compile_wasm(&c).unwrap(),
        backend::compile_wasm(&c).unwrap()
    );
}
