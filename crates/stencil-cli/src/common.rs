/// CLI-layer error. Wraps [`stencil::StencilError`] and carries CLI-only
/// variants. The binary turns these into `rootcause` reports for display.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error(transparent)]
    Stencil(#[from] stencil::StencilError),

    /// A subcommand whose engine has not landed yet. Scaffolding ships the CLI
    /// surface ahead of the implementing beans (kinora-exay / kinora-guv8).
    #[error("`stencil {command}` is not implemented yet (tracked in beans)")]
    NotImplemented { command: &'static str },
}
