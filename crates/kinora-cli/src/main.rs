use std::io::Write;
use std::process::ExitCode;

use assign::{format_assign_summary, run_assign, AssignRunArgs};
use cli::{Cli, Command};
use commit::{render_commit_entry, run_commit, CommitRunArgs};
use fastrace::collector::{Config as FastraceConfig, ConsoleReporter, SpanContext};
use fastrace::Span;
use logforth::append::{FastraceEvent, Stderr};
use logforth::diagnostic::FastraceDiagnostic;
use logforth::filter::env_filter::EnvFilterBuilder;
use logforth::record::LevelFilter;
use render::{run_render, RenderRunArgs};
use resolve::{
    head_lineages, render_all_heads, render_fork_report, run_resolve, ResolveOutcome,
    ResolveRunArgs,
};
use rootcause::Report;
use store::{format_store_summary, run_store, StoreRunArgs};

mod assign;
mod cli;
mod common;
mod commit;
mod render;
mod resolve;
mod store;

fn main() -> ExitCode {
    init_logging();
    let code = {
        let root = Span::root("kinora.main", SpanContext::random());
        let _g = root.set_local_parent();
        run()
    };
    fastrace::flush();
    code
}

/// Wire logforth + fastrace.
///
/// Two dispatches:
///   1. `FastraceEvent` — bridges `log::*` events into the active fastrace
///      span so they appear inline on traces when a reporter is attached.
///   2. `Stderr` with `FastraceDiagnostic` — stamps trace/span ids on
///      stderr output. `RUST_LOG` gates level via the default `EnvFilter`.
///
/// `KINORA_TRACE=1` installs a `ConsoleReporter` so fastrace dumps every
/// root span to stderr on `flush()`. Off by default — tracing is opt-in.
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

    if std::env::var("KINORA_TRACE").is_ok_and(|v| v == "1") {
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

    match cli.command {
        Command::Store {
            kind,
            path,
            provenance,
            name,
            id,
            parents,
            draft,
            author,
            metadata,
            root,
        } => {
            let args = StoreRunArgs {
                kind,
                path,
                provenance,
                name,
                id,
                parents,
                draft,
                author,
                metadata,
                root,
            };
            match run_store(&cwd, args) {
                Ok(stored) => {
                    println!("{}", format_store_summary(&stored));
                    ExitCode::SUCCESS
                }
                Err(e) => report_err("store", e),
            }
        }
        Command::Assign { kino, root, resolves, author, provenance } => {
            let args = AssignRunArgs { kino, root, resolves, author, provenance };
            match run_assign(&cwd, args) {
                Ok(result) => {
                    println!("{}", format_assign_summary(&result));
                    ExitCode::SUCCESS
                }
                Err(e) => report_err("assign", e),
            }
        }
        Command::Render { cache_dir } => {
            let args = RenderRunArgs { cache_dir };
            match run_render(&cwd, args) {
                Ok(report) => {
                    let skipped_note = if report.skipped_count == 0 {
                        String::new()
                    } else {
                        format!(" (skipped {} forked)", report.skipped_count)
                    };
                    println!(
                        "rendered {} pages{} into {}",
                        report.page_count,
                        skipped_note,
                        report.cache_path.display(),
                    );
                    ExitCode::SUCCESS
                }
                Err(e) => report_err("render", e),
            }
        }
        Command::Commit { author, provenance } => {
            let args = CommitRunArgs { author, provenance };
            match run_commit(&cwd, args) {
                Ok(report) => {
                    let mut stdout = std::io::stdout().lock();
                    for entry in &report.per_root {
                        let _ = render_commit_entry(&mut stdout, entry);
                    }
                    if report.any_error() {
                        ExitCode::FAILURE
                    } else {
                        ExitCode::SUCCESS
                    }
                }
                Err(e) => report_err("commit", e),
            }
        }
        Command::Resolve { name_or_id, version, all_heads } => {
            let args = ResolveRunArgs { name_or_id: name_or_id.clone(), version, all_heads };
            match run_resolve(&cwd, args) {
                Ok(ResolveOutcome::Content(resolved)) => {
                    let mut stdout = std::io::stdout().lock();
                    if let Err(e) = stdout.write_all(&resolved.content) {
                        eprintln!("error: {e}");
                        return ExitCode::FAILURE;
                    }
                    ExitCode::SUCCESS
                }
                Ok(ResolveOutcome::AllHeads { id, heads }) => {
                    let lineages = match kinora_resolver_from_cwd(&cwd) {
                        Ok(r) => head_lineages(&r, &id, &heads),
                        Err(_) => heads.iter().map(|_| "?".to_owned()).collect(),
                    };
                    let mut stdout = std::io::stdout().lock();
                    let _ = render_all_heads(&mut stdout, &id, &heads, &lineages);
                    ExitCode::SUCCESS
                }
                Err(common::CliError::Resolve(kinora::resolve::ResolveError::MultipleHeads {
                    id,
                    heads,
                    lineages,
                })) => {
                    let mut stderr = std::io::stderr().lock();
                    let _ = render_fork_report(&mut stderr, &name_or_id, &id, &heads, &lineages);
                    ExitCode::FAILURE
                }
                Err(e) => report_err("resolve", e),
            }
        }
    }
}

fn kinora_resolver_from_cwd(cwd: &std::path::Path) -> Result<kinora::resolve::Resolver, common::CliError> {
    let repo_root = common::find_repo_root(cwd)?;
    let kin_root = kinora::paths::kinora_root(&repo_root);
    Ok(kinora::resolve::Resolver::load(&kin_root)?)
}

/// Wrap a `CliError` in a `rootcause::Report` with per-command context and
/// print its chained Debug form to stderr. The child chain (thiserror
/// `source()` links) appears under the top-level context string.
fn report_err(command: &'static str, e: common::CliError) -> ExitCode {
    let report = Report::new_sendsync(e).context(format!("`kinora {command}` failed"));
    eprintln!("{report}");
    ExitCode::FAILURE
}
