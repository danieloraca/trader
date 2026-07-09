mod app;
mod config;
mod error;
mod exchange;
mod market;
mod orders;
mod portfolio;
mod risk;
mod storage;
mod strategy;
mod telemetry;

use crate::error::Result;

fn main() -> Result<()> {
    let config = config::Config::load()?;
    telemetry::init(&config.telemetry);

    let mut app = app::App::new(config)?;
    app.run()
}
