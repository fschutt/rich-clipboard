//! `FILETIME`, as a raw tick count.
//!
//! Deliberately not a calendar type. Converting to one needs either a date
//! library — a dependency this workspace does not want in a codec — or a
//! hand-rolled civil-from-days routine, and the seeding template's attempt at
//! the latter is a good argument against: it mixed up the 1601 and 1970 epochs,
//! used a `START_YEAR_UNIX` of 1900, and produced month numbers off by one.
//!
//! Callers that want a `DateTime` have a date library already. This type hands
//! them the number and the epoch and gets out of the way.

use rclip_core::Reader;

/// 100-nanosecond intervals since 1601-01-01 00:00:00 UTC (MS-DTYP 2.3.3).
///
/// Zero means "not recorded", which is common and is *not* the year 1601 —
/// check [`FileTime::is_unset`] before doing arithmetic on it.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct FileTime(pub u64);

impl FileTime {
    /// Ticks between 1601-01-01 and 1970-01-01: 134 774 days.
    pub const UNIX_EPOCH_TICKS: u64 = 116_444_736_000_000_000;
    /// Ticks per second.
    pub const TICKS_PER_SECOND: u64 = 10_000_000;

    #[must_use]
    pub const fn is_unset(self) -> bool {
        self.0 == 0
    }

    /// Whole seconds since the Unix epoch, or `None` if unset.
    ///
    /// Signed, because a `FILETIME` can legitimately predate 1970 and saturating
    /// to zero would silently turn a 1980 timestamp into 1970.
    #[must_use]
    pub const fn unix_seconds(self) -> Option<i64> {
        if self.is_unset() {
            return None;
        }
        let ticks = self.0 as i128 - Self::UNIX_EPOCH_TICKS as i128;
        Some((ticks / Self::TICKS_PER_SECOND as i128) as i64)
    }

    /// Build from whole seconds since the Unix epoch. Negative input before
    /// 1601 clamps to zero, which reads back as "unset".
    #[must_use]
    pub const fn from_unix_seconds(secs: i64) -> Self {
        let ticks = secs as i128 * Self::TICKS_PER_SECOND as i128
            + Self::UNIX_EPOCH_TICKS as i128;
        if ticks < 0 {
            Self(0)
        } else {
            Self(ticks as u64)
        }
    }

    pub(crate) fn read(r: &mut Reader<'_>) -> rclip_core::Result<Self> {
        r.u64_le().map(Self)
    }

    #[must_use]
    pub const fn to_le_bytes(self) -> [u8; 8] {
        self.0.to_le_bytes()
    }
}
