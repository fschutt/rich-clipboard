//! Windows shell links (`.lnk`, MS-SHLLINK) — `rclip_shell_link::ShellLink::parse`.
//!
//! The format is a chain of variable-length sections each gated on a flag bit,
//! with `LinkInfo` carrying five independent offsets into its own body. The
//! plan names those offsets as attacker-controlled indices; this is where they
//! get hammered.
//!
//! Coverage note: the header is a fixed 76 bytes with a `HeaderSize == 0x4C`
//! and a fixed CLSID at offset 4, i.e. 20 exact bytes a mutator has to
//! reproduce before anything past `ShellLinkHeader::parse` is reachable. The
//! corpus seeds carry them, and libFuzzer's value profile does learn the
//! comparison, but a structure-aware target that emitted a valid header and
//! mutated only the body would explore the sections far faster. Noted rather
//! than done: the no-panic property below is the one that matters most and it
//! holds for the naive target too.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rclip_shell_link::{ExtraDataBlock, ShellLink, ShellLinkBuilder, ShellLinkHeader};

fuzz_target!(|data: &[u8]| {
    // The header parser is its own entry point and is reachable on far more
    // inputs than the whole-file parser, so drive it separately.
    let header_only = ShellLinkHeader::parse(data);
    let Ok(link) = ShellLink::parse(data) else {
        return;
    };
    let header = header_only.expect("full parse succeeded where the header parse failed");
    assert_eq!(header.link_flags, link.header.link_flags);

    let _ = (
        link.header.file_attributes,
        link.header.creation_time,
        link.header.access_time,
        link.header.write_time,
        link.header.file_size,
        link.header.icon_index,
        link.header.show_command,
        link.header.hot_key,
    );

    if let Some(t) = &link.target_id_list {
        assert_eq!(t.wire_size(), 2 + t.as_bytes().len());
        for item in t.items() {
            let Ok(item) = item else { break };
            let _ = item.parse().display_name().map(|n| n.to_string_lossy());
        }
    }

    if let Some(info) = &link.link_info {
        let _ = info.local_base_path().map(|p| p.map(|s| s.to_string_lossy()));
        let _ = info.common_path_suffix().map(|s| s.to_string_lossy());
        if let Ok(Some(v)) = info.volume_id() {
            let _ = v.volume_label().map(|s| s.to_string_lossy());
        }
        if let Ok(Some(n)) = info.common_network_relative_link() {
            let _ = n.net_name().map(|s| s.to_string_lossy());
            let _ = n.device_name().map(|s| s.map(|d| d.to_string_lossy()));
        }
    }

    let sd = link.string_data;
    for s in [
        sd.name,
        sd.relative_path,
        sd.working_dir,
        sd.arguments,
        sd.icon_location,
    ]
    .into_iter()
    .flatten()
    {
        let _ = s.to_string_lossy();
        let _ = s.as_ascii();
    }

    // The ExtraData chain is a second length-prefixed walk, independent of
    // everything above it.
    let mut blocks = 0usize;
    for block in link.extra_data() {
        let Ok(block) = block else { break };
        blocks += 1;
        assert!(blocks <= data.len(), "extra-data chain outran the buffer");
        match block {
            ExtraDataBlock::EnvironmentVariable(p)
            | ExtraDataBlock::Darwin(p)
            | ExtraDataBlock::IconEnvironment(p) => {
                let _ = p.path().to_string_lossy();
            }
            _ => {}
        }
        let _ = block.signature();
    }
    let _ = link.environment_path().map(|p| p.to_string_lossy());

    // Round trip through the writer. Deliberately weak, and deliberately
    // present: `ShellLinkBuilder` builds from fields rather than re-emitting a
    // parsed file, so `serialize(parse(x)) == x` is not a property this crate
    // has. What is a property is that anything the reader hands out is
    // something the writer can take back — a string the parser produced must
    // survive being written and read again, or the two halves disagree about
    // the on-the-wire encoding.
    let ascii = |s: Option<rclip_shell_link::ShellStr<'_>>| -> Option<String> {
        s.and_then(|v| v.as_ascii()).map(str::to_owned)
    };
    let (name, rel, wd, args, icon) = (
        ascii(sd.name),
        ascii(sd.relative_path),
        ascii(sd.working_dir),
        ascii(sd.arguments),
        ascii(sd.icon_location),
    );
    let mut b = ShellLinkBuilder::new();
    if let Some(v) = &name {
        b = b.name(v);
    }
    if let Some(v) = &rel {
        b = b.relative_path(v);
    }
    if let Some(v) = &wd {
        b = b.working_dir(v);
    }
    if let Some(v) = &args {
        b = b.arguments(v);
    }
    if let Some(v) = &icon {
        b = b.icon_location(v);
    }
    let Ok(built) = b.build() else { return };
    let back = ShellLink::parse(&built).expect("our own output must parse");
    let back_ascii = |s: Option<rclip_shell_link::ShellStr<'_>>| -> Option<String> {
        s.map(|v| v.to_string_lossy())
    };
    assert_eq!(back_ascii(back.string_data.name), name);
    assert_eq!(back_ascii(back.string_data.relative_path), rel);
    assert_eq!(back_ascii(back.string_data.working_dir), wd);
    assert_eq!(back_ascii(back.string_data.arguments), args);
    assert_eq!(back_ascii(back.string_data.icon_location), icon);
});
