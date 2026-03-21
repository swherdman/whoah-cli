pub mod config;
pub mod init;
pub mod migrate;
pub mod recover;
pub mod status;

use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(name = "whoah", about = "Manage Oxide-at-home deployments")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Select deployment by name
    #[arg(long, global = true)]
    pub deployment: Option<String>,

    /// Increase verbosity (-v, -vv, -vvv)
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    /// Run in demo mode (simulated build pipeline, no SSH)
    #[arg(long, global = true)]
    pub demo: bool,
}

#[derive(Subcommand)]
pub enum Command {
    /// Quick status snapshot (headless)
    Status,
    /// Reboot recovery (headless, stdout progress)
    Recover,
    /// Initialize a new deployment
    Init(InitArgs),
    /// Configuration management
    Config(ConfigArgs),
    /// Rename a deployment
    Migrate(MigrateArgs),
}

#[derive(Args)]
pub struct InitArgs {
    /// Import config from a running Helios host (e.g., user@192.168.2.209)
    #[arg(long)]
    pub import: Option<String>,
}

#[derive(Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommand,
}

#[derive(Args)]
pub struct MigrateArgs {
    /// Current deployment name
    pub old_name: String,
    /// New deployment name
    pub new_name: String,
}

#[derive(Subcommand)]
pub enum ConfigCommand {
    /// Display current deployment config
    Show,
}
