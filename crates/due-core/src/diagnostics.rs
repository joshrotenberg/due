//! Shared bounded provider diagnostics.

const DIAGNOSTIC_LIMIT: usize = 16 * 1024;
const MARKER: &str = " [truncated]";

pub fn diagnostic(text: &str) -> String {
    diagnostic_with_limit(text, DIAGNOSTIC_LIMIT)
}

fn diagnostic_with_limit(text: &str, limit: usize) -> String {
    // Precondition: limit can hold MARKER; callers use the fixed 16 KiB budget.
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit.saturating_sub(MARKER.len()).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &text[..end], MARKER)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn utf8_diagnostic_boundary() {
        assert_eq!(diagnostic_with_limit("ééé", 6), "ééé");
        let short = diagnostic_with_limit(&"é".repeat(20), 17);
        assert!(short.len() <= 17 && short.ends_with(MARKER));
    }
}
