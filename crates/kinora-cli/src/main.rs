use std::process::ExitCode;

use cli::{Cli, Command};
use store::{run_store, StoreRunArgs};

mod cli;
mod common;
mod store;

fn main() -> ExitCode {
    let cli: Cli = match figue::from_std_args::<Cli>().into_result() {
        Ok(output) => output.get(),
        Err(e) => {
            eprintln!("{e}");
            return if e.is_help() { ExitCode::SUCCESS } else { ExitCode::FAILURE };
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
    }
}
