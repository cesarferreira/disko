//! Human-readable byte formatting.

/// Which size a caller wants when a tree is queried.
///
/// `Allocated` is what the filesystem actually took (`du`), `Apparent` is the
/// sum of file lengths (`ls`). Sparse files and compression make them differ.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum SizeKind {
    #[default]
    Allocated,
    Apparent,
}

/// Decimal (GB, what disk vendors and macOS print) or binary (GiB).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum Unit {
    #[default]
    Decimal,
    Binary,
}

const DECIMAL: [&str; 7] = ["B", "KB", "MB", "GB", "TB", "PB", "EB"];
const BINARY: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];

/// `4096` -> `"4.1 KB"`, `433791631360` -> `"434 GB"`.
///
/// Values below 10 keep one decimal, larger ones are rounded to a whole
/// number: at 100 GB the tenths digit is noise, and ragged decimals make a
/// column of sizes harder to compare at a glance.
pub fn format(bytes: u64, unit: Unit) -> String {
    let (base, names) = match unit {
        Unit::Decimal => (1000.0_f64, DECIMAL),
        Unit::Binary => (1024.0_f64, BINARY),
    };

    if (bytes as f64) < base {
        return format!("{bytes} B");
    }

    let mut value = bytes as f64;
    let mut idx = 0;
    while value >= base && idx + 1 < names.len() {
        value /= base;
        idx += 1;
    }

    if value < 10.0 {
        let rendered = format!("{value:.1}");
        let trimmed = rendered.strip_suffix(".0").unwrap_or(&rendered);
        format!("{trimmed} {}", names[idx])
    } else {
        format!("{value:.0} {}", names[idx])
    }
}

/// Same as [`format`], right-aligned into `width` so a column lines up.
pub fn format_padded(bytes: u64, unit: Unit, width: usize) -> String {
    format!("{:>width$}", format(bytes, unit), width = width)
}

/// `0.187` -> `"18.7%"`, `0.82` -> `"82%"`. Used for the per-row share of the
/// current directory, where a tenth of a percent still distinguishes two rows.
pub fn format_percent(fraction: f64) -> String {
    let pct = (fraction * 100.0).clamp(0.0, 100.0);
    let rendered = format!("{pct:.1}");
    let trimmed = rendered.strip_suffix(".0").unwrap_or(&rendered);
    format!("{trimmed}%")
}

/// `0.817` -> `"82%"`. Used for the capacity header, where the extra digit
/// only adds noise.
pub fn format_percent_whole(fraction: f64) -> String {
    format!("{:.0}%", (fraction * 100.0).clamp(0.0, 100.0))
}

/// Guards against the 0/0 that an empty directory would otherwise produce.
pub fn fraction(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 / whole as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_decimal_sizes() {
        assert_eq!(format(0, Unit::Decimal), "0 B");
        assert_eq!(format(999, Unit::Decimal), "999 B");
        assert_eq!(format(1_000, Unit::Decimal), "1 KB");
        assert_eq!(format(1_500, Unit::Decimal), "1.5 KB");
        assert_eq!(format(494_000_000_000, Unit::Decimal), "494 GB");
        assert_eq!(format(4_000_000_000, Unit::Decimal), "4 GB");
        assert_eq!(format(4_200_000_000, Unit::Decimal), "4.2 GB");
    }

    #[test]
    fn formats_binary_sizes() {
        assert_eq!(format(1_024, Unit::Binary), "1 KiB");
        assert_eq!(format(1_048_576, Unit::Binary), "1 MiB");
    }

    #[test]
    fn percentages_drop_a_trailing_zero_decimal() {
        assert_eq!(format_percent(0.187), "18.7%");
        assert_eq!(format_percent(0.82), "82%");
        assert_eq!(format_percent(1.0), "100%");
        assert_eq!(format_percent_whole(0.817), "82%");
    }

    #[test]
    fn fraction_of_nothing_is_zero() {
        assert_eq!(fraction(10, 0), 0.0);
        assert_eq!(fraction(1, 4), 0.25);
    }
}
