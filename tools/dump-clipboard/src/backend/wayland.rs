//! Wayland: `ext-data-control-v1`, falling back to `wlr-data-control-v1`.
//!
//! **Not verified by running** — written from the two protocol XMLs and
//! compile-checked for `x86_64-unknown-linux-gnu`. No compositor has seen it.
//!
//! `wl_data_device` is not usable here, and the reason is structural rather
//! than a matter of taste: a client only receives `wl_data_device.selection`
//! while it holds *keyboard focus*, and keyboard focus belongs to a surface.
//! A CLI has no surface, will never be focused, and would therefore be handed
//! an empty clipboard forever. That is the whole point of the data-control
//! protocols — they exist so that clipboard managers, which also have no
//! surface, can read the selection.
//!
//! Two of them exist. `zwlr_data_control_manager_v1` came from wlroots and is
//! what most compositors have shipped for years; `ext_data_control_manager_v1`
//! is its standardised successor, identical in shape. This tries `ext` first
//! and falls back, because a compositor that has both is telling you which one
//! it would rather you used.
//!
//! **Neither is implemented by GNOME's Mutter.** There is no polite way around
//! that: on GNOME this backend reports what is missing and stops, rather than
//! pretending an empty clipboard. Capturing under GNOME means either a GTK
//! helper with a real surface or reading the Xwayland side with
//! `RCLIP_BACKEND=x11`, which captures Xwayland's *translation* of the
//! selection rather than the original.
//!
//! Transfer itself is a pipe per MIME type: hand the source a write end, close
//! our copy of it, read the read end to EOF. Two ordering rules follow from
//! that, and getting either wrong hangs the tool:
//!
//! * The fd must still be open when the connection is flushed, because
//!   wayland-rs sends it with the queued request rather than duplicating it.
//! * Our write end must be closed *before* the read, or EOF never arrives —
//!   the pipe would still have a writer, namely us.

use std::collections::HashMap;
use std::fs::File;
use std::io::Read as _;
use std::os::fd::{AsFd as _, OwnedFd};
use std::time::{Duration, Instant};

use rustix::event::{poll, PollFd, PollFlags, Timespec};
use wayland_client::backend::ObjectId;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{
    delegate_noop, event_created_child, Connection, Dispatch, Proxy, QueueHandle,
};

use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1::{self, ExtDataControlDeviceV1},
    ext_data_control_manager_v1::ExtDataControlManagerV1,
    ext_data_control_offer_v1::{self, ExtDataControlOfferV1},
};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1::{self, ZwlrDataControlDeviceV1},
    zwlr_data_control_manager_v1::ZwlrDataControlManagerV1,
    zwlr_data_control_offer_v1::{self, ZwlrDataControlOfferV1},
};

use crate::capture::{bail, Body, Capture, Offered, Result, Selection};

/// Per-MIME-type deadline. The source client renders on demand and may be
/// doing real work — a browser re-serialising a selection as HTML — so this is
/// deliberately generous.
const TIMEOUT: Duration = Duration::from_secs(5);

/// Ceiling on one transfer. The far end is another process writing into a pipe
/// for as long as it likes; a capture tool should stop rather than fill the
/// disk.
const MAX_BYTES: usize = 256 * 1024 * 1024;

/// One offer, whichever protocol produced it.
///
/// The two protocols are the same protocol twice, so everything above this
/// enum is written once.
#[derive(Debug, Clone)]
enum Offer {
    Ext(ExtDataControlOfferV1),
    Wlr(ZwlrDataControlOfferV1),
}

impl Offer {
    fn id(&self) -> ObjectId {
        match self {
            Self::Ext(o) => o.id(),
            Self::Wlr(o) => o.id(),
        }
    }

    fn receive(&self, mime_type: String, fd: std::os::fd::BorrowedFd<'_>) {
        match self {
            Self::Ext(o) => o.receive(mime_type, fd),
            Self::Wlr(o) => o.receive(mime_type, fd),
        }
    }
}

#[derive(Debug, Default)]
struct State {
    /// MIME types per offer. Keyed by object id because the `offer` events
    /// that describe an offer arrive on the offer object itself, after the
    /// `data_offer` event that created it and before the `selection` event
    /// that says which offer is current.
    mimes: HashMap<ObjectId, Vec<String>>,
    selection: Option<Offer>,
    primary: Option<Offer>,
}

pub fn capture(selection: Selection) -> Result<Capture> {
    let conn = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn)?;
    let qh = queue.handle();

    // Version 1 is enough: the data-control managers only need the seat's
    // identity, and binding low keeps this working on every compositor.
    let seat: WlSeat = globals
        .bind(&qh, 1..=1, ())
        .map_err(|e| format!("no usable wl_seat: {e}"))?;

    let mut state = State::default();
    let protocol;

    if let Ok(manager) = globals.bind::<ExtDataControlManagerV1, _, _>(&qh, 1..=1, ()) {
        protocol = "ext-data-control-v1";
        let _device = manager.get_data_device(&seat, &qh, ());
        roundtrip(&mut queue, &mut state)?;
    } else if let Ok(manager) = globals.bind::<ZwlrDataControlManagerV1, _, _>(&qh, 1..=2, ()) {
        // `primary_selection` only exists from version 2 of the wlroots
        // protocol; on a version-1 compositor `--primary` finds nothing, which
        // is reported below as an empty selection rather than as an error.
        protocol = "wlr-data-control-v1";
        let _device = manager.get_data_device(&seat, &qh, ());
        roundtrip(&mut queue, &mut state)?;
    } else {
        bail!(
            "this compositor offers neither ext_data_control_manager_v1 nor \
             zwlr_data_control_manager_v1, so a client with no surface cannot read the \
             clipboard. GNOME/Mutter is the common case. RCLIP_BACKEND=x11 will read \
             Xwayland's bridged copy instead, which is a translation rather than the \
             original."
        );
    }

    let source = format!(
        "the Wayland {} selection via {protocol}",
        selection.as_str()
    );

    let offer = match selection {
        Selection::Clipboard => state.selection.clone(),
        Selection::Primary => state.primary.clone(),
    };
    let Some(offer) = offer else {
        return Ok(Capture {
            platform: rclip_core::Platform::Unix,
            source,
            offered: Vec::new(),
        });
    };

    // A source is free to advertise the same MIME type twice; reading it twice
    // would be two pipes for one payload and two files that differ only in a
    // disambiguating suffix.
    let mut seen = Vec::new();
    for mime in state.mimes.get(&offer.id()).into_iter().flatten() {
        if !seen.contains(mime) {
            seen.push(mime.clone());
        }
    }

    let mut offered = Vec::new();
    for mime in seen {
        // Xwayland bridges X11 selection targets through as MIME types, so the
        // side-effecting ICCCM targets can turn up here too, with the same
        // consequences if they are requested.
        if let Some(why) = super::side_effecting(&mime) {
            offered.push(Offered {
                native: mime,
                item: None,
                body: Body::Skipped(format!("{why} (bridged from X11 by Xwayland)")),
                detail: None,
            });
            continue;
        }

        let (read_end, write_end) = rustix::pipe::pipe()?;
        offer.receive(mime.clone(), write_end.as_fd());
        // Flush while our write end is still open — wayland-rs sends the fd
        // with the request rather than duplicating it — and only then drop it,
        // because a pipe with a live writer never reaches EOF.
        conn.flush()?;
        drop(write_end);

        let body = match read_to_eof(read_end) {
            Ok(bytes) => Body::Bytes(bytes),
            Err(e) => Body::Skipped(e.to_string()),
        };
        offered.push(Offered {
            native: mime,
            item: None,
            body,
            detail: None,
        });
    }

    Ok(Capture {
        platform: rclip_core::Platform::Unix,
        source,
        offered,
    })
}

/// Two round trips: the first delivers the globals' answer to
/// `get_data_device`, the second is insurance for a compositor that batches
/// the `data_offer` / `offer` / `selection` burst differently.
fn roundtrip(queue: &mut wayland_client::EventQueue<State>, state: &mut State) -> Result<()> {
    queue.roundtrip(state)?;
    queue.roundtrip(state)?;
    Ok(())
}

/// Read one transfer to EOF, with a deadline.
fn read_to_eof(fd: OwnedFd) -> Result<Vec<u8>> {
    let deadline = Instant::now() + TIMEOUT;
    let mut file = File::from(fd);
    let mut out = Vec::new();
    let mut buf = [0u8; 64 * 1024];

    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            bail!(
                "timed out after {} bytes; the source client stopped writing",
                out.len()
            );
        }
        let timeout = Timespec {
            tv_sec: left.as_secs() as _,
            tv_nsec: left.subsec_nanos() as _,
        };
        let mut fds = [PollFd::new(&file, PollFlags::IN)];
        if poll(&mut fds, Some(&timeout))? == 0 {
            bail!(
                "timed out after {} bytes; the source client stopped writing",
                out.len()
            );
        }

        match file.read(&mut buf) {
            Ok(0) => return Ok(out),
            Ok(n) => {
                if out.len() + n > MAX_BYTES {
                    bail!("transfer exceeded {MAX_BYTES} bytes; refusing to keep reading");
                }
                out.extend_from_slice(&buf[..n]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(Box::new(e)),
        }
    }
}

// -- protocol plumbing --------------------------------------------------------

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // `registry_queue_init` keeps its own record of the globals; this
        // tool binds once at startup and never reacts to a later change.
    }
}

delegate_noop!(State: ignore WlSeat);
delegate_noop!(State: ExtDataControlManagerV1);
delegate_noop!(State: ZwlrDataControlManagerV1);

impl Dispatch<ExtDataControlDeviceV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtDataControlDeviceV1,
        event: ext_data_control_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_data_control_device_v1::Event::DataOffer { id } => {
                state.mimes.entry(id.id()).or_default();
            }
            ext_data_control_device_v1::Event::Selection { id } => {
                state.selection = id.map(Offer::Ext);
            }
            ext_data_control_device_v1::Event::PrimarySelection { id } => {
                state.primary = id.map(Offer::Ext);
            }
            // `finished` means the compositor has invalidated the device;
            // whatever was already collected is still what was on offer.
            _ => {}
        }
    }

    // The `data_offer` event carries a `new_id`, so wayland-rs has to be told
    // what user data to give the object it creates on our behalf.
    event_created_child!(State, ExtDataControlDeviceV1, [
        ext_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ExtDataControlOfferV1, ()),
    ]);
}

impl Dispatch<ExtDataControlOfferV1, ()> for State {
    fn event(
        state: &mut Self,
        offer: &ExtDataControlOfferV1,
        event: ext_data_control_offer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_data_control_offer_v1::Event::Offer { mime_type } = event {
            state.mimes.entry(offer.id()).or_default().push(mime_type);
        }
    }
}

impl Dispatch<ZwlrDataControlDeviceV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrDataControlDeviceV1,
        event: zwlr_data_control_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_data_control_device_v1::Event::DataOffer { id } => {
                state.mimes.entry(id.id()).or_default();
            }
            zwlr_data_control_device_v1::Event::Selection { id } => {
                state.selection = id.map(Offer::Wlr);
            }
            zwlr_data_control_device_v1::Event::PrimarySelection { id } => {
                state.primary = id.map(Offer::Wlr);
            }
            _ => {}
        }
    }

    event_created_child!(State, ZwlrDataControlDeviceV1, [
        zwlr_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ZwlrDataControlOfferV1, ()),
    ]);
}

impl Dispatch<ZwlrDataControlOfferV1, ()> for State {
    fn event(
        state: &mut Self,
        offer: &ZwlrDataControlOfferV1,
        event: zwlr_data_control_offer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_data_control_offer_v1::Event::Offer { mime_type } = event {
            state.mimes.entry(offer.id()).or_default().push(mime_type);
        }
    }
}
