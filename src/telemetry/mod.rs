use crate::config::TelemetryConfig;
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{Level, info};

pub fn init(config: &TelemetryConfig) {
    let max_level = if config.verbose {
        Level::DEBUG
    } else {
        Level::INFO
    };

    let _ = tracing_subscriber::fmt()
        .with_max_level(max_level)
        .with_target(false)
        .compact()
        .try_init();

    info!(verbose = config.verbose, "telemetry initialized");
}

pub fn new_run_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);

    format!("run-{millis}-{}", process::id())
}
