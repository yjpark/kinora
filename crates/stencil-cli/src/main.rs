use std::path::Path;
use std::process::ExitCode;

use cli::{Cli, Command};
use stencil::target::RustTarget;
use fastrace::collector::{Config as FastraceConfig, ConsoleReporter, SpanContext};
use fastrace::Span;
use logforth::append::{FastraceEvent, Stderr};
use logforth::diagnostic::FastraceDiagnostic;
use logforth::filter::env_filter::EnvFilterBuilder;
use logforth::record::LevelFilter;
use rootcause::Report;

mod cli;
mod common;
mod scaffold;
mod sync;

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

    let cwd = match std::env::current_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("cannot determine current directory: {e}");
            return ExitCode::FAILURE;
        }
    };
    let repo_root = match common::resolve_repo_root(
        &cwd,
        cli.repo_root.as_deref().map(Path::new),
    ) {
        Ok(p) => p,
        Err(e) => return report_err("repo-root", e),
    };

    match cli.command {
        Command::Sync { paths } => run_sync_command(&repo_root, &cwd, &paths),
        Command::Scaffold { kinograph } => run_scaffold_command(&repo_root, &kinograph),
    }
}

/// Drive `stencil sync`: render the bound api-kinograph into the read-only
/// blocks of the scanned files, print a summary, and select the exit code.
/// A clean run exits 0; any per-file error or unknown slot exits non-zero.
fn run_sync_command(repo_root: &Path, cwd: &Path, paths: &[String]) -> ExitCode {
    match sync::run_sync(repo_root, cwd, paths, &RustTarget) {
        Ok(summary) => {
            print!("{}", sync::format_sync_summary(&summary));
            if summary.has_errors() {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(e) => report_err("sync", e),
    }
}

/// Drive `stencil scaffold`: generate a fresh source file for an api-kinograph
/// and print it to stdout for the caller to redirect.
fn run_scaffold_command(repo_root: &Path, kinograph: &str) -> ExitCode {
    match scaffold::run_scaffold(repo_root, kinograph, &RustTarget) {
        Ok(source) => {
            print!("{source}");
            ExitCode::SUCCESS
        }
        Err(e) => report_err("scaffold", e),
    }
}

/// Wrap a `CliError` in a `rootcause::Report` with per-command context and
/// print its chained Debug form to stderr.
fn report_err(command: &'static str, e: common::CliError) -> ExitCode {
    let report = Report::new_sendsync(e).context(format!("`stencil {command}` failed"));
    eprintln!("{report}");
    ExitCode::FAILURE
}
