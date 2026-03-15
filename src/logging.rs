use std::fs;
use std::path::PathBuf;

use color_eyre::Result;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

fn log_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".whoah").join("logs")
}

pub fn init(verbose: u8) -> Result<()> {
    let log_path = log_dir();
    fs::create_dir_all(&log_path)?;

    let file_appender = tracing_appender::rolling::daily(&log_path, "whoah.log");

    let env_filter = EnvFilter::try_from_env("WHOAH_LOG").unwrap_or_else(|_| {
        match verbose {
            0 => EnvFilter::new("whoah_cli=info"),
            1 => EnvFilter::new("whoah_cli=debug"),
            _ => EnvFilter::new("whoah_cli=trace"),
        }
    });

    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(file_appender)
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(false);

    // Initialize tui-logger so it captures tracing events for the TUI log panel
    tui_logger::init_logger(log::LevelFilter::Debug)?;
    let tui_layer = tui_logger::tracing_subscriber_layer();

    tracing_subscriber::registry()
        .with(env_filter)
        .with(file_layer)
        .with(tui_layer)
        .init();

    tracing::debug!("Logging initialized (verbosity level: {verbose})");

    Ok(())
}
