use color_eyre::Result;

use crate::config::editor::migrate_deployment;

pub fn run_migrate(old_name: &str, new_name: &str) -> Result<()> {
    migrate_deployment(old_name, new_name)?;
    eprintln!("Renamed deployment '{old_name}' → '{new_name}'");
    Ok(())
}
