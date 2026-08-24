//! X11: ICCCM selection transfer, including INCR.
//!
//! **Not verified by running** — written from ICCCM 2.0 §2.6–2.7 and
//! compile-checked for `x86_64-unknown-linux-gnu`. No X server has seen it.
//!
//! There is no "clipboard" in X11. There is a *selection*, owned by a client,
//! and reading it is a conversation:
//!
//! 1. `ConvertSelection` on `TARGETS` asks the owner what it can convert to.
//!    The answer arrives as a property on *our* window, so we need a window,
//!    even though nothing is ever drawn in it.
//! 2. `SelectionNotify` says the property is ready (or that `property` is
//!    `None`, meaning the owner declined).
//! 3. `GetProperty` reads it. This is where it stops being simple.
//!
//! **INCR is mandatory, not optional.** A property cannot exceed the server's
//! maximum request size, so anything over roughly 256 KB — and a screenshot
//! always is — comes back as a property of type `INCR` whose value is only a
//! *lower bound on the size*. The real data then arrives one property at a
//! time: the requestor deletes the property to say "ready", the owner replaces
//! it and the server sends `PropertyNotify(NewValue)`, and a zero-length
//! property ends the transfer. Skipping this does not truncate large payloads,
//! it drops them entirely and hands back four bytes of length.
//!
//! Two smaller traps:
//!
//! * `GetProperty` with `delete = true` only actually deletes when
//!   `bytes_after` reaches zero, so a partial read must keep going to the end
//!   or the INCR handshake stalls forever.
//! * Some targets are *requests*, not data. Converting `DELETE` destroys the
//!   selection's contents, and `SAVE_TARGETS` asks the clipboard manager to
//!   take ownership. A tool whose whole job is "convert every target" would
//!   otherwise delete the thing it was asked to inspect.

use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, Property, WindowClass,
};
use x11rb::protocol::Event;
use x11rb::{COPY_DEPTH_FROM_PARENT, COPY_FROM_PARENT, CURRENT_TIME, NONE};

use crate::capture::{bail, Body, Capture, Offered, Result, Selection};

use super::side_effecting;

/// How long to wait for one target. Generous: the owner may be a browser that
/// has to re-render a selection before it can answer.
const TIMEOUT: Duration = Duration::from_secs(5);

/// Words (not bytes) per `GetProperty`. 256 KB per round trip, comfortably
/// under any server's maximum reply size.
const CHUNK_WORDS: u32 = 65536;

pub fn capture(selection: Selection) -> Result<Capture> {
    let (conn, screen_num) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen_num].root;

    // An unmapped 1x1 window, purely as somewhere for the owner to put
    // properties. PROPERTY_CHANGE is what makes INCR work: without it the
    // PropertyNotify events that drive the handshake never arrive.
    let window = conn.generate_id()?;
    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        window,
        root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_OUTPUT,
        COPY_FROM_PARENT,
        &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
    )?
    .check()?;
    let _window_guard = WindowGuard(&conn, window);

    let sel_atom = match selection {
        Selection::Clipboard => intern(&conn, b"CLIPBOARD")?,
        Selection::Primary => AtomEnum::PRIMARY.into(),
    };
    let property = intern(&conn, b"RCLIP_DUMP")?;
    let incr = intern(&conn, b"INCR")?;
    let targets = intern(&conn, b"TARGETS")?;

    let display = std::env::var("DISPLAY").unwrap_or_else(|_| "?".to_owned());
    let source = format!(
        "the X11 {} selection on {display}",
        match selection {
            Selection::Clipboard => "CLIPBOARD",
            Selection::Primary => "PRIMARY",
        }
    );

    if conn.get_selection_owner(sel_atom)?.reply()?.owner == NONE {
        return Ok(Capture {
            platform: rclip_core::Platform::Unix,
            source,
            offered: Vec::new(),
        });
    }

    let Some(list) = convert(&conn, window, sel_atom, targets, property, incr)? else {
        bail!("the selection owner refused to convert TARGETS");
    };

    let mut offered = Vec::new();
    for atom in atoms(&list.bytes) {
        if atom == NONE {
            continue;
        }
        let native = String::from_utf8_lossy(&conn.get_atom_name(atom)?.reply()?.name).into_owned();

        if let Some(why) = side_effecting(&native) {
            offered.push(Offered {
                native,
                item: None,
                body: Body::Skipped(why.to_owned()),
                detail: None,
            });
            continue;
        }

        let (body, detail) = match convert(&conn, window, sel_atom, atom, property, incr)? {
            Some(value) => {
                let type_name = if value.type_ == NONE {
                    "None".to_owned()
                } else {
                    String::from_utf8_lossy(&conn.get_atom_name(value.type_)?.reply()?.name)
                        .into_owned()
                };
                let detail = format!(
                    "X11 property type {type_name}, format {}{}.",
                    value.format,
                    if value.incr {
                        ", transferred via INCR"
                    } else {
                        ""
                    }
                );
                (Body::Bytes(value.bytes), Some(detail))
            }
            None => (
                Body::Skipped("the owner declined to convert this target".to_owned()),
                None,
            ),
        };
        offered.push(Offered {
            native,
            item: None,
            body,
            detail,
        });
    }

    Ok(Capture {
        platform: rclip_core::Platform::Unix,
        source,
        offered,
    })
}

/// A property's contents, with the two things about it that the bytes alone
/// do not say.
struct PropValue {
    type_: Atom,
    /// 8, 16 or 32. A 32-bit property is four bytes per element on the wire,
    /// which is what gets dumped — an atom list is a list of `u32`s.
    format: u8,
    incr: bool,
    bytes: Vec<u8>,
}

/// Ask the owner to convert one target, and read the answer.
///
/// `Ok(None)` means the owner declined, which is a normal answer and not an
/// error: a target can be advertised in `TARGETS` and still fail to convert.
fn convert<C: Connection>(
    conn: &C,
    window: u32,
    selection: Atom,
    target: Atom,
    property: Atom,
    incr: Atom,
) -> Result<Option<PropValue>> {
    // Start from a clean slate: a leftover value from a previous target would
    // be indistinguishable from this one's answer.
    conn.delete_property(window, property)?.check()?;
    conn.convert_selection(window, selection, target, property, CURRENT_TIME)?
        .check()?;

    let deadline = Instant::now() + TIMEOUT;
    let notified = wait(conn, deadline, |event| match event {
        Event::SelectionNotify(e)
            if e.requestor == window && e.selection == selection && e.target == target =>
        {
            Some(e.property)
        }
        _ => None,
    })?;

    match notified {
        None => bail!("timed out waiting for SelectionNotify"),
        Some(NONE) => return Ok(None),
        Some(_) => {}
    }

    // Peek with a zero-length read: this returns the type, the format and
    // `bytes_after` (the whole length, since nothing was consumed) without
    // moving any data, and without deleting anything.
    let peek = conn
        .get_property(false, window, property, AtomEnum::ANY, 0, 0)?
        .reply()?;
    if peek.type_ == NONE {
        return Ok(None);
    }

    if peek.type_ == incr {
        return Ok(Some(PropValue {
            type_: incr,
            format: peek.format,
            incr: true,
            bytes: read_incr(conn, window, property, deadline)?,
        }));
    }

    let bytes = read_all(conn, window, property, false)?;
    conn.delete_property(window, property)?.check()?;
    Ok(Some(PropValue {
        type_: peek.type_,
        format: peek.format,
        incr: false,
        bytes,
    }))
}

/// Read one property to the end, in `CHUNK_WORDS` steps.
///
/// With `delete = true` the server only removes the property once
/// `bytes_after` hits zero, which is exactly why this must run to completion:
/// stopping early leaves the property in place and, mid-INCR, leaves the owner
/// waiting for a delete that never comes.
fn read_all<C: Connection>(conn: &C, window: u32, property: Atom, delete: bool) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut offset = 0u32;
    loop {
        let reply = conn
            .get_property(delete, window, property, AtomEnum::ANY, offset, CHUNK_WORDS)?
            .reply()?;
        if reply.type_ == NONE {
            break;
        }
        let bytes_after = reply.bytes_after;
        // `long_offset` counts 32-bit words, not bytes, whatever the format is.
        offset += u32::try_from(reply.value.len().div_ceil(4)).unwrap_or(u32::MAX);
        out.extend_from_slice(&reply.value);
        if bytes_after == 0 || reply.value.is_empty() {
            break;
        }
    }
    Ok(out)
}

/// The INCR handshake (ICCCM §2.7.2).
///
/// The `INCR` property has already been seen; deleting it is the signal that
/// starts the transfer. From there each `PropertyNotify(NewValue)` announces
/// one chunk, which is read *and deleted* to ask for the next, until a
/// zero-length chunk ends it.
fn read_incr<C: Connection>(
    conn: &C,
    window: u32,
    property: Atom,
    deadline: Instant,
) -> Result<Vec<u8>> {
    conn.delete_property(window, property)?.check()?;

    let mut out = Vec::new();
    loop {
        let arrived = wait(conn, deadline, |event| match event {
            Event::PropertyNotify(e)
                if e.window == window && e.atom == property && e.state == Property::NEW_VALUE =>
            {
                Some(())
            }
            _ => None,
        })?;
        if arrived.is_none() {
            bail!(
                "timed out mid-INCR after {} bytes; the selection owner stopped answering",
                out.len()
            );
        }

        let chunk = read_all(conn, window, property, true)?;
        if chunk.is_empty() {
            // A zero-length property is the end-of-transfer marker.
            return Ok(out);
        }
        out.extend_from_slice(&chunk);
        conn.flush()?;
    }
}

/// Poll for an event matching `f` until `deadline`.
///
/// X11 has no per-request timeout and a selection owner is another process
/// that is free to be wedged, hung or gone. `wait_for_event` would block this
/// tool forever in that case, so the answer is a deadline and a short sleep.
fn wait<C: Connection, T>(
    conn: &C,
    deadline: Instant,
    mut f: impl FnMut(&Event) -> Option<T>,
) -> Result<Option<T>> {
    loop {
        while let Some(event) = conn.poll_for_event()? {
            if let Some(value) = f(&event) {
                return Ok(Some(value));
            }
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn intern<C: Connection>(conn: &C, name: &[u8]) -> Result<Atom> {
    Ok(conn.intern_atom(false, name)?.reply()?.atom)
}

/// A `TARGETS` value is a list of 32-bit atoms.
///
/// x11rb negotiates the native byte order in the connection setup, so the four
/// bytes of each element are already in host order.
fn atoms(bytes: &[u8]) -> impl Iterator<Item = Atom> + '_ {
    bytes
        .chunks_exact(4)
        .map(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
}

/// Tears down the scratch window on every exit path.
struct WindowGuard<'c, C: Connection>(&'c C, u32);

impl<C: Connection> Drop for WindowGuard<'_, C> {
    fn drop(&mut self) {
        let _ = self.0.destroy_window(self.1);
        let _ = self.0.flush();
    }
}
