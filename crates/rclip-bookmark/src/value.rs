//! Data records and the values they decode to.
//!
//! A record is `u32 length | u32 type | length bytes of payload`, padded out to
//! a four-byte boundary. The padding is *not* counted by the length field, so a
//! reader that trusts `length` never sees it — which is why nothing here does
//! anything about alignment.
//!
//! The type word splits into a type in the high 24 bits and a subtype in the
//! low 8. Numbers use `CFNumberType` as the subtype, which is why "int32" and
//! "int64" are `0x0303` and `0x0304` and not two adjacent numbers.

use rclip_core::{Error, ErrorKind, Result};

use crate::Bookmark;

/// High 24 bits of a record's type word.
pub const TYPE_MASK: u32 = 0xFFFF_FF00;
/// Low 8 bits of a record's type word.
pub const SUBTYPE_MASK: u32 = 0x0000_00FF;

/// Record type codes, masked with [`TYPE_MASK`].
pub mod ty {
    pub const STRING: u32 = 0x0100;
    pub const DATA: u32 = 0x0200;
    pub const NUMBER: u32 = 0x0300;
    pub const DATE: u32 = 0x0400;
    pub const BOOLEAN: u32 = 0x0500;
    pub const ARRAY: u32 = 0x0600;
    pub const DICT: u32 = 0x0700;
    pub const UUID: u32 = 0x0800;
    pub const URL: u32 = 0x0900;
    pub const NULL: u32 = 0x0A00;
}

/// `CFNumberType` values, used as the subtype of a [`ty::NUMBER`] record.
pub mod number {
    pub const SINT8: u32 = 1;
    pub const SINT16: u32 = 2;
    pub const SINT32: u32 = 3;
    pub const SINT64: u32 = 4;
    pub const FLOAT32: u32 = 5;
    pub const FLOAT64: u32 = 6;
}

/// Subtypes of a [`ty::URL`] record.
pub mod url {
    pub const ABSOLUTE: u32 = 1;
    pub const RELATIVE: u32 = 2;
}

/// Seconds between the Unix epoch (1970-01-01) and the Core Foundation epoch
/// (2001-01-01), both UTC.
pub const CF_EPOCH_UNIX_SECS: f64 = 978_307_200.0;

/// A `0x0400` date.
///
/// Stored as a **big-endian** IEEE double giving seconds since 2001-01-01 UTC —
/// the single big-endian field in an otherwise little-endian format. Reading it
/// little-endian does not fail, it just yields a date around the year 10^300,
/// which is why this is worth a type of its own.
#[derive(Debug, Copy, Clone, PartialEq, PartialOrd)]
pub struct Date(f64);

impl Date {
    #[must_use]
    pub const fn from_absolute_seconds(secs: f64) -> Self {
        Self(secs)
    }

    /// Seconds since 2001-01-01 UTC, as stored.
    #[must_use]
    pub const fn absolute_seconds(self) -> f64 {
        self.0
    }

    /// Seconds since 1970-01-01 UTC, for handing to anything that speaks Unix
    /// time. No leap-second handling, because Core Foundation does none either.
    #[must_use]
    pub fn unix_seconds(self) -> f64 {
        self.0 + CF_EPOCH_UNIX_SECS
    }
}

/// A decoded record.
///
/// Containers are lazy: an [`Array`] or [`Dict`] holds the raw offset table and
/// resolves elements on demand, so parsing a bookmark never walks the whole
/// object graph and a malformed graph costs nothing until something asks for
/// it. [`Bookmark::validate`](crate::Bookmark::validate) is the opt-in walk.
#[derive(Debug, Copy, Clone, PartialEq)]
#[non_exhaustive]
pub enum Value<'a> {
    Null,
    Bool(bool),
    I8(i8),
    I16(i16),
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
    Date(Date),
    /// A `0x0101` UTF-8 string. Not NUL-terminated; any trailing zero byte is
    /// record padding and is outside the length.
    Str(&'a str),
    /// A `0x0201` opaque byte string.
    Data(&'a [u8]),
    /// A `0x0801` UUID, sixteen raw bytes in wire order.
    Uuid([u8; 16]),
    /// A `0x0901` absolute URL.
    Url(&'a str),
    /// A `0x0902` URL expressed relative to another one.
    RelativeUrl(RelativeUrl<'a>),
    Array(Array<'a>),
    Dict(Dict<'a>),
    /// A well-formed record whose type this crate does not decode — an
    /// unhandled `CFNumberType`, or a type Apple has added since. The payload
    /// is handed back untouched so a caller can still salvage it.
    Unknown {
        type_code: u32,
        data: &'a [u8],
    },
}

impl<'a> Value<'a> {
    /// The string behind a [`Value::Str`] or [`Value::Url`].
    ///
    /// Both spellings turn up for the same logical field — `0x2011` holds a
    /// volume UUID as a string, `0x2005` holds a volume URL as a URL — so
    /// callers that only want the characters should not have to match on which.
    #[must_use]
    pub const fn as_str(&self) -> Option<&'a str> {
        match self {
            Self::Str(s) | Self::Url(s) => Some(s),
            _ => None,
        }
    }

    /// The payload of a [`Value::Data`].
    #[must_use]
    pub const fn as_data(&self) -> Option<&'a [u8]> {
        match self {
            Self::Data(d) => Some(d),
            _ => None,
        }
    }

    /// Any of the integer records, widened to `i64`.
    #[must_use]
    pub const fn as_i64(&self) -> Option<i64> {
        Some(match *self {
            Self::I8(v) => v as i64,
            Self::I16(v) => v as i64,
            Self::I32(v) => v as i64,
            Self::I64(v) => v,
            _ => return None,
        })
    }

    /// Any of the floating-point records, widened to `f64`. Deliberately does
    /// *not* include [`Value::Date`]: a date is a point in time on a different
    /// epoch, and quietly returning its raw seconds as a number is how the
    /// 2001-vs-1970 offset gets lost.
    #[must_use]
    pub fn as_f64(&self) -> Option<f64> {
        Some(match *self {
            Self::F32(v) => f64::from(v),
            Self::F64(v) => v,
            _ => return None,
        })
    }

    #[must_use]
    pub const fn as_bool(&self) -> Option<bool> {
        match *self {
            Self::Bool(v) => Some(v),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_date(&self) -> Option<Date> {
        match *self {
            Self::Date(v) => Some(v),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_array(&self) -> Option<Array<'a>> {
        match *self {
            Self::Array(a) => Some(a),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_dict(&self) -> Option<Dict<'a>> {
        match *self {
            Self::Dict(d) => Some(d),
            _ => None,
        }
    }
}

/// A `0x0601` array: a flat table of 4-byte record offsets.
///
/// Carries the depth it was resolved at so that resolving an element can charge
/// against [`rclip_core::MAX_DEPTH`]. Without that, an array whose element
/// offset points back at the array is an infinite descent.
#[derive(Debug, Copy, Clone)]
pub struct Array<'a> {
    pub(crate) bm: Bookmark<'a>,
    pub(crate) offsets: &'a [u8],
    pub(crate) at: usize,
    pub(crate) depth: u32,
}

impl PartialEq for Array<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.at == other.at && self.offsets.as_ptr() == other.offsets.as_ptr()
    }
}

impl<'a> Array<'a> {
    /// Number of elements. Derived from the record length, never from a count
    /// field, so it cannot claim more elements than the record holds.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.offsets.len() / 4
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Absolute offset of the array record in the caller's buffer.
    #[must_use]
    pub const fn offset(&self) -> usize {
        self.at
    }

    /// Resolve element `index`.
    pub fn get(&self, index: usize) -> Result<Value<'a>> {
        let off = read_offset(self.offsets, index, self.at)?;
        self.bm.value_at(off, self.depth + 1)
    }

    #[must_use]
    pub const fn iter(&self) -> ArrayIter<'a> {
        ArrayIter { array: *self, index: 0 }
    }
}

impl<'a> IntoIterator for Array<'a> {
    type Item = Result<Value<'a>>;
    type IntoIter = ArrayIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// Iterator over the elements of an [`Array`].
#[derive(Debug, Clone)]
pub struct ArrayIter<'a> {
    array: Array<'a>,
    index: usize,
}

impl<'a> Iterator for ArrayIter<'a> {
    type Item = Result<Value<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.array.len() {
            return None;
        }
        let out = self.array.get(self.index);
        self.index += 1;
        Some(out)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.array.len().saturating_sub(self.index);
        (n, Some(n))
    }
}

impl ExactSizeIterator for ArrayIter<'_> {}

/// A `0x0701` dictionary: a flat table of `(key offset, value offset)` pairs.
#[derive(Debug, Copy, Clone)]
pub struct Dict<'a> {
    pub(crate) bm: Bookmark<'a>,
    pub(crate) offsets: &'a [u8],
    pub(crate) at: usize,
    pub(crate) depth: u32,
}

impl PartialEq for Dict<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.at == other.at && self.offsets.as_ptr() == other.offsets.as_ptr()
    }
}

impl<'a> Dict<'a> {
    /// Number of key/value pairs, derived from the record length.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.offsets.len() / 8
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Absolute offset of the dictionary record in the caller's buffer.
    #[must_use]
    pub const fn offset(&self) -> usize {
        self.at
    }

    /// Resolve pair `index` as `(key, value)`.
    pub fn get(&self, index: usize) -> Result<(Value<'a>, Value<'a>)> {
        let key_off = read_offset(self.offsets, index * 2, self.at)?;
        let val_off = read_offset(self.offsets, index * 2 + 1, self.at)?;
        let key = self.bm.value_at(key_off, self.depth + 1)?;
        let val = self.bm.value_at(val_off, self.depth + 1)?;
        Ok((key, val))
    }

    #[must_use]
    pub const fn iter(&self) -> DictIter<'a> {
        DictIter { dict: *self, index: 0 }
    }
}

impl<'a> IntoIterator for Dict<'a> {
    type Item = Result<(Value<'a>, Value<'a>)>;
    type IntoIter = DictIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// Iterator over the pairs of a [`Dict`].
#[derive(Debug, Clone)]
pub struct DictIter<'a> {
    dict: Dict<'a>,
    index: usize,
}

impl<'a> Iterator for DictIter<'a> {
    type Item = Result<(Value<'a>, Value<'a>)>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.dict.len() {
            return None;
        }
        let out = self.dict.get(self.index);
        self.index += 1;
        Some(out)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.dict.len().saturating_sub(self.index);
        (n, Some(n))
    }
}

impl ExactSizeIterator for DictIter<'_> {}

/// A `0x0902` record: a URL given as a base plus a relative part, each of which
/// is itself a record somewhere else in the payload.
#[derive(Debug, Copy, Clone)]
pub struct RelativeUrl<'a> {
    pub(crate) bm: Bookmark<'a>,
    pub(crate) base_offset: u32,
    pub(crate) relative_offset: u32,
    pub(crate) at: usize,
    pub(crate) depth: u32,
}

impl PartialEq for RelativeUrl<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.at == other.at
    }
}

impl<'a> RelativeUrl<'a> {
    /// Absolute offset of the record in the caller's buffer.
    #[must_use]
    pub const fn offset(&self) -> usize {
        self.at
    }

    /// The base URL. Itself often another [`Value::RelativeUrl`], which is why
    /// this counts against the depth limit.
    pub fn base(&self) -> Result<Value<'a>> {
        self.bm.value_at(self.base_offset, self.depth + 1)
    }

    /// The part relative to [`RelativeUrl::base`].
    pub fn relative(&self) -> Result<Value<'a>> {
        self.bm.value_at(self.relative_offset, self.depth + 1)
    }
}

/// Read the `index`-th little-endian `u32` out of an offset table.
///
/// `table` is already a bounds-checked slice of the record's own payload, so
/// the only failure is asking for an index past its end.
fn read_offset(table: &[u8], index: usize, at: usize) -> Result<u32> {
    let start = index
        .checked_mul(4)
        .ok_or(Error::new(ErrorKind::TooLarge, at))?;
    let end = start.checked_add(4).ok_or(Error::new(ErrorKind::TooLarge, at))?;
    let b = table
        .get(start..end)
        .ok_or(Error::new(ErrorKind::BadOffset, at))?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}
