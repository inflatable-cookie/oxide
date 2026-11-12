//! Basic test-execution correspondence: two plain `#[test]` functions. The
//! driver requires a zero exit and both test names in the combined output.
//! This is a correspondence probe, not test-route qualification.

/// Adds two numbers; probed by `adds`.
pub fn add(left: u64, right: u64) -> u64 {
    left + right
}

/// Joins two words; probed by `concats`.
pub fn concat(left: &str, right: &str) -> String {
    format!("{left}{right}")
}

#[cfg(test)]
mod tests {
    use super::{add, concat};

    #[test]
    fn adds() {
        assert_eq!(add(1, 1), 2);
    }

    #[test]
    fn concats() {
        assert_eq!(concat("a", "b"), "ab");
    }
}
