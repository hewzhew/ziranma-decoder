//! Text-free input-mode state shared with the optional notification-area UI.
//!
//! The cross-process compartment contains exactly one bounded integer. It
//! never carries input codes, candidate text, committed text, window handles,
//! or document identity.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum PublishedInputMode {
    Chinese = 1,
    English = 2,
}

impl PublishedInputMode {
    pub fn parse(raw: i32) -> Option<Self> {
        match raw {
            1 => Some(Self::Chinese),
            2 => Some(Self::English),
            _ => None,
        }
    }

    pub const fn raw(self) -> i32 {
        self as i32
    }
}

#[cfg(windows)]
use windows::core::GUID;

/// Current-user TSF global compartment used by the optional mode indicator.
#[cfg(windows)]
pub const INPUT_MODE_STATUS_COMPARTMENT_GUID: GUID =
    GUID::from_u128(0xec15904a_1450_4b69_a3c4_b5a454fc4d82);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_mode_words_are_small_and_fail_closed() {
        assert_eq!(
            PublishedInputMode::parse(1),
            Some(PublishedInputMode::Chinese)
        );
        assert_eq!(
            PublishedInputMode::parse(2),
            Some(PublishedInputMode::English)
        );
        for invalid in [i32::MIN, -1, 0, 3, i32::MAX] {
            assert_eq!(PublishedInputMode::parse(invalid), None);
        }
        assert_eq!(PublishedInputMode::Chinese.raw(), 1);
        assert_eq!(PublishedInputMode::English.raw(), 2);
    }
}
