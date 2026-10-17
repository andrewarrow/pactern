use std::ffi::OsString;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand};
use pactern::{Command, Diagnostic, Target};
use serde::Serialize;

#[derive(Parser)]
#[command(
    name = "pactern",
    about = "Pactern toolchain (milestone 2 skeleton)",
    disable_help_flag = true,
    disable_version_flag = true,
    disable_help_subcommand = true,
    color = clap::ColorChoice::Never
)]
struct Cli {
    /// Emit one JSON document on stdout, including errors
    #[arg(long, global = true)]
    json: bool,

    /// Select an exact diagnostic protocol version
    #[arg(long, global = true, default_value = pactern::PROTOCOL_VERSION)]
    protocol_version: String,

    /// Print help
    #[arg(short, long, global = true)]
    help: bool,

    /// Print tool, CLI, language, and protocol versions
    #[arg(short = 'V', long, global = true)]
    version: bool,

    #[command(subcommand)]
    command: Option<CliCommand>,
}

#[derive(Subcommand)]
enum CliCommand {
    /// Print tool, CLI, language, and protocol versions
    Version,
    /// Print help for the CLI or one command
    Help { command: Option<String> },
    /// Check a project (not implemented)
    Check,
    /// Format source (not implemented)
    Fmt {
        #[arg(long)]
        check: bool,
    },
    /// Verify pacts (not implemented)
    Verify,
    /// Run tests (not implemented)
    Test {
        #[arg(long)]
        affected: bool,
    },
    /// Build a WebAssembly component (not implemented)
    Build {
        #[arg(long, default_value = "wasm")]
        target: String,
    },
    /// Inspect a symbol (not implemented)
    Inspect {
        /// Symbol to inspect (required unless displaying help or version)
        symbol: Option<String>,
    },
    /// List a symbol's callers (not implemented)
    Callers {
        /// Symbol to query (required unless displaying help or version)
        symbol: Option<String>,
    },
    /// Inspect the impact of a change request (not implemented)
    Impact {
        /// Change request path (required unless displaying help or version)
        #[arg(long)]
        change: Option<PathBuf>,
    },
    /// Stage or apply a change request (not implemented)
    Apply {
        /// Change request path (required unless displaying help or version)
        change: Option<PathBuf>,
        #[arg(long)]
        dry_run: bool,
    },
}

impl CliCommand {
    fn into_command(self) -> Result<Command, Diagnostic> {
        Ok(match self {
            Self::Version => Command::Version,
            Self::Check => Command::Check,
            Self::Fmt { check } => Command::Fmt { check },
            Self::Verify => Command::Verify,
            Self::Test { affected } => Command::Test { affected },
            Self::Build { target } => {
                if target != "wasm" {
                    return Err(Diagnostic::invalid_arguments(format!(
                        "Unknown target '{target}'; the planned target is 'wasm'."
                    )));
                }
                Command::Build {
                    target: Target::Wasm,
                }
            }
            Self::Inspect { symbol } => Command::Inspect {
                symbol: required(symbol, "SYMBOL")?,
            },
            Self::Callers { symbol } => Command::Callers {
                symbol: required(symbol, "SYMBOL")?,
            },
            Self::Impact { change } => Command::Impact {
                change: required(change, "--change PATH")?,
            },
            Self::Apply { change, dry_run } => Command::Apply {
                change: required(change, "CHANGE")?,
                dry_run,
            },
            Self::Help { .. } => {
                return Err(Diagnostic::invalid_arguments("Help is handled by the CLI."));
            }
        })
    }
}

fn required<T>(value: Option<T>, name: &str) -> Result<T, Diagnostic> {
    value.ok_or_else(|| Diagnostic::invalid_arguments(format!("Missing required argument {name}.")))
}

#[derive(Serialize)]
struct HelpOutput {
    cli_version: &'static str,
    kind: &'static str,
    text: String,
}

fn help(command: Option<&str>) -> Result<HelpOutput, Diagnostic> {
    let mut root = Cli::command();
    root.build();
    let selected = if let Some(name) = command {
        root.find_subcommand_mut(name).ok_or_else(|| {
            Diagnostic::invalid_arguments(format!("Unknown help command '{name}'."))
        })?
    } else {
        &mut root
    };
    Ok(HelpOutput {
        cli_version: pactern::CLI_VERSION,
        kind: "help",
        text: selected.render_long_help().to_string(),
    })
}

fn write_json(value: &impl Serialize) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, value).map_err(io::Error::other)?;
    writeln!(stdout)?;
    stdout.flush()
}

fn report_error(error: &Diagnostic, json: bool) -> io::Result<u8> {
    if json {
        write_json(error)?;
    } else {
        writeln!(
            io::stderr().lock(),
            "error[{}]: {}",
            error.code(),
            error.message()
        )?;
    }
    Ok(match error.code() {
        "PTN_TOOL_INVALID_ARGUMENTS" | "PTN_TOOL_UNSUPPORTED_PROTOCOL" => 2,
        _ => 1,
    })
}

fn run(args: Vec<OsString>) -> io::Result<u8> {
    // Parsing can fail before clap returns the global flag. Respect the argument
    // terminator so a filename spelled --json does not select the output mode.
    let json_requested = args
        .iter()
        .skip(1)
        .take_while(|arg| *arg != "--")
        .any(|arg| arg == "--json");
    let cli = match Cli::try_parse_from(&args) {
        Ok(cli) => cli,
        Err(error) => {
            return report_error(
                &Diagnostic::invalid_arguments(error.to_string()),
                json_requested,
            );
        }
    };
    if let Err(error) = pactern::validate_protocol_version(&cli.protocol_version) {
        return report_error(&error, cli.json);
    }

    let help_command = match &cli.command {
        Some(CliCommand::Help { command }) => Some(command.as_deref()),
        _ if cli.help || (cli.command.is_none() && !cli.version) => {
            // clap retains the selected subcommand name in its parsed matches.
            let matches = Cli::command()
                .try_get_matches_from(&args)
                .map_err(io::Error::other)?;
            let output = help(matches.subcommand_name());
            return print_help(output, cli.json);
        }
        _ => None,
    };
    if let Some(command) = help_command {
        return print_help(help(command), cli.json);
    }

    let command = if cli.version {
        Ok(Command::Version)
    } else {
        cli.command
            .ok_or_else(|| Diagnostic::invalid_arguments("A command is required."))
            .and_then(CliCommand::into_command)
    };
    let result = command.and_then(|command| pactern::execute(&command, &cli.protocol_version));
    match result {
        Ok(version) => {
            if cli.json {
                write_json(&version)?;
            } else {
                let mut stdout = io::stdout().lock();
                writeln!(stdout, "pactern {}", pactern::TOOL_VERSION)?;
                writeln!(stdout, "CLI: {}", pactern::CLI_VERSION)?;
                writeln!(stdout, "Language: {} (planned)", pactern::LANGUAGE_VERSION)?;
                writeln!(stdout, "Protocol: {}", pactern::PROTOCOL_VERSION)?;
            }
            Ok(0)
        }
        Err(error) => report_error(&error, cli.json),
    }
}

fn print_help(output: Result<HelpOutput, Diagnostic>, json: bool) -> io::Result<u8> {
    match output {
        Ok(output) => {
            if json {
                write_json(&output)?;
            } else {
                writeln!(io::stdout().lock(), "{}", output.text)?;
            }
            Ok(0)
        }
        Err(error) => report_error(&error, json),
    }
}

fn main() -> ExitCode {
    match run(std::env::args_os().collect()) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            // A failed output stream cannot reliably carry another JSON message.
            // Best-effort stderr reporting must not panic on a second I/O failure.
            let diagnostic = Diagnostic::io_error(error.to_string());
            let _ = writeln!(
                io::stderr().lock(),
                "error[{}]: {}",
                diagnostic.code(),
                diagnostic.message()
            );
            ExitCode::FAILURE
        }
    }
}
