use crate::decimal::Decimal;
use crate::error::{BotError, Result};
use csv::{ReaderBuilder, StringRecord, Trim};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TradingDate {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

#[derive(Debug, Clone)]
pub struct DailyBar {
    pub date: TradingDate,
    pub date_text: String,
    pub open: Decimal,
    pub high: Decimal,
    pub low: Decimal,
    pub close: Decimal,
    pub volume: Option<Decimal>,
}

pub fn load(path: impl AsRef<Path>) -> Result<Vec<DailyBar>> {
    let path = path.as_ref();
    let mut reader = ReaderBuilder::new()
        .trim(Trim::All)
        .from_path(path)
        .map_err(|error| csv_error(path, error))?;
    let headers = reader
        .headers()
        .map_err(|error| csv_error(path, error))?
        .clone();
    let columns = Columns::from_headers(&headers)?;
    let mut bars: Vec<DailyBar> = Vec::new();

    for (row_index, row) in reader.records().enumerate() {
        let row_number = row_index + 2;
        let row = row.map_err(|error| {
            BotError::MarketData(format!(
                "failed to parse {} row {row_number}: {error}",
                path.to_string_lossy()
            ))
        })?;
        let bar = columns.parse_row(&row, row_number)?;
        if let Some(previous) = bars.last()
            && bar.date <= previous.date
        {
            return Err(BotError::MarketData(format!(
                "CSV dates must be strictly increasing: {} follows {}",
                bar.date_text, previous.date_text
            )));
        }
        bars.push(bar);
    }

    if bars.len() < 2 {
        return Err(BotError::MarketData(
            "equity CSV must contain at least two daily bars".to_string(),
        ));
    }
    Ok(bars)
}

struct Columns {
    date: usize,
    open: usize,
    high: usize,
    low: usize,
    close: usize,
    volume: Option<usize>,
}

impl Columns {
    fn from_headers(headers: &StringRecord) -> Result<Self> {
        let indexes = headers
            .iter()
            .enumerate()
            .map(|(index, header)| (normalize_header(header), index))
            .collect::<HashMap<_, _>>();
        let required = |name: &str| {
            indexes.get(name).copied().ok_or_else(|| {
                BotError::MarketData(format!("equity CSV is missing required '{name}' column"))
            })
        };

        Ok(Self {
            date: required("date")?,
            open: required("open")?,
            high: required("high")?,
            low: required("low")?,
            close: required("close")?,
            volume: indexes.get("volume").copied(),
        })
    }

    fn parse_row(&self, row: &StringRecord, row_number: usize) -> Result<DailyBar> {
        let date_text = field(row, self.date, "date", row_number)?.to_string();
        let date = parse_date(&date_text).map_err(|message| {
            BotError::MarketData(format!("invalid date at CSV row {row_number}: {message}"))
        })?;
        let open = decimal_field(row, self.open, "open", row_number)?;
        let high = decimal_field(row, self.high, "high", row_number)?;
        let low = decimal_field(row, self.low, "low", row_number)?;
        let close = decimal_field(row, self.close, "close", row_number)?;
        let volume = self
            .volume
            .map(|index| decimal_field(row, index, "volume", row_number))
            .transpose()?;

        if [open, high, low, close]
            .into_iter()
            .any(|value| value <= Decimal::ZERO)
        {
            return Err(BotError::MarketData(format!(
                "OHLC prices must be positive at CSV row {row_number}"
            )));
        }
        if high < open || high < close || high < low {
            return Err(BotError::MarketData(format!(
                "high is below another OHLC value at CSV row {row_number}"
            )));
        }
        if low > open || low > close {
            return Err(BotError::MarketData(format!(
                "low is above another OHLC value at CSV row {row_number}"
            )));
        }
        if volume.is_some_and(|value| value < Decimal::ZERO) {
            return Err(BotError::MarketData(format!(
                "volume must not be negative at CSV row {row_number}"
            )));
        }

        Ok(DailyBar {
            date,
            date_text,
            open,
            high,
            low,
            close,
            volume,
        })
    }
}

fn normalize_header(header: &str) -> String {
    header
        .trim_start_matches('\u{feff}')
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn field<'a>(
    row: &'a StringRecord,
    index: usize,
    name: &str,
    row_number: usize,
) -> Result<&'a str> {
    row.get(index)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| BotError::MarketData(format!("missing {name} at CSV row {row_number}")))
}

fn decimal_field(
    row: &StringRecord,
    index: usize,
    name: &str,
    row_number: usize,
) -> Result<Decimal> {
    let value = field(row, index, name, row_number)?;
    Decimal::from_decimal_str(value).map_err(|error| {
        BotError::MarketData(format!(
            "invalid {name} '{value}' at CSV row {row_number}: {error}"
        ))
    })
}

fn parse_date(value: &str) -> std::result::Result<TradingDate, String> {
    let parts = value.split('-').collect::<Vec<_>>();
    if parts.len() != 3 || parts[0].len() != 4 || parts[1].len() != 2 || parts[2].len() != 2 {
        return Err(format!("'{value}' must use YYYY-MM-DD"));
    }
    let year = parts[0]
        .parse::<i32>()
        .map_err(|_| format!("'{value}' has an invalid year"))?;
    let month = parts[1]
        .parse::<u32>()
        .map_err(|_| format!("'{value}' has an invalid month"))?;
    let day = parts[2]
        .parse::<u32>()
        .map_err(|_| format!("'{value}' has an invalid day"))?;
    if year <= 0 || !(1..=12).contains(&month) {
        return Err(format!("'{value}' is not a valid calendar date"));
    }
    let days_in_month = match month {
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 31,
    };
    if day == 0 || day > days_in_month {
        return Err(format!("'{value}' is not a valid calendar date"));
    }
    Ok(TradingDate { year, month, day })
}

fn is_leap_year(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn csv_error(path: &Path, error: csv::Error) -> BotError {
    BotError::MarketData(format!(
        "failed to read equity CSV {}: {error}",
        path.to_string_lossy()
    ))
}

#[cfg(test)]
mod tests {
    use super::load;
    use std::fs;

    fn write_csv(name: &str, contents: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(name);
        fs::write(&path, contents).expect("CSV should write");
        path
    }

    #[test]
    fn loads_case_insensitive_daily_ohlcv_csv() {
        let path = write_csv(
            "trader-valid-equity.csv",
            "Date,Open,High,Low,Close,Volume\n2026-01-02,100,103,99,102,1000\n2026-01-05,102,104,101,103,1200\n",
        );

        let bars = load(&path).expect("CSV should load");

        assert_eq!(bars.len(), 2);
        assert_eq!(bars[0].date_text, "2026-01-02");
        assert_eq!(bars[1].close.to_string(), "103");
        assert_eq!(
            bars[1].volume.expect("volume should exist").to_string(),
            "1200"
        );
        fs::remove_file(path).expect("CSV should remove");
    }

    #[test]
    fn rejects_out_of_order_dates() {
        let path = write_csv(
            "trader-unordered-equity.csv",
            "date,open,high,low,close\n2026-01-05,100,101,99,100\n2026-01-02,100,101,99,100\n",
        );

        let error = load(&path).expect_err("CSV should fail");

        assert!(error.to_string().contains("strictly increasing"));
        fs::remove_file(path).expect("CSV should remove");
    }

    #[test]
    fn rejects_impossible_ohlc_values() {
        let path = write_csv(
            "trader-invalid-ohlc.csv",
            "date,open,high,low,close\n2026-01-02,100,99,98,100\n2026-01-05,100,101,99,100\n",
        );

        let error = load(&path).expect_err("CSV should fail");

        assert!(error.to_string().contains("high is below"));
        fs::remove_file(path).expect("CSV should remove");
    }
}
