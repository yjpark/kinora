use std::process::ExitCode;

use cli::{Cli, Command};
use fastrace::collector::{Config as FastraceConfig, ConsoleReporter, SpanContext};
use fastrace::Span;
use logforth::append::{FastraceEvent, Stderr};
use logforth::diagnostic::FastraceDiagnostic;
use logforth::filter::env_filter::EnvFilterBuilder;
use logforth::record::LevelFilter;
use rootcause::Report;

mod cli;
mod common;

fn main() -> ExitCode {
    init_logging();
    let code = {
        let root = Span::root("stencil.main", SpanContext::random());
        let _g = root.set_local_parent();
        run()
    };
    fastrace::flush();
    code
}

/// Wire logforth + fastrace, mirroring kinora-cli: a `FastraceEvent` dispatch
/// bridges `log::*` into the active span, and a `Stderr` dispatch (gated by
/// `RUST_LOG`, default `info`) stamps trace/span ids on output. `STENCIL_TRACE=1`
/// installs a `ConsoleReporter` so spans dump to stderr on `flush()`.
fn init_logging() {
    logforth::starter_log::builder()
        .dispatch(|d| {
            d.filter(LevelFilter::All)
                .append(FastraceEvent::default())
        })
        .dispatch(|d| {
            d.filter(EnvFilterBuilder::from_default_env_or("info").build())
                .diagnostic(FastraceDiagnostic::default())
                .append(Stderr::default())
        })
        .apply();

    if std::env::var("STENCIL_TRACE").is_ok_and(|v| v == "1") {
        fastrace::set_reporter(ConsoleReporter, FastraceConfig::default());
    }
}

fn run() -> ExitCode {
    let cli: Cli = match figue::from_std_args::<Cli>().into_result() {
        Ok(output) => output.get(),
        Err(e) => {
            eprintln!("{e}");
            // DriverError classifies help, --version, and --completions as
            // success exits (exit_code == 0).
            return if e.is_success() { ExitCode::SUCCESS } else { ExitCode::FAILURE };
        }
    };

    // The command surface is in place; the engines land in later beans. Each
    // arm dispatches to its `run_*` once implemented.
    match cli.command {
        Command::Sync { .. } => report_err("sync", common::CliError::NotImplemented { command: "sync" }),
        Command::Scaffold { .. } => {
            report_err("scaffold", common::CliError::NotImplemented { command: "scaffold" })
        }
    }
}

/// Wrap a `CliError` in a `rootcause::Report` with per-command context and
/// print its chained Debug form to stderr.
fn report_err(command: &'static str, e: common::CliError) -> ExitCode {
    let report = Report::new_sendsync(e).context(format!("`stencil {command}` failed"));
    eprintln!("{report}");
    ExitCode::FAILURE
}
