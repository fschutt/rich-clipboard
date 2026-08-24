//! MS-DOS (FAT) packed date and time, as carried by file entry shell items.
//!
//! Kept as the raw words plus accessors rather than converted to a calendar
//! type. A date library is a dependency this workspace does not want, and the
//! encoding is lossy in ways a `DateTime` would paper over: two-second
//! granularity, no time zone, and a valid-looking encoding for
//! "31 February 1980".

/// A FAT date/time pair, as stored: the date word first, then the time word.
///
/// A zero value means "not recorded" and is extremely common; check
/// [`DosDateTime::is_unset`] before reading the fields.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct DosDateTime {
    /// Bits 15-9 year since 1980, 8-5 month, 4-0 day.
    pub date: u16,
    /// Bits 15-11 hours, 10-5 minutes, 4-0 seconds divided by two.
    pub time: u16,
}

impl DosDateTime {
    /// Read the four on-the-wire bytes: date word, then time word, both
    /// little-endian.
    #[must_use]
    pub const fn from_le_bytes(b: [u8; 4]) -> Self {
        Self {
            date: u16::from_le_bytes([b[0], b[1]]),
            time: u16::from_le_bytes([b[2], b[3]]),
        }
    }

    #[must_use]
    pub const fn to_le_bytes(self) -> [u8; 4] {
        let d = self.date.to_le_bytes();
        let t = self.time.to_le_bytes();
        [d[0], d[1], t[0], t[1]]
    }

    /// `true` if both words are zero, i.e. the field was never filled in.
    #[must_use]
    pub const fn is_unset(self) -> bool {
        self.date == 0 && self.time == 0
    }

    /// Full year. The FAT epoch is 1980, so this ranges 1980..=2107.
    #[must_use]
    pub const fn year(self) -> u16 {
        1980 + (self.date >> 9)
    }

    /// 1-12 on well-formed input. Not validated — FAT allows nonsense and this
    /// crate reports what is there.
    #[must_use]
    pub const fn month(self) -> u8 {
        ((self.date >> 5) & 0x0F) as u8
    }

    /// 1-31 on well-formed input, not validated.
    #[must_use]
    pub const fn day(self) -> u8 {
        (self.date & 0x1F) as u8
    }

    /// 0-23 on well-formed input, not validated.
    #[must_use]
    pub const fn hour(self) -> u8 {
        (self.time >> 11) as u8
    }

    /// 0-59 on well-formed input, not validated.
    #[must_use]
    pub const fn minute(self) -> u8 {
        ((self.time >> 5) & 0x3F) as u8
    }

    /// Even, 0-62. FAT stores seconds divided by two, so an odd second is
    /// simply not representable — and the five-bit field can encode 30 and 31,
    /// which are not real clock values. Reported as-is rather than clamped,
    /// like every other field here.
    #[must_use]
    pub const fn second(self) -> u8 {
        ((self.time & 0x1F) * 2) as u8
    }
}
