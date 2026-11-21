//! Plain-language helpers shared by plan reasons, scan notes and report headers, so every count
//! is said the same way ("1 file is", "2 files are").

/// `n` with the noun that agrees with it: `count(1, "file", "files")` is `1 file`.
pub fn count(n: u64, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// The word that agrees with `n`: `agree(1, "is", "are")` is `is`.
pub fn agree<'a>(n: u64, one: &'a str, many: &'a str) -> &'a str {
    if n == 1 {
        one
    } else {
        many
    }
}

/// Bytes in whole KiB, rounded up: `kib(1)` is `1 KiB`, `kib(0)` is `0 KiB`.
pub fn kib(bytes: u64) -> String {
    format!("{} KiB", bytes.div_ceil(1024))
}

/// Bytes in 1024-based units with one decimal: `size(1536)` is `1.5 KiB`, `size(5)` is `5 B`.
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

/// Up to `max` of `items`, comma separated, then `and N more` for the rest.
pub fn some_of(items: &[String], max: usize) -> String {
    let shown = items
        .iter()
        .take(max)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let rest = items.len().saturating_sub(max);
    if rest > 0 {
        format!("{shown} and {rest} more")
    } else {
        shown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_agree_with_their_number() {
        assert_eq!(count(0, "file", "files"), "0 files");
        assert_eq!(count(1, "file", "files"), "1 file");
        assert_eq!(count(2, "entry", "entries"), "2 entries");
        assert_eq!(agree(1, "is", "are"), "is");
        assert_eq!(agree(3, "is", "are"), "are");
        assert_eq!(kib(0), "0 KiB");
        assert_eq!(kib(1), "1 KiB");
        assert_eq!(kib(2048), "2 KiB");
        assert_eq!(kib(2049), "3 KiB");
        assert_eq!(size(5), "5 B");
        assert_eq!(size(1536), "1.5 KiB");
        assert_eq!(size(3 * 1024 * 1024 * 1024), "3.0 GiB");
    }

    #[test]
    fn some_of_names_up_to_max_and_counts_the_rest() {
        let v: Vec<String> = "abcd".chars().map(String::from).collect();
        assert_eq!(some_of(&v[..1], 3), "a");
        assert_eq!(some_of(&v[..3], 3), "a, b, c");
        assert_eq!(some_of(&v, 3), "a, b, c and 1 more");
    }
}
