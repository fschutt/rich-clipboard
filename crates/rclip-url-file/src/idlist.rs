//! `IDList=` — the one binary value in a `.url`, and how it is spelled.
//!
//! Behind the `idlist` feature.
//!
//! # The encoding is not "hex", it is `WritePrivateProfileStruct`
//!
//! A `.url` is whatever the Win32 profile API reads and writes, and the profile
//! API has a documented way to put a binary struct in a text file:
//! [`WritePrivateProfileStruct`] emits two uppercase hex digits per byte and
//! then **one extra byte holding a checksum**, and
//! [`GetPrivateProfileStruct`] refuses the value if the checksum does not
//! match. Wine's `dlls/kernel32/profile.c` implements both, and the checksum is
//! the plain sum of the data bytes modulo 256.
//!
//! That is not a guess from the shape of the string. It is the same encoding
//! the `Modified=` key uses, and it is what makes the ninth byte of
//! `Modified=20F06BA06D07BD014D` — the one the unofficial guide calls
//! "a checksum, unimportant" — come out to exactly `0x4D`:
//! `0x20 + 0xF0 + 0x6B + 0xA0 + 0x6D + 0x07 + 0xBD + 0x01 = 0x34D`.
//!
//! So a value of `2N + 2` hex digits decodes to `N` bytes, and those `N` bytes
//! are an [`ITEMIDLIST`](rclip_idlist::ItemIdList).
//!
//! [`WritePrivateProfileStruct`]: https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-writeprivateprofilestructw
//! [`GetPrivateProfileStruct`]: https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-getprivateprofilestructw
//!
//! # Why this allocates, and why it is optional
//!
//! Hex digits are not bytes, so the decoded PIDL is not a contiguous slice of
//! the file and cannot be borrowed from it. Decoding therefore needs `alloc`,
//! and walking the result needs a PIDL parser — two things a crate that only
//! wants `URL=` should not have to link. Hence the feature.
//!
//! # A PIDL out of a `.url` is still a name, not a location
//!
//! Everything [`rclip_idlist`] says applies here with the file's own emphasis:
//! a `.url` is a document that arrived from somewhere, and the PIDL inside it
//! names whatever the writer chose. Decode it, show it, do not bind it.
//!
//! # Example
//!
//! ```
//! use rclip_url_file::{parse, ItemIdList};
//!
//! // Six hex digits: two bytes of PIDL and one of checksum.
//! let text = b"[InternetShortcut]\r\nURL=https://example.com/\r\nIDList=000000\r\n";
//! let f = parse(text).unwrap();
//! let bytes = f.id_list_bytes().unwrap().unwrap();
//! assert_eq!(bytes, [0x00, 0x00]);
//! // A bare list terminator: a valid, empty ITEMIDLIST.
//! let mut list = ItemIdList::new(&bytes);
//! assert!(list.next().is_none());
//! assert!(list.is_terminated());
//! ```

extern crate alloc;

use alloc::vec::Vec;

use rclip_core::{Error, ErrorKind, Result};

/// Decode a `WritePrivateProfileStruct` value and verify its checksum.
///
/// The empty string decodes to no bytes. That is the pragmatic reading rather
/// than the strict one — strictly, the shortest legal value is the two hex
/// digits of a checksum over nothing — and it is what real files need: a `.url`
/// written by Explorer carries a bare `IDList=` with nothing after it far more
/// often than it carries a PIDL.
///
/// # Errors
///
/// [`ErrorKind::BadLength`] for an odd number of digits or for a single digit
/// (which cannot even be a checksum), [`ErrorKind::Malformed`] for a non-hex
/// character or for a checksum that does not match. `offset` is echoed into
/// every one of them.
pub fn decode(v: &str, offset: usize) -> Result<Vec<u8>> {
    let mut bytes = decode_unchecked(v, offset)?;
    // The last decoded byte is the checksum. An empty value has none, and
    // decodes to nothing.
    let Some(sum) = bytes.pop() else {
        return Ok(bytes);
    };
    if checksum(&bytes) != sum {
        return Err(Error::new(ErrorKind::Malformed, offset));
    }
    Ok(bytes)
}

/// The same, without verifying the checksum.
///
/// The trailing checksum byte is still removed — it is part of the encoding,
/// not part of the value. Use this when a file is known to have been written by
/// something that got the checksum wrong and the PIDL is wanted anyway; prefer
/// [`decode`] otherwise, because a wrong checksum usually means the value was
/// truncated somewhere and a truncated PIDL parses into confident nonsense.
///
/// # Errors
///
/// [`ErrorKind::BadLength`] and [`ErrorKind::Malformed`] as for [`decode`],
/// minus the checksum test.
pub fn decode_no_checksum(v: &str, offset: usize) -> Result<Vec<u8>> {
    let mut bytes = decode_unchecked(v, offset)?;
    bytes.pop();
    Ok(bytes)
}

/// Decode the hex digits, checksum byte included, without verifying it.
fn decode_unchecked(v: &str, offset: usize) -> Result<Vec<u8>> {
    let b = v.as_bytes();
    if b.is_empty() {
        return Ok(Vec::new());
    }
    if b.len() % 2 != 0 || b.len() < 2 {
        return Err(Error::new(ErrorKind::BadLength, offset));
    }
    // The length is halved before it sizes the allocation, and it is bounded by
    // the value's own length in a file already in memory, so there is nothing
    // here for a length field to lie about.
    let mut out = Vec::with_capacity(b.len() / 2);
    for pair in b.chunks_exact(2) {
        let hi = hex_digit(pair[0]).ok_or(Error::new(ErrorKind::Malformed, offset))?;
        let lo = hex_digit(pair[1]).ok_or(Error::new(ErrorKind::Malformed, offset))?;
        out.push(hi << 4 | lo);
    }
    Ok(out)
}

/// The `WritePrivateProfileStruct` checksum: the sum of the data bytes, low
/// eight bits.
#[must_use]
pub fn checksum(data: &[u8]) -> u8 {
    data.iter().fold(0u8, |acc, b| acc.wrapping_add(*b))
}

/// Encode bytes the way `WritePrivateProfileStruct` does: uppercase hex, then
/// the checksum byte.
///
/// Round-trips with [`decode`]. Present because the corpus fixtures for this
/// key have to be built somehow, and because a `.url` writer will want it in
/// Phase 4.
#[must_use]
pub fn encode(data: &[u8]) -> alloc::string::String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = alloc::string::String::with_capacity(data.len() * 2 + 2);
    let mut push = |b: u8| {
        out.push(char::from(HEX[usize::from(b >> 4)]));
        out.push(char::from(HEX[usize::from(b & 0x0F)]));
    };
    for &b in data {
        push(b);
    }
    push(checksum(data));
    out
}

/// Wine accepts `a`–`z` and `A`–`Z` here and gives `g`..`z` nonsense values.
/// This accepts hex digits only: a `g` in the middle of a PIDL is a corrupt
/// file, and decoding it to `0x10` produces bytes nobody wrote.
const fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
