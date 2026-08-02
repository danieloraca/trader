mod app;
mod backtest;
mod candles;
mod config;
mod decimal;
mod equity;
mod error;
mod exchange;
mod market;
mod orders;
mod portfolio;
mod risk;
mod shutdown;
mod storage;
mod strategy;
mod sweep;
mod telemetry;

use crate::config::RuntimeCommand;
use crate::error::Result;

fn main() -> Result<()> {
    let runtime = config::RuntimeOptions::from_runtime()?;

    if runtime.command == RuntimeCommand::BacktestEquity {
        let price_path = runtime.backtest_equity_path.as_deref().ok_or_else(|| {
            error::BotError::Config("--backtest-equity requires a daily price file".to_string())
        })?;
        let report = equity::run(&runtime.config_path, price_path)?;
        println!("{report}");
        return Ok(());
    }
    if runtime.command == RuntimeCommand::WalkForwardEquity {
        let price_path = runtime.walk_forward_equity_path.as_deref().ok_or_else(|| {
            error::BotError::Config("--walk-forward-equity requires a daily price file".to_string())
        })?;
        let report = equity::run_walk_forward(&runtime.config_path, price_path)?;
        println!("{report}");
        return Ok(());
    }
    if runtime.command == RuntimeCommand::BacktestEquityPortfolio {
        let report = equity::run_portfolio(&runtime.config_path)?;
        println!("{report}");
        return Ok(());
    }

    let config = config::Config::load_from_path(&runtime.config_path)?;

    match runtime.command {
        RuntimeCommand::Backtest => {
            let report = backtest::run(&config)?;
            println!("{report}");
            return Ok(());
        }
        RuntimeCommand::BacktestSqlite => {
            let sqlite_path = runtime.backtest_sqlite_path.as_deref().ok_or_else(|| {
                error::BotError::Config("--backtest-sqlite requires a sqlite path".to_string())
            })?;
            let report = backtest::run_from_sqlite(&config, sqlite_path)?;
            println!("{report}");
            return Ok(());
        }
        RuntimeCommand::BacktestEquity => unreachable!("handled before daemon config loading"),
        RuntimeCommand::BacktestEquityPortfolio => {
            unreachable!("handled before daemon config loading")
        }
        RuntimeCommand::WalkForwardEquity => {
            unreachable!("handled before daemon config loading")
        }
        RuntimeCommand::SweepSqlite => {
            let sqlite_path = runtime.sweep_sqlite_path.as_deref().ok_or_else(|| {
                error::BotError::Config("--sweep-sqlite requires a sqlite path".to_string())
            })?;
            let report = sweep::run(&config, sqlite_path)?;
            println!("{report}");
            return Ok(());
        }
        RuntimeCommand::SweepCandlesSqlite => {
            let sqlite_path = runtime
                .sweep_candles_sqlite_path
                .as_deref()
                .ok_or_else(|| {
                    error::BotError::Config(
                        "--sweep-candles-sqlite requires a sqlite path".to_string(),
                    )
                })?;
            let report = sweep::run_candles(&config, sqlite_path)?;
            println!("{report}");
            return Ok(());
        }
        RuntimeCommand::WalkForwardSqlite => {
            let sqlite_path = runtime.walk_forward_sqlite_path.as_deref().ok_or_else(|| {
                error::BotError::Config("--walk-forward-sqlite requires a sqlite path".to_string())
            })?;
            let report = sweep::run_walk_forward(&config, sqlite_path)?;
            println!("{report}");
            return Ok(());
        }
        RuntimeCommand::Run => {}
    }

    telemetry::init(&config.telemetry);
    let shutdown = shutdown::Shutdown::install_signal_handlers()?;

    let mut app = app::App::new(config)?;
    app.run(&shutdown)
}
