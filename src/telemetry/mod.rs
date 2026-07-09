use crate::config::TelemetryConfig;

pub fn init(config: &TelemetryConfig) {
    if config.verbose {
        println!("telemetry initialized in verbose mode");
    }
}
