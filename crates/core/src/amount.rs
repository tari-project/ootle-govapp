//! Fee amounts as people type them: TARI with up to six decimals.

use tari_template_lib_types::Amount;

pub const MICRO_PER_TARI: u128 = 1_000_000;

/// Parses `10`, `0.5` or `2.125` TARI into microtari.
pub fn parse_tari(s: &str) -> Result<Amount, String> {
    let s = s.trim();
    let (whole, frac) = s.split_once('.').unwrap_or((s, ""));
    if whole.is_empty() && frac.is_empty() {
        return Err("enter an amount".to_string());
    }
    if frac.len() > 6 || !whole.chars().chain(frac.chars()).all(|c| c.is_ascii_digit()) {
        return Err(format!("`{s}` is not an amount of TARI with at most six decimals"));
    }
    let whole: u128 = if whole.is_empty() {
        0
    } else {
        whole.parse().map_err(|_| format!("`{s}` is too large"))?
    };
    let frac: u128 = format!("{frac:0<6}").parse().expect("six ASCII digits");
    whole
        .checked_mul(MICRO_PER_TARI)
        .and_then(|w| w.checked_add(frac))
        .map(Amount::new)
        .ok_or_else(|| format!("`{s}` is too large"))
}

/// Renders microtari as TARI, trimming trailing zeros: `10_500_000` → `10.5`.
pub fn format_tari(amount: Amount) -> String {
    let micro: u128 = amount.to_string().parse().unwrap_or_default();
    let whole = micro / MICRO_PER_TARI;
    let frac = micro % MICRO_PER_TARI;
    if frac == 0 {
        return whole.to_string();
    }
    format!("{whole}.{}", format!("{frac:06}").trim_end_matches('0'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_whole_and_fractional_tari() {
        assert_eq!(parse_tari("10").unwrap(), Amount::new(10_000_000));
        assert_eq!(parse_tari("0.5").unwrap(), Amount::new(500_000));
        assert_eq!(parse_tari(".000001").unwrap(), Amount::new(1));
        assert!(parse_tari("1.0000001").is_err());
        assert!(parse_tari("-1").is_err());
        assert!(parse_tari("").is_err());
    }

    #[test]
    fn formats_microtari_as_tari() {
        assert_eq!(format_tari(Amount::new(10_000_000)), "10");
        assert_eq!(format_tari(Amount::new(10_500_000)), "10.5");
        assert_eq!(format_tari(Amount::new(1)), "0.000001");
    }
}
