use std::io::Write;
use std::process::ExitCode;

use assign::{format_assign_summary, run_assign, AssignRunArgs};
use cli::{Cli, Command};
use compact::{run_compact, CompactRunArgs};
use render::{run_render, RenderRunArgs};
use resolve::{
    head_lineages, render_all_heads, render_fork_report, run_resolve, ResolveOutcome,
    ResolveRunArgs,
};
use store::{format_store_summary, run_store, StoreRunArgs};

mod assign;
mod cli;
mod common;
mod compact;
mod render;
mod resolve;
mod store;

fn main() -> ExitCode {
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
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Command::Assign { kino, root, resolves, author, provenance } => {
            let args = AssignRunArgs { kino, root, resolves, author, provenance };
            match run_assign(&cwd, args) {
                Ok(result) => {
                    println!("{}", format_assign_summary(&result));
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
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
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Command::Compact { author, provenance } => {
            let args = CompactRunArgs { author, provenance };
            match run_compact(&cwd, args) {
                Ok(report) => {
                    for (name, result) in &report.per_root {
                        match result {
                            Ok(r) => match &r.new_version {
                                Some(h) => println!(
                                    "root={} version={} (new version)",
                                    name,
                                    h.shorthash(),
                                ),
                                None => {
                                    let version_str = r
                                        .prior_version
                                        .as_ref()
                                        .map(|h| h.shorthash().to_owned())
                                        .unwrap_or_else(|| "-".into());
                                    println!(
                                        "root={} version={} (no-op)",
                                        name, version_str,
                                    );
                                }
                            },
                            Err(e) => {
                                println!("root={} ERROR: {}", name, e);
                            }
                        }
                    }
                    if report.any_error() {
                        ExitCode::FAILURE
                    } else {
                        ExitCode::SUCCESS
                    }
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
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
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}

fn kinora_resolver_from_cwd(cwd: &std::path::Path) -> Result<kinora::resolve::Resolver, common::CliError> {
    let repo_root = common::find_repo_root(cwd)?;
    let kin_root = kinora::paths::kinora_root(&repo_root);
    Ok(kinora::resolve::Resolver::load(&kin_root)?)
}
