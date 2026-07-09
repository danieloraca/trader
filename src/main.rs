mod app;
mod backtest;
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

use crate::config::RuntimeCommand;
use crate::error::Result;

fn main() -> Result<()> {
    let runtime = config::RuntimeOptions::from_runtime()?;
    let config = config::Config::load_from_path(&runtime.config_path)?;

    if runtime.command == RuntimeCommand::Backtest {
        let report = backtest::run(&config)?;
        println!("{report}");
        return Ok(());
    }

    telemetry::init(&config.telemetry);
    let shutdown = shutdown::Shutdown::install_signal_handlers()?;

    let mut app = app::App::new(config)?;
    app.run(&shutdown)
}
