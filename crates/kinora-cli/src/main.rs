use std::io::Write;
use std::process::ExitCode;

use cli::{Cli, Command};
use resolve::{
    head_lineages, render_all_heads, render_fork_report, run_resolve, ResolveOutcome,
    ResolveRunArgs,
};
use store::{run_store, StoreRunArgs};

mod cli;
mod common;
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
            };
            match run_store(&cwd, args) {
                Ok(stored) => {
                    println!(
                        "stored kind={} id={} hash={} lineage={}{}",
                        stored.event.kind,
                        stored.event.id,
                        stored.event.hash,
                        stored.lineage,
                        if stored.was_new_lineage { " (new lineage)" } else { "" },
                    );
                    ExitCode::SUCCESS
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
