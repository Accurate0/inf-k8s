//! Types shared between the ai-gateway server and its admin CLI, kept free of server
//! dependencies so the CLI doesn't link sqlx or redis.

use std::{fmt, str::FromStr};

use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    amount: i64,
    unit: char,
}

impl Window {
    const MAX_SECONDS: i64 = 3650 * 86_400;

    fn unit_seconds(unit: char) -> Option<i64> {
        match unit {
            'm' => Some(60),
            'h' => Some(3600),
            'd' => Some(86_400),
            'w' => Some(604_800),
            _ => None,
        }
    }

    pub fn seconds(&self) -> i64 {
        self.amount * Self::unit_seconds(self.unit).unwrap_or(0)
    }

    pub fn start(&self) -> DateTime<Utc> {
        Utc::now() - TimeDelta::seconds(self.seconds())
    }
}

impl Default for Window {
    fn default() -> Self {
        Self {
            amount: 7,
            unit: 'd',
        }
    }
}

impl fmt::Display for Window {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.amount, self.unit)
    }
}

impl FromStr for Window {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let invalid =
            || format!("invalid window `{s}`; expected a number and unit like 30m, 24h, 7d or 2w");

        let Some(unit) = s.chars().last() else {
            return Err(invalid());
        };

        let Some(unit_seconds) = Self::unit_seconds(unit) else {
            return Err(invalid());
        };

        let digits = &s[..s.len() - unit.len_utf8()];

        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(invalid());
        }

        let amount: i64 = digits.parse().map_err(|_| invalid())?;

        if amount == 0 {
            return Err(invalid());
        }

        match amount.checked_mul(unit_seconds) {
            Some(seconds) if seconds <= Self::MAX_SECONDS => Ok(Self { amount, unit }),
            _ => Err(format!("window `{s}` is too large; the maximum is 3650d")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageRow {
    pub key_name: String,
    pub model: String,
    pub requests: i64,
    pub cache_hits: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_parses_each_unit() {
        let seconds = |s: &str| s.parse::<Window>().unwrap().seconds();

        assert_eq!(seconds("30m"), 1800);
        assert_eq!(seconds("24h"), 86_400);
        assert_eq!(seconds("7d"), 604_800);
        assert_eq!(seconds("2w"), 1_209_600);
    }

    #[test]
    fn window_round_trips_through_display() {
        for text in ["30m", "24h", "7d", "2w"] {
            assert_eq!(text.parse::<Window>().unwrap().to_string(), text);
        }

        assert_eq!(Window::default().to_string(), "7d");
    }

    #[test]
    fn window_rejects_malformed_input() {
        for text in [
            "", "7", "d", "0d", "-1d", "+1d", "1.5h", "7 d", "7y", "7dd", "7é",
        ] {
            assert!(text.parse::<Window>().is_err(), "{text:?} should not parse");
        }
    }

    #[test]
    fn window_rejects_oversized_spans() {
        assert!("3650d".parse::<Window>().is_ok());
        assert!("3651d".parse::<Window>().is_err());
        assert!("9223372036854775807w".parse::<Window>().is_err());
    }
}
