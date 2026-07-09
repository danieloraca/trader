use std::error::Error;
use std::fmt::{Display, Formatter};

pub type Result<T> = std::result::Result<T, BotError>;

#[derive(Debug)]
pub enum BotError {
    Config(String),
    Exchange(String),
    MarketData(String),
    Risk(String),
    Storage(String),
}

impl Display for BotError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config(message) => write!(f, "configuration error: {message}"),
            Self::Exchange(message) => write!(f, "exchange error: {message}"),
            Self::MarketData(message) => write!(f, "market data error: {message}"),
            Self::Risk(message) => write!(f, "risk error: {message}"),
            Self::Storage(message) => write!(f, "storage error: {message}"),
        }
    }
}

impl Error for BotError {}
