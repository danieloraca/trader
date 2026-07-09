mod app;
mod config;
mod decimal;
mod error;
mod exchange;
mod market;
mod orders;
mod portfolio;
mod risk;
mod shutdown;
mod storage;
mod strategy;
mod telemetry;

use crate::error::Result;

fn main() -> Result<()> {
    let config = config::Config::load_from_runtime()?;
    telemetry::init(&config.telemetry);
    let shutdown = shutdown::Shutdown::install_signal_handlers()?;

    let mut app = app::App::new(config)?;
    app.run(&shutdown)
}
