//! Amounts are stored as integer agorot so non-round values are exact.
//! Paamonim only accepts whole shekels, so exports round via [`to_paamonim_shekels`].

/// Round agorot to whole shekels, half away from zero.
pub fn to_paamonim_shekels(agorot: i64) -> i64 {
    let half = if agorot >= 0 { 50 } else { -50 };
    (agorot + half) / 100
}

pub fn from_shekels(shekels: i64) -> i64 {
    shekels * 100
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_half_away_from_zero() {
        assert_eq!(to_paamonim_shekels(1049), 10);
        assert_eq!(to_paamonim_shekels(1050), 11);
        assert_eq!(to_paamonim_shekels(-1050), -11);
        assert_eq!(to_paamonim_shekels(-1049), -10);
        assert_eq!(to_paamonim_shekels(0), 0);
    }
}
