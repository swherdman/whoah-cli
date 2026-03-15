use clap::Parser;
use color_eyre::Result;

mod action;
mod app;
mod cli;
mod config;
mod event;
mod logging;
mod ops;
mod parse;
mod ssh;
mod tui;

use cli::{Cli, Command, ConfigCommand};

#[tokio::main]
async fn main() -> Result<()> {
    human_panic::setup_panic!();
    color_eyre::install()?;

    let cli = Cli::parse();
    logging::init(cli.verbose)?;

    let deployment = cli.deployment.as_deref();

    match cli.command {
        Some(Command::Status) => {
            cli::status::run(deployment).await?;
        }
        Some(Command::Recover) => {
            cli::recover::run(deployment).await?;
        }
        Some(Command::Init(args)) => {
            cli::init::run(args).await?;
        }
        Some(Command::Config(args)) => match args.command {
            ConfigCommand::Show => {
                cli::config::show(deployment).await?;
            }
        },
        None => {
            // Launch TUI dashboard
            let deployment_name = config::resolve_deployment(deployment)?;
            let cfg = config::load_deployment(&deployment_name)?;
            let mut application = app::App::new(cfg, deployment_name);
            application.run().await?;
        }
    }

    Ok(())
}
