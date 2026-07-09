use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer};
use std::fmt::{Display, Formatter};
use std::ops::{Add, AddAssign, Div, Mul, Sub, SubAssign};

const SCALE: i64 = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Decimal {
    micro_units: i64,
}

impl Decimal {
    pub const ZERO: Self = Self { micro_units: 0 };

    pub fn from_micro_units(micro_units: i64) -> Self {
        Self { micro_units }
    }

    pub fn micro_units(self) -> i64 {
        self.micro_units
    }

    pub fn from_f64(value: f64) -> Result<Self, String> {
        if !value.is_finite() {
            return Err("decimal value must be finite".to_string());
        }

        let scaled = value * SCALE as f64;
        if scaled > i64::MAX as f64 || scaled < i64::MIN as f64 {
            return Err("decimal value is outside supported range".to_string());
        }

        Ok(Self {
            micro_units: scaled.round() as i64,
        })
    }

    pub fn from_decimal_str(value: &str) -> Result<Self, String> {
        let value = value.trim();
        if value.is_empty() {
            return Err("decimal value must not be empty".to_string());
        }

        let (negative, value) = match value.strip_prefix('-') {
            Some(value) => (true, value),
            None => (false, value.strip_prefix('+').unwrap_or(value)),
        };

        let mut parts = value.split('.');
        let whole = parts.next().unwrap_or_default();
        let fractional = parts.next();
        if parts.next().is_some() {
            return Err("decimal value must contain at most one decimal point".to_string());
        }

        if whole.is_empty() && fractional.unwrap_or_default().is_empty() {
            return Err("decimal value must contain digits".to_string());
        }

        if !whole.chars().all(|character| character.is_ascii_digit()) {
            return Err("decimal whole part must contain only digits".to_string());
        }

        let whole_units = if whole.is_empty() {
            0
        } else {
            whole
                .parse::<i64>()
                .map_err(|_| "decimal whole part is outside supported range".to_string())?
        };

        let fractional = fractional.unwrap_or_default();
        if !fractional
            .chars()
            .all(|character| character.is_ascii_digit())
        {
            return Err("decimal fractional part must contain only digits".to_string());
        }

        let mut fractional_digits = fractional.chars();
        let mut fractional_units = 0_i64;
        for _ in 0..6 {
            fractional_units *= 10;
            if let Some(character) = fractional_digits.next() {
                fractional_units += i64::from(character as u8 - b'0');
            }
        }

        if matches!(fractional_digits.next(), Some(character) if character >= '5') {
            fractional_units += 1;
        }

        let micro_units = whole_units
            .checked_mul(SCALE)
            .and_then(|value| value.checked_add(fractional_units))
            .ok_or_else(|| "decimal value is outside supported range".to_string())?;

        Ok(Self {
            micro_units: if negative { -micro_units } else { micro_units },
        })
    }

    pub fn ratio_to(self, denominator: Self) -> f64 {
        self.micro_units as f64 / denominator.micro_units as f64
    }
}

impl Display for Decimal {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let sign = if self.micro_units < 0 { "-" } else { "" };
        let absolute = self.micro_units.abs();
        let whole = absolute / SCALE;
        let fractional = absolute % SCALE;

        if fractional == 0 {
            write!(f, "{sign}{whole}")
        } else {
            let mut fractional = format!("{fractional:06}");
            while fractional.ends_with('0') {
                fractional.pop();
            }
            write!(f, "{sign}{whole}.{fractional}")
        }
    }
}

impl Add for Decimal {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self::from_micro_units(self.micro_units + rhs.micro_units)
    }
}

impl AddAssign for Decimal {
    fn add_assign(&mut self, rhs: Self) {
        self.micro_units += rhs.micro_units;
    }
}

impl Sub for Decimal {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self::from_micro_units(self.micro_units - rhs.micro_units)
    }
}

impl SubAssign for Decimal {
    fn sub_assign(&mut self, rhs: Self) {
        self.micro_units -= rhs.micro_units;
    }
}

impl Mul for Decimal {
    type Output = Self;

    fn mul(self, rhs: Self) -> Self::Output {
        let scaled = (self.micro_units as i128 * rhs.micro_units as i128) / SCALE as i128;
        Self::from_micro_units(scaled as i64)
    }
}

impl Div for Decimal {
    type Output = Self;

    fn div(self, rhs: Self) -> Self::Output {
        let scaled = (self.micro_units as i128 * SCALE as i128) / rhs.micro_units as i128;
        Self::from_micro_units(scaled as i64)
    }
}

impl<'de> Deserialize<'de> for Decimal {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(DecimalVisitor)
    }
}

struct DecimalVisitor;

impl Visitor<'_> for DecimalVisitor {
    type Value = Decimal;

    fn expecting(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a finite decimal number")
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Decimal::from_f64(value as f64).map_err(E::custom)
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Decimal::from_f64(value as f64).map_err(E::custom)
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Decimal::from_f64(value).map_err(E::custom)
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Decimal::from_decimal_str(value).map_err(E::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::Decimal;

    #[test]
    fn stores_micro_units_without_binary_float_arithmetic() {
        let lhs = Decimal::from_f64(0.1).expect("decimal should parse");
        let rhs = Decimal::from_f64(0.2).expect("decimal should parse");

        assert_eq!((lhs + rhs).to_string(), "0.3");
    }

    #[test]
    fn multiplies_with_fixed_scale() {
        let quantity = Decimal::from_f64(0.01).expect("decimal should parse");
        let price = Decimal::from_f64(101.0).expect("decimal should parse");

        assert_eq!((quantity * price).to_string(), "1.01");
    }

    #[test]
    fn trims_display_trailing_zeroes() {
        assert_eq!(
            Decimal::from_micro_units(9_998_465_000).to_string(),
            "9998.465"
        );
    }

    #[test]
    fn parses_decimal_strings_without_binary_float_arithmetic() {
        assert_eq!(
            Decimal::from_decimal_str("1011.1908877900")
                .expect("decimal should parse")
                .to_string(),
            "1011.190888"
        );
        assert_eq!(
            Decimal::from_decimal_str(".25")
                .expect("decimal should parse")
                .to_string(),
            "0.25"
        );
    }
}
