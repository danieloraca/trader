use crate::decimal::Decimal;
use crate::error::{BotError, Result};
use calamine::{Data, DataType, Reader, open_workbook_auto};
use csv::{ReaderBuilder, Trim};
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DailyPriceKind {
    Ohlcv,
    CloseOnly,
}

impl Display for DailyPriceKind {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ohlcv => formatter.write_str("daily OHLCV"),
            Self::CloseOnly => formatter.write_str("daily close-only"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputFormat {
    Csv,
    Excel,
}

impl Display for InputFormat {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Csv => formatter.write_str("CSV"),
            Self::Excel => formatter.write_str("Excel"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DailyPriceData {
    pub bars: Vec<DailyBar>,
    pub kind: DailyPriceKind,
    pub input_format: InputFormat,
    pub price_column: String,
}

pub fn load(path: impl AsRef<Path>) -> Result<DailyPriceData> {
    let path = path.as_ref();
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("csv") => load_csv(path),
        Some("xlsx") | Some("xls") | Some("xlsm") | Some("xlsb") => load_excel(path),
        _ => Err(BotError::MarketData(format!(
            "unsupported equity price file {}; expected CSV or Excel",
            path.to_string_lossy()
        ))),
    }
}

fn load_csv(path: &Path) -> Result<DailyPriceData> {
    let mut reader = ReaderBuilder::new()
        .trim(Trim::All)
        .from_path(path)
        .map_err(|error| csv_error(path, error))?;
    let headers = reader
        .headers()
        .map_err(|error| csv_error(path, error))?
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let columns = Columns::from_headers(&headers).ok_or_else(missing_columns_error)?;
    let rows = reader
        .records()
        .enumerate()
        .map(|(index, record)| {
            record
                .map(|record| record.iter().map(ToString::to_string).collect::<Vec<_>>())
                .map(|row| (index + 2, row))
                .map_err(|error| csv_error(path, error))
        })
        .collect::<Result<Vec<_>>>()?;
    parse_table(columns, rows, InputFormat::Csv)
}

fn load_excel(path: &Path) -> Result<DailyPriceData> {
    let mut workbook = open_workbook_auto(path).map_err(|error| {
        BotError::MarketData(format!(
            "failed to open equity spreadsheet {}: {error}",
            path.to_string_lossy()
        ))
    })?;
    let sheet_names = workbook.sheet_names().to_vec();

    for sheet_name in sheet_names {
        let range = workbook.worksheet_range(&sheet_name).map_err(|error| {
            BotError::MarketData(format!(
                "failed to read spreadsheet sheet '{sheet_name}': {error}"
            ))
        })?;
        let rows = range
            .rows()
            .map(|row| row.iter().map(excel_cell_text).collect::<Vec<_>>())
            .collect::<Vec<_>>();

        if let Some((header_index, columns)) = rows
            .iter()
            .enumerate()
            .find_map(|(index, row)| Columns::from_headers(row).map(|columns| (index, columns)))
        {
            let data_rows = rows
                .into_iter()
                .enumerate()
                .skip(header_index + 1)
                .map(|(index, row)| (index + 1, row))
                .collect::<Vec<_>>();
            return parse_table(columns, data_rows, InputFormat::Excel);
        }
    }

    Err(missing_columns_error())
}

fn excel_cell_text(cell: &Data) -> String {
    if matches!(cell, Data::DateTime(_) | Data::DateTimeIso(_))
        && let Some(date_time) = cell.as_datetime()
    {
        return date_time.date().to_string();
    }
    match cell {
        Data::Empty => String::new(),
        Data::Int(value) => value.to_string(),
        Data::Float(value) => value.to_string(),
        Data::String(value) | Data::DateTimeIso(value) | Data::DurationIso(value) => value.clone(),
        Data::Bool(value) => value.to_string(),
        Data::DateTime(value) => value.as_f64().to_string(),
        Data::Error(value) => format!("{value:?}"),
    }
}

fn parse_table(
    columns: Columns,
    rows: Vec<(usize, Vec<String>)>,
    input_format: InputFormat,
) -> Result<DailyPriceData> {
    let mut bars = Vec::new();
    for (row_number, row) in rows {
        if row.iter().all(|value| value.trim().is_empty()) {
            continue;
        }
        if let Some(bar) = columns.parse_row(&row, row_number)? {
            bars.push(bar);
        }
    }

    if bars.len() < 2 {
        return Err(BotError::MarketData(
            "equity price file must contain at least two daily prices".to_string(),
        ));
    }
    normalize_date_order(&mut bars)?;

    Ok(DailyPriceData {
        bars,
        kind: columns.kind,
        input_format,
        price_column: columns.price_column,
    })
}

fn normalize_date_order(bars: &mut [DailyBar]) -> Result<()> {
    let increasing = bars
        .windows(2)
        .all(|window| window[0].date < window[1].date);
    if increasing {
        return Ok(());
    }
    let decreasing = bars
        .windows(2)
        .all(|window| window[0].date > window[1].date);
    if decreasing {
        bars.reverse();
        return Ok(());
    }
    Err(BotError::MarketData(
        "daily price dates must be unique and consistently ascending or descending".to_string(),
    ))
}

#[derive(Debug)]
struct Columns {
    date: usize,
    open: Option<usize>,
    high: Option<usize>,
    low: Option<usize>,
    close: usize,
    volume: Option<usize>,
    kind: DailyPriceKind,
    price_column: String,
}

impl Columns {
    fn from_headers(headers: &[String]) -> Option<Self> {
        let normalized = headers
            .iter()
            .map(|header| normalize_header(header))
            .collect::<Vec<_>>();
        let indexes = normalized
            .iter()
            .enumerate()
            .map(|(index, header)| (header.as_str(), index))
            .collect::<HashMap<_, _>>();
        let date = indexes.get("date").copied()?;
        let open = indexes.get("open").copied();
        let high = indexes.get("high").copied();
        let low = indexes.get("low").copied();
        let volume = indexes.get("volume").copied();
        let close = indexes
            .get("close")
            .copied()
            .or_else(|| find_price_column(&normalized, "marketprice"))
            .or_else(|| find_price_column(&normalized, "marketvalue"))
            .or_else(|| indexes.get("price").copied())
            .or_else(|| find_price_column(&normalized, "navprice"))
            .or_else(|| find_price_column(&normalized, "nav"))?;
        let kind = if open.is_some() && high.is_some() && low.is_some() {
            DailyPriceKind::Ohlcv
        } else {
            DailyPriceKind::CloseOnly
        };

        Some(Self {
            date,
            open,
            high,
            low,
            close,
            volume,
            kind,
            price_column: headers[close].trim().to_string(),
        })
    }

    fn parse_row(&self, row: &[String], row_number: usize) -> Result<Option<DailyBar>> {
        let date_value = field(row, self.date).unwrap_or_default().trim();
        if date_value.is_empty() {
            return Ok(None);
        }
        let date = parse_date(date_value).map_err(|message| {
            BotError::MarketData(format!("invalid date at price row {row_number}: {message}"))
        })?;
        let close = decimal_field(row, self.close, &self.price_column, row_number)?;
        let (open, high, low) = if self.kind == DailyPriceKind::Ohlcv {
            (
                decimal_field(
                    row,
                    self.open.expect("OHLC open should exist"),
                    "open",
                    row_number,
                )?,
                decimal_field(
                    row,
                    self.high.expect("OHLC high should exist"),
                    "high",
                    row_number,
                )?,
                decimal_field(
                    row,
                    self.low.expect("OHLC low should exist"),
                    "low",
                    row_number,
                )?,
            )
        } else {
            (close, close, close)
        };
        let volume = self
            .volume
            .and_then(|index| field(row, index))
            .filter(|value| !value.trim().is_empty())
            .map(|value| parse_decimal(value, "volume", row_number))
            .transpose()?;

        if [open, high, low, close]
            .into_iter()
            .any(|value| value <= Decimal::ZERO)
        {
            return Err(BotError::MarketData(format!(
                "prices must be positive at price row {row_number}"
            )));
        }
        if high < open || high < close || high < low {
            return Err(BotError::MarketData(format!(
                "high is below another OHLC value at price row {row_number}"
            )));
        }
        if low > open || low > close {
            return Err(BotError::MarketData(format!(
                "low is above another OHLC value at price row {row_number}"
            )));
        }
        if volume.is_some_and(|value| value < Decimal::ZERO) {
            return Err(BotError::MarketData(format!(
                "volume must not be negative at price row {row_number}"
            )));
        }

        Ok(Some(DailyBar {
            date,
            date_text: date.to_string(),
            open,
            high,
            low,
            close,
            volume,
        }))
    }
}

fn find_price_column(headers: &[String], prefix: &str) -> Option<usize> {
    headers.iter().position(|header| header.starts_with(prefix))
}

fn normalize_header(header: &str) -> String {
    header
        .trim_start_matches('\u{feff}')
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn field(row: &[String], index: usize) -> Option<&str> {
    row.get(index).map(String::as_str)
}

fn decimal_field(row: &[String], index: usize, name: &str, row_number: usize) -> Result<Decimal> {
    let value = field(row, index)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| BotError::MarketData(format!("missing {name} at price row {row_number}")))?;
    parse_decimal(value, name, row_number)
}

fn parse_decimal(value: &str, name: &str, row_number: usize) -> Result<Decimal> {
    let negative_parentheses = value.trim().starts_with('(') && value.trim().ends_with(')');
    let mut cleaned = value
        .chars()
        .filter(|character| character.is_ascii_digit() || matches!(character, '.' | '-' | '+'))
        .collect::<String>();
    if negative_parentheses && !cleaned.starts_with('-') {
        cleaned.insert(0, '-');
    }
    Decimal::from_decimal_str(&cleaned).map_err(|error| {
        BotError::MarketData(format!(
            "invalid {name} '{value}' at price row {row_number}: {error}"
        ))
    })
}

fn parse_date(value: &str) -> std::result::Result<TradingDate, String> {
    let value = value.trim();
    if let Some(date) = parse_numeric_date(value, '-') {
        return validate_date(date, value);
    }
    if let Some(date) = parse_numeric_date(value, '/') {
        return validate_date(date, value);
    }

    let parts = value.split_whitespace().collect::<Vec<_>>();
    if parts.len() == 3 {
        let day = parts[0].parse::<u32>().ok();
        let month = month_number(parts[1]);
        let year = parts[2].parse::<i32>().ok();
        if let (Some(year), Some(month), Some(day)) = (year, month, day) {
            return validate_date(TradingDate { year, month, day }, value);
        }
    }
    Err(format!(
        "'{value}' must use YYYY-MM-DD, DD/MM/YYYY, or DD Mon YYYY"
    ))
}

fn parse_numeric_date(value: &str, separator: char) -> Option<TradingDate> {
    let parts = value.split(separator).collect::<Vec<_>>();
    if parts.len() != 3 {
        return None;
    }
    if parts[0].len() == 4 {
        Some(TradingDate {
            year: parts[0].parse().ok()?,
            month: parts[1].parse().ok()?,
            day: parts[2].parse().ok()?,
        })
    } else {
        Some(TradingDate {
            year: parts[2].parse().ok()?,
            month: parts[1].parse().ok()?,
            day: parts[0].parse().ok()?,
        })
    }
}

fn month_number(value: &str) -> Option<u32> {
    match value.to_ascii_lowercase().as_str() {
        "jan" | "january" => Some(1),
        "feb" | "february" => Some(2),
        "mar" | "march" => Some(3),
        "apr" | "april" => Some(4),
        "may" => Some(5),
        "jun" | "june" => Some(6),
        "jul" | "july" => Some(7),
        "aug" | "august" => Some(8),
        "sep" | "sept" | "september" => Some(9),
        "oct" | "october" => Some(10),
        "nov" | "november" => Some(11),
        "dec" | "december" => Some(12),
        _ => None,
    }
}

fn validate_date(date: TradingDate, original: &str) -> std::result::Result<TradingDate, String> {
    if date.year <= 0 || !(1..=12).contains(&date.month) {
        return Err(format!("'{original}' is not a valid calendar date"));
    }
    let days_in_month = match date.month {
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(date.year) => 29,
        2 => 28,
        _ => 31,
    };
    if date.day == 0 || date.day > days_in_month {
        return Err(format!("'{original}' is not a valid calendar date"));
    }
    Ok(date)
}

fn is_leap_year(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

impl Display for TradingDate {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{:04}-{:02}-{:02}",
            self.year, self.month, self.day
        )
    }
}

fn missing_columns_error() -> BotError {
    BotError::MarketData(
        "price file needs a date column plus either OHLC columns or a close/market price column"
            .to_string(),
    )
}

fn csv_error(path: &Path, error: csv::Error) -> BotError {
    BotError::MarketData(format!(
        "failed to read equity CSV {}: {error}",
        path.to_string_lossy()
    ))
}

#[cfg(test)]
mod tests {
    use super::{DailyPriceKind, InputFormat, load};
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

        let data = load(&path).expect("CSV should load");

        assert_eq!(data.kind, DailyPriceKind::Ohlcv);
        assert_eq!(data.input_format, InputFormat::Csv);
        assert_eq!(data.bars.len(), 2);
        assert_eq!(data.bars[1].close.to_string(), "103");
        assert_eq!(
            data.bars[1]
                .volume
                .expect("volume should exist")
                .to_string(),
            "1200"
        );
        fs::remove_file(path).expect("CSV should remove");
    }

    #[test]
    fn loads_vanguard_style_close_prices_and_reverses_descending_dates() {
        let path = write_csv(
            "trader-vanguard-equity.csv",
            "Date,NAV (USD),Market price (GBP)\n31 Jul 2026,US$188.7980,£139.1600\n30 Jul 2026,US$186.5467,£138.7800\n29 Jul 2026,US$183.8182,£139.0600\n",
        );

        let data = load(&path).expect("close-only CSV should load");

        assert_eq!(data.kind, DailyPriceKind::CloseOnly);
        assert_eq!(data.price_column, "Market price (GBP)");
        assert_eq!(data.bars[0].date_text, "2026-07-29");
        assert_eq!(data.bars[0].close.to_string(), "139.06");
        assert_eq!(data.bars[2].close.to_string(), "139.16");
        fs::remove_file(path).expect("CSV should remove");
    }

    #[test]
    fn rejects_mixed_or_duplicate_date_order() {
        let path = write_csv(
            "trader-unordered-equity.csv",
            "date,close\n2026-01-05,100\n2026-01-02,101\n2026-01-06,102\n",
        );

        let error = load(&path).expect_err("CSV should fail");

        assert!(
            error
                .to_string()
                .contains("consistently ascending or descending")
        );
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
