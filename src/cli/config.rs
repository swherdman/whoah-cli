use color_eyre::Result;

use crate::config;
use crate::config::types::derive_expected_zones;

pub async fn show(deployment: Option<&str>) -> Result<()> {
    let deployment_name = config::resolve_deployment(deployment)?;
    let cfg = config::load_deployment(&deployment_name)?;

    println!("Deployment: {deployment_name}");
    println!();

    println!("--- deployment.toml ---");
    println!("{}", toml::to_string_pretty(&cfg.deployment)?);

    println!("--- build.toml ---");
    println!("{}", toml::to_string_pretty(&cfg.build)?);

    println!("--- monitoring.toml ---");
    println!("{}", toml::to_string_pretty(&cfg.monitoring)?);

    // Show derived values
    let expected = derive_expected_zones(&cfg.build.omicron.overrides);
    let total: u32 = expected.values().sum();
    println!("--- derived ---");
    println!("Expected zones ({total} total):");
    let mut services: Vec<_> = expected.iter().collect();
    services.sort_by_key(|(k, _)| (*k).clone());
    for (svc, count) in services {
        println!("  {svc:<18} {count}");
    }

    Ok(())
}
