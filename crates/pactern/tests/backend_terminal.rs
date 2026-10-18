use pactern::{
    backend::{self, Execution, HostLimits, TerminalIo},
    core,
    interpreter::{Limits, TrapKind, Value},
    resolution::{ParsedFile, SymbolKind},
    syntax::{FileKind, SourceFile, parse},
};
use std::io::{self, Cursor, Write};

fn program(body: &str) -> core::Program {
    let text = format!(
        "module main; import host::terminal; pub fn main(t:host::terminal::Terminal)->Unit effects{{env.read,terminal.read,terminal.write}}{{{body}}}"
    );
    let source = SourceFile::new("src/main.ptn", text);
    core::lower(&[ParsedFile::new(
        source.name(),
        parse(&source, FileKind::Implementation).unwrap(),
    )])
    .unwrap()
}
fn emitted(body: &str) -> String {
    let p = program(body);
    let id = p
        .resolution()
        .symbols
        .iter()
        .find(|s| s.kind == SymbolKind::Function && s.name == "main")
        .unwrap()
        .id;
    backend::emit_c_terminal(&p, id, Limits::default(), true).unwrap()
}
fn wasm(body: &str) -> Vec<u8> {
    backend::compile_wasm(&emitted(body)).unwrap()
}
fn run(
    bytes: &[u8],
    input: &[u8],
    args: &[String],
    budget: usize,
) -> (Result<Execution, backend::BackendError>, Vec<u8>, Vec<u8>) {
    let (mut out, mut err) = (vec![], vec![]);
    let result = backend::run_wasm_terminal(
        bytes,
        HostLimits::default(),
        TerminalIo {
            input: &mut Cursor::new(input),
            output: &mut out,
            error: &mut err,
            arguments: args,
            byte_limit: budget,
        },
    );
    (result, out, err)
}

#[test]
fn arguments_are_dynamic_exact_utf8_values_and_streams_are_separate() {
    let bytes = wasm(
        r#"let args=host::terminal::args(borrow t);
        let count=host::terminal::arg_count(borrow t);
        if(count!=3){host::terminal::write_error(borrow t,"wrong count");}
        host::terminal::write(borrow t,args[0]);
        host::terminal::write(borrow t,args[1]);
        host::terminal::write_error(borrow t,args[2]);"#,
    );
    for args in [
        vec!["--help".into(), "".into(), "é日\"\\".into()],
        vec!["--json".into(), "--".into(), "line\nbreak".into()],
    ] {
        let (result, out, err) = run(&bytes, b"", &args, 4096);
        assert_eq!(result.unwrap(), Execution::Value(Value::Unit));
        assert_eq!(out, format!("{}{}", args[0], args[1]).as_bytes());
        assert_eq!(err, args[2].as_bytes());
    }
    assert!(
        backend::run_wasm(&bytes, HostLimits::default())
            .unwrap_err()
            .to_string()
            .contains("ambient import denied")
    );
}

#[test]
fn lines_distinguish_eof_from_empty_and_normalize_crlf() {
    let bytes = wasm(
        r#"let mut done=false; while(!done){
        let line=host::terminal::read_line(borrow t);
        let text=match(line){Option::Some(value)=>value,Option::None=>"<EOF>"};
        host::terminal::write(borrow t,text);
        host::terminal::write(borrow t,"|");
        done=match(line){Option::Some(_)=>false,Option::None=>true};
    }"#,
    );
    let (result, out, err) = run(&bytes, "\nhello\r\n日\nlast".as_bytes(), &[], 4096);
    assert_eq!(result.unwrap(), Execution::Value(Value::Unit));
    assert_eq!(out, "|hello|日|last|<EOF>|".as_bytes());
    assert!(err.is_empty());
    let (result, out, _) = run(&bytes, b"", &[], 4096);
    assert_eq!(result.unwrap(), Execution::Value(Value::Unit));
    assert_eq!(out, b"<EOF>|");
}

#[test]
fn terminal_values_survive_guest_garbage_collection() {
    let c = emitted(
        r#"let args=host::terminal::args(borrow t);let mut i=0;while(i<100){
        let line=host::terminal::read_line(borrow t);
        let text=match(line){Option::Some(value)=>value,Option::None=>"unexpected EOF"};
        host::terminal::write(borrow t,text);i=i+1;
    }host::terminal::write(borrow t,args[0]);"#,
    );
    let bytes = backend::compile_wasm(&format!("#define PT_HEAP_SIZE 2048\n{c}")).unwrap();
    let (result, out, err) = run(
        &bytes,
        "日\n".repeat(100).as_bytes(),
        &["kept alive".into()],
        4096,
    );
    assert_eq!(result.unwrap(), Execution::Value(Value::Unit));
    assert_eq!(out, format!("{}kept alive", "日".repeat(100)).as_bytes());
    assert!(err.is_empty());
}

#[test]
fn byte_budgets_invalid_utf8_and_long_lines_trap() {
    let bytes = wasm(r#"let line=host::terminal::read_line(borrow t);"#);
    for (input, budget, expected) in [
        (vec![b'x'; 65537], 100000, "line"),
        (vec![255, b'\n'], 10, "utf-8"),
        (b"abc\n".to_vec(), 2, "limit"),
    ] {
        let (result, out, err) = run(&bytes, &input, &[], budget);
        assert!(result.unwrap_err().to_string().contains(expected));
        assert!(out.is_empty() && err.is_empty());
    }
    let bytes = wasm(r#"host::terminal::write(borrow t,"hello");"#);
    assert!(
        run(&bytes, b"", &[], 4)
            .0
            .unwrap_err()
            .to_string()
            .contains("byte limit")
    );
    assert!(
        run(&bytes, b"", &["too long".into()], 4)
            .0
            .unwrap_err()
            .to_string()
            .contains("argument limit")
    );
    let bytes = wasm("while(true){}");
    assert!(matches!(
        run(&bytes, b"", &[], 100).0.unwrap(),
        Execution::Trap {
            kind: TrapKind::ExecutionLimit,
            ..
        }
    ));
}

#[test]
fn forged_stale_tokens_roots_and_imports_are_denied() {
    let c = emitted(r#"host::terminal::write(borrow t,"hello");"#);
    for (before, after, message) in [
        ("pt_terminal_root(0)", "pt_terminal_root(1)", "root denied"),
        (
            "pt_terminal_write((U32)",
            "pt_terminal_write(999+(U32)",
            "handle denied",
        ),
        ("pactern.terminal.v1", "untrusted.terminal", "import denied"),
        (
            "import_name(\"write\")",
            "import_name(\"argc\")",
            "instantiation failed",
        ),
        (
            "pt_terminal_write((U32)",
            "pt_terminal_release(1);pt_terminal_write((U32)",
            "handle denied",
        ),
    ] {
        assert!(c.contains(before));
        let bytes = backend::compile_wasm(&c.replace(before, after)).unwrap();
        let (result, out, err) = run(&bytes, b"", &[], 4096);
        assert!(result.unwrap_err().to_string().contains(message));
        assert!(out.is_empty() && err.is_empty());
    }
}

#[test]
fn stream_errors_propagate_and_writes_flush_prompts() {
    #[derive(Default)]
    struct Output {
        flushed: usize,
        bytes: Vec<u8>,
    }
    impl Write for Output {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            self.flushed += 1;
            Ok(())
        }
    }
    let bytes = wasm(r#"host::terminal::write(borrow t,"Choose: ");"#);
    let mut output = Output::default();
    let result = backend::run_wasm_terminal(
        &bytes,
        HostLimits::default(),
        TerminalIo {
            input: &mut io::empty(),
            output: &mut output,
            error: &mut io::sink(),
            arguments: &[],
            byte_limit: 4096,
        },
    )
    .unwrap();
    assert_eq!(result, Execution::Value(Value::Unit));
    assert_eq!(output.bytes, b"Choose: ");
    assert_eq!(output.flushed, 1);
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let result = backend::run_wasm_terminal(
        &bytes,
        HostLimits::default(),
        TerminalIo {
            input: &mut io::empty(),
            output: &mut Broken,
            error: &mut io::sink(),
            arguments: &[],
            byte_limit: 4096,
        },
    );
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("terminal I/O failed")
    );
}
