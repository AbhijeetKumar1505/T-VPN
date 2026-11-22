//! Command implementations.

mod cloud;
mod doctor;
mod down;
mod profile;
mod server;
pub mod session;
mod status;
mod up;

use crate::cli::{Cli, Command};
use anyhow::Result;
use clap::CommandFactory;

/// Run a parsed command line; returns the process exit code.
pub async fn run(cli: Cli) -> Result<i32> {
    match cli.command {
        Command::Up(a) => up::run(a).await,
        Command::Down(a) => down::stop(crate::control::Mode::Client, !a.no_sudo).await,
        Command::Status(a) => status::run(a).await,
        Command::Import(a) => profile::import(a),
        Command::List => profile::list(),
        Command::Show(a) => profile::show(a),
        Command::Remove(a) => profile::remove(a),
        Command::Genkey => profile::genkey(),
        Command::Pubkey => profile::pubkey(),
        Command::Genpsk => profile::genpsk(),
        Command::Keypair => profile::keypair(),
        Command::Cloud(c) => cloud::run(c).await,
        Command::Server(s) => server::run(s).await,
        Command::Doctor => doctor::run().await,
        Command::Completions(a) => {
            clap_complete::generate(a.shell, &mut Cli::command(), "tvpn", &mut std::io::stdout());
            Ok(0)
        }
    }
}
