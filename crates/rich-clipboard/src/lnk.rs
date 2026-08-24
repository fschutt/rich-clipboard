//! `.lnk`, which is a file and not a clipboard flavor.

use crate::error::{Error, Result};
use crate::item::Shortcut;

impl Shortcut {
    /// Parse a `.lnk`.
    ///
    /// Not reachable through [`decode`](crate::decode), and deliberately so:
    /// [`Flavor::ShellLink`](rclip_core::Flavor::ShellLink) exists in the
    /// registry but has no identifier on any platform, because Windows has no
    /// registered clipboard format for a shell link. A `.lnk` reaches an
    /// application as a *file* — its path in a `CF_HDROP`, or its bytes as
    /// `CFSTR_FILECONTENTS` behind a `FILEGROUPDESCRIPTORW` — so the conversion
    /// is a direct one.
    ///
    /// # This resolves nothing
    ///
    /// CVE-2010-2568 (Stuxnet) and CVE-2017-8464 were both bugs in the
    /// *acting*, not in the parsing: the shell loaded the icon a parsed `.lnk`
    /// named, from a path the `.lnk` chose, at display time, with no user
    /// action at all. Everything below comes back as data with nothing checked.
    ///
    /// # What is lost
    ///
    /// [`Shortcut`] keeps the parts a consumer can act on. The header's
    /// `LinkFlags`, `ShowCommand`, hot key, file attributes and three
    /// `FILETIME`s, the `LinkInfo` volume serial and network share, the shell
    /// items' individual structure, and every `ExtraData` block except the
    /// environment path are all dropped. Parse with `rclip_shell_link::ShellLink`
    /// directly when any of that matters.
    ///
    /// # Errors
    ///
    /// [`Error::Codec`] for a bad `HeaderSize` or `LinkCLSID`, or for a length
    /// field that does not agree with the buffer. Unknown flag bits, an
    /// unrecognised `ShowCommand`, an unknown `ExtraData` signature and an
    /// unparseable shell item are all kept rather than rejected — refusing a
    /// link over a byte Microsoft has not documented is a self-inflicted
    /// compatibility break.
    pub fn from_lnk(bytes: &[u8]) -> Result<Self> {
        let link =
            rclip_shell_link::ShellLink::parse(bytes).map_err(|e| Error::codec("Shell Link", e))?;
        let s = |v: Option<rclip_shell_link::ShellStr<'_>>| v.map(|v| v.to_string_lossy());
        Ok(Self {
            target_path: link
                .link_info
                .as_ref()
                .and_then(|info| info.local_base_path().ok().flatten())
                .map(|p| p.to_string_lossy()),
            display_path: link
                .target_id_list
                .as_ref()
                .map(|t| rclip_idlist::display_path(t.items(), "\\"))
                .filter(|s| !s.is_empty()),
            name: s(link.string_data.name),
            relative_path: s(link.string_data.relative_path),
            working_dir: s(link.string_data.working_dir),
            arguments: s(link.string_data.arguments),
            icon_location: s(link.string_data.icon_location),
            environment_path: s(link.environment_path()),
        })
    }

    /// Serialize as a `.lnk`.
    ///
    /// The way to hand one to the OS is as a promised file: publish a
    /// [`RichItem::PromisedFiles`](crate::RichItem::PromisedFiles) naming
    /// `something.lnk`, and answer the `CFSTR_FILECONTENTS` request with these
    /// bytes. That last step is transport, which is why it is not in this
    /// workspace.
    ///
    /// # What is lost
    ///
    /// [`Shortcut::display_path`] is a *label* built from an `ITEMIDLIST` and
    /// cannot be turned back into one. That matters: the target IDList is a
    /// shell link's authoritative target, and the shell binds a PIDL by handing
    /// the bytes back to the namespace extension that owns them — a
    /// reconstructed one would resolve to something unintended or not at all.
    /// A link written here therefore carries only the `LinkInfo` path, the
    /// strings, and the environment-variable block, which is what the shell
    /// falls back to and is enough for a link that opens the right file.
    ///
    /// # Errors
    ///
    /// [`Error::Codec`] if a string exceeds the 260-character cap MS-SHLLINK
    /// revision 10.0 puts on every `StringData` field except the arguments.
    pub fn to_lnk(&self) -> Result<alloc::vec::Vec<u8>> {
        let mut b = rclip_shell_link::ShellLinkBuilder::new();
        if let Some(p) = self.target_path.as_deref() {
            b = b.local_path(p);
        }
        if let Some(s) = self.name.as_deref() {
            b = b.name(s);
        }
        if let Some(s) = self.relative_path.as_deref() {
            b = b.relative_path(s);
        }
        if let Some(s) = self.working_dir.as_deref() {
            b = b.working_dir(s);
        }
        if let Some(s) = self.arguments.as_deref() {
            b = b.arguments(s);
        }
        if let Some(s) = self.icon_location.as_deref() {
            b = b.icon_location(s);
        }
        if let Some(s) = self.environment_path.as_deref() {
            b = b.environment_path(s);
        }
        b.build().map_err(|e| Error::codec("Shell Link", e))
    }
}
