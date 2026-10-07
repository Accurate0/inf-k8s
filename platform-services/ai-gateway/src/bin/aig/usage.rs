use std::fmt;

use ai_gateway::usage::{UsageRow, Window};

const COLUMNS: usize = 7;
const LEFT_ALIGNED: usize = 2;
const HEADER: [&str; COLUMNS] = [
    "KEY", "MODEL", "REQUESTS", "CACHED", "INPUT", "OUTPUT", "COST",
];

pub struct UsageTable {
    window: Window,
    rows: Vec<UsageRow>,
}

impl UsageTable {
    pub fn new(window: Window, rows: Vec<UsageRow>) -> Self {
        Self { window, rows }
    }

    fn grouped(value: i64) -> String {
        let digits = value.unsigned_abs().to_string();
        let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);

        if value < 0 {
            out.push('-');
        }

        for (index, digit) in digits.chars().enumerate() {
            if index > 0 && (digits.len() - index).is_multiple_of(3) {
                out.push(',');
            }

            out.push(digit);
        }

        out
    }

    fn cells(key: &str, model: &str, row: &UsageRow) -> [String; COLUMNS] {
        [
            key.to_owned(),
            model.to_owned(),
            Self::grouped(row.requests),
            Self::grouped(row.cache_hits),
            Self::grouped(row.input_tokens),
            Self::grouped(row.output_tokens),
            format!("${:.4}", row.cost_usd),
        ]
    }

    fn total(&self) -> UsageRow {
        let mut total = UsageRow {
            key_name: String::new(),
            model: String::new(),
            requests: 0,
            cache_hits: 0,
            input_tokens: 0,
            output_tokens: 0,
            cost_usd: 0.0,
        };

        for row in &self.rows {
            total.requests += row.requests;
            total.cache_hits += row.cache_hits;
            total.input_tokens += row.input_tokens;
            total.output_tokens += row.output_tokens;
            total.cost_usd += row.cost_usd;
        }

        total
    }

    fn write_line(
        f: &mut fmt::Formatter<'_>,
        cells: &[String; COLUMNS],
        widths: &[usize; COLUMNS],
    ) -> fmt::Result {
        let mut line = String::new();

        for (index, (cell, width)) in cells.iter().zip(widths).enumerate() {
            if index > 0 {
                line.push_str("  ");
            }

            if index < LEFT_ALIGNED {
                line.push_str(&format!("{cell:<width$}"));
            } else {
                line.push_str(&format!("{cell:>width$}"));
            }
        }

        writeln!(f, "{}", line.trim_end())
    }
}

impl fmt::Display for UsageTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.rows.is_empty() {
            return writeln!(f, "No usage in the last {}.", self.window);
        }

        let header = HEADER.map(str::to_owned);
        let total = Self::cells("TOTAL", "", &self.total());

        let mut previous_key = None;
        let body: Vec<_> = self
            .rows
            .iter()
            .map(|row| {
                let repeated = previous_key == Some(row.key_name.as_str());
                previous_key = Some(row.key_name.as_str());

                let key = if repeated { "" } else { row.key_name.as_str() };

                Self::cells(key, &row.model, row)
            })
            .collect();

        let mut widths = [0usize; COLUMNS];

        for cells in body.iter().chain([&header, &total]) {
            for (width, cell) in widths.iter_mut().zip(cells) {
                *width = (*width).max(cell.chars().count());
            }
        }

        let rule = "-".repeat(widths.iter().sum::<usize>() + 2 * (COLUMNS - 1));

        writeln!(f, "Usage for the last {}", self.window)?;
        writeln!(f)?;

        Self::write_line(f, &header, &widths)?;
        writeln!(f, "{rule}")?;

        for cells in &body {
            Self::write_line(f, cells, &widths)?;
        }

        writeln!(f, "{rule}")?;
        Self::write_line(f, &total, &widths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(key: &str, model: &str, requests: i64, input: i64, output: i64, cost: f64) -> UsageRow {
        UsageRow {
            key_name: key.to_owned(),
            model: model.to_owned(),
            requests,
            cache_hits: 0,
            input_tokens: input,
            output_tokens: output,
            cost_usd: cost,
        }
    }

    #[test]
    fn groups_thousands() {
        assert_eq!(UsageTable::grouped(0), "0");
        assert_eq!(UsageTable::grouped(999), "999");
        assert_eq!(UsageTable::grouped(1000), "1,000");
        assert_eq!(UsageTable::grouped(1_234_567), "1,234,567");
        assert_eq!(UsageTable::grouped(-12_345), "-12,345");
    }

    #[test]
    fn renders_rows_grouped_by_key_with_a_total() {
        let table = UsageTable::new(
            "7d".parse().unwrap(),
            vec![
                row(
                    "claude-code",
                    "claude-opus-4-8",
                    1200,
                    3_400_000,
                    250_000,
                    69.75,
                ),
                row("claude-code", "claude-haiku-4-5", 80, 12_000, 900, 0.0165),
                row("janitor-bot", "gpt-5-mini", 5, 700, 60, 0.0),
            ],
        );

        let expected = "\
Usage for the last 7d

KEY          MODEL             REQUESTS  CACHED      INPUT   OUTPUT      COST
-----------------------------------------------------------------------------
claude-code  claude-opus-4-8      1,200       0  3,400,000  250,000  $69.7500
             claude-haiku-4-5        80       0     12,000      900   $0.0165
janitor-bot  gpt-5-mini               5       0        700       60   $0.0000
-----------------------------------------------------------------------------
TOTAL                             1,285       0  3,412,700  250,960  $69.7665
";

        assert_eq!(table.to_string(), expected);
    }

    #[test]
    fn reports_an_empty_window() {
        let table = UsageTable::new("24h".parse().unwrap(), Vec::new());

        assert_eq!(table.to_string(), "No usage in the last 24h.\n");
    }
}
