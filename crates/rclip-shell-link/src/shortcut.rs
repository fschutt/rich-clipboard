//! Where a `.lnk` points, in the shortcut family's vocabulary.
//!
//! `.url`, `.webloc`, `.desktop` (`Type=Link`), `text/uri-list` and `.lnk` are
//! five encodings of one idea, and `plan/PLAN.md` §4.10 puts them behind one
//! [`ShortcutTarget`]. A shell link is the odd member: the other four state
//! their destination as *one string*, while a `.lnk` states it up to four
//! times, in four different structures, in two different encodings, and calls
//! the authoritative one an `ITEMIDLIST` — which is a binary shell namespace
//! path and not text at all.
//!
//! So this module does not pretend a `.lnk` has one target. It enumerates the
//! destination *strings* the file carries, in the order a resolver would fall
//! back through them, and offers the first one that can be borrowed as a `&str`
//! as a [`ShortcutTarget`].
//!
//! # This resolves nothing
//!
//! Every warning in the crate docs applies with full force here. A
//! [`ShortcutTarget::Path`] is a string that *looks* like a path, chosen by
//! whoever wrote the link. Nothing here binds an IDList, expands
//! `%USERPROFILE%`, joins a relative path against anything, or touches a
//! filesystem.

use rclip_idlist::ShellStr;

/// The family's shared vocabulary, re-exported so that
/// `rclip_shell_link::shortcut::ShortcutTarget` names the same type as
/// `rclip_url_file::shortcut::ShortcutTarget` and the helpers are reachable
/// from the same place in every member of the family.
pub use rclip_core::shortcut::{looks_like_path, scheme, ShortcutTarget};

use crate::{extra, ExtraDataBlock, ShellLink};

/// Which structure a [`TargetCandidate`] was read out of.
///
/// Worth knowing, because the four are not equally trustworthy: a
/// [`Self::LocalBasePath`] is absolute and machine-specific, a
/// [`Self::RelativePath`] means nothing without knowing where the `.lnk` itself
/// lives, and an [`Self::EnvironmentPath`] is only a path after an expansion
/// this crate will not perform.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TargetSource {
    /// `LinkInfo`'s `LocalBasePath` (MS-SHLLINK 2.3) — the drive-letter path,
    /// e.g. `C:\Users\me\notes.txt`.
    LocalBasePath,
    /// `CommonNetworkRelativeLink`'s `NetName` (MS-SHLLINK 2.3.2) — the UNC
    /// share, e.g. `\\fileserver\public`.
    NetName,
    /// The `EnvironmentVariableDataBlock` path (MS-SHLLINK 2.5.4), e.g.
    /// `%windir%\system32\cmd.exe`.
    EnvironmentPath,
    /// `StringData`'s `RELATIVE_PATH` (MS-SHLLINK 2.4), e.g. `.\notes.txt`.
    RelativePath,
}

/// One destination string a shell link carries, and where it came from.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct TargetCandidate<'a> {
    /// Which structure it was read out of.
    pub source: TargetSource,
    /// The string, in whichever encoding that structure used.
    pub text: ShellStr<'a>,
}

impl<'a> TargetCandidate<'a> {
    /// Classify this candidate, when its bytes can be borrowed as a `&str`.
    ///
    /// `None` for a UTF-16 field and for an ANSI field holding a byte the
    /// system code page decides — see [`ShellStr::as_ascii`]. Both cases need
    /// re-encoding, which needs an allocation, which is not this layer's job.
    #[must_use]
    pub fn target(&self) -> Option<ShortcutTarget<'a>> {
        self.text.as_ascii().map(ShortcutTarget::classify)
    }
}

/// The destination strings a shell link carries, most absolute first.
///
/// See [`ShellLink::target_candidates`].
#[derive(Debug, Clone)]
pub struct TargetCandidates<'a> {
    items: [Option<TargetCandidate<'a>>; 4],
    index: usize,
}

impl<'a> Iterator for TargetCandidates<'a> {
    type Item = TargetCandidate<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(slot) = self.items.get(self.index) {
            self.index += 1;
            if let Some(c) = *slot {
                return Some(c);
            }
        }
        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.items[self.index.min(self.items.len())..]
            .iter()
            .filter(|s| s.is_some())
            .count();
        (n, Some(n))
    }
}

impl core::iter::FusedIterator for TargetCandidates<'_> {}

impl<'a> ShellLink<'a> {
    /// Every destination string this link carries, most absolute first.
    ///
    /// The order is `LocalBasePath`, `NetName`, `EnvironmentPath`,
    /// `RelativePath`: a resolver that cannot bind the target IDList works down
    /// exactly this list, from the form that means the most on another machine
    /// to the form that means the least. A link routinely carries several, and
    /// they routinely disagree — a `.lnk` copied between machines keeps a
    /// `LocalBasePath` that no longer exists — so the whole list is offered
    /// rather than a guess.
    ///
    /// # The IDList is not in here
    ///
    /// [`ShellLink::target_id_list`] is the *authoritative* target and is
    /// deliberately absent: it is a chain of shell items, not text, and several
    /// of them (a control-panel applet, a search folder, a namespace extension)
    /// name something that has no path at all. Walk it with
    /// [`LinkTargetIdList::items`](crate::LinkTargetIdList::items) when you want
    /// it.
    ///
    /// # A `LinkInfo` that will not parse yields nothing
    ///
    /// `LocalBasePath` and `NetName` sit behind offsets that can be wrong, and
    /// this returns candidates rather than a `Result`, so a `LinkInfo` whose
    /// offsets do not resolve contributes no candidate instead of an error.
    /// That is a lossy answer to a structural question; ask
    /// [`ShellLink::link_info`] directly when the difference matters.
    ///
    /// `LocalBasePath` is also skipped when `CommonPathSuffix` is non-empty:
    /// MS-SHLLINK 2.3 builds the full path by concatenating the two, and this
    /// crate cannot concatenate without allocating. Returning the base alone
    /// would be a confidently wrong path, which is worse than none.
    #[must_use]
    pub fn target_candidates(&self) -> TargetCandidates<'a> {
        let mut items = [None; 4];

        if let Some(info) = &self.link_info {
            // `?`-free on purpose: see the doc comment. A structurally broken
            // LinkInfo contributes nothing rather than poisoning the list.
            if let (Ok(Some(path)), Ok(suffix)) =
                (info.local_base_path(), info.common_path_suffix())
            {
                if suffix.is_empty() {
                    items[0] = Some(TargetCandidate {
                        source: TargetSource::LocalBasePath,
                        text: path,
                    });
                }
            }
            if let Ok(Some(net)) = info.common_network_relative_link() {
                if let Ok(name) = net.net_name() {
                    items[1] = Some(TargetCandidate {
                        source: TargetSource::NetName,
                        text: name,
                    });
                }
            }
        }

        if let Some(ExtraDataBlock::EnvironmentVariable(p)) =
            self.find_extra(extra::SIG_ENVIRONMENT_VARIABLE)
        {
            items[2] = Some(TargetCandidate {
                source: TargetSource::EnvironmentPath,
                text: p.path(),
            });
        }

        if let Some(rel) = self.string_data.relative_path {
            items[3] = Some(TargetCandidate {
                source: TargetSource::RelativePath,
                text: rel,
            });
        }

        TargetCandidates { items, index: 0 }
    }

    /// Where this link points, in the shortcut family's shared vocabulary.
    ///
    /// The first of [`ShellLink::target_candidates`] whose bytes can be
    /// borrowed as a `&str`, classified by [`ShortcutTarget::classify`].
    ///
    /// # When this is `None`
    ///
    /// Three different situations, which [`ShellLink::target_candidates`]
    /// distinguishes and this does not:
    ///
    /// - the link names its target only as an IDList, which is not text;
    /// - every candidate is UTF-16, which cannot be borrowed as a `&str`
    ///   without re-encoding, and re-encoding allocates;
    /// - every candidate is ANSI containing a byte above `0x7F`, whose meaning
    ///   depends on a code page the file does not record.
    ///
    /// The second is the common one. Windows writes `StringData` in UTF-16
    /// whenever `LinkFlags::IS_UNICODE` is set, which is every link written
    /// this century, so a caller that wants those strings wants
    /// `ShellStr::to_string_lossy` behind the `alloc` feature and its own
    /// classification.
    #[must_use]
    pub fn target(&self) -> Option<ShortcutTarget<'a>> {
        self.target_candidates().find_map(|c| c.target())
    }
}
