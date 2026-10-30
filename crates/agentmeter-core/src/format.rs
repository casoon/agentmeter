//! Number formatting shared by messages and tables.

/// `7.77 G`, `739 M`, `78 k`, `950`.
pub fn tokens(n: u64) -> String {
    let n_f = n as f64;
    if n >= 1_000_000_000 {
        format!("{:.2} G", n_f / 1e9)
    } else if n >= 100_000_000 {
        format!("{:.0} M", n_f / 1e6)
    } else if n >= 1_000_000 {
        format!("{:.1} M", n_f / 1e6)
    } else if n >= 1000 {
        format!("{:.0}k", n_f / 1e3)
    } else {
        n.to_string()
    }
}

/// `29,902`.
pub fn count(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(tokens(7_770_000_000), "7.77 G");
        assert_eq!(tokens(739_000_000), "739 M");
        assert_eq!(tokens(78_400), "78k");
        assert_eq!(count(29_902), "29,902");
        assert_eq!(count(7), "7");
    }
}
