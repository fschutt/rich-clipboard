#!/usr/bin/env bash
# Populate fuzz/corpus/<target>/ from ../corpus.
#
# Copies, not symlinks. The trade-off the plan calls out is real -- a symlink
# farm keeps one source of truth, a copy can go stale -- so this script is the
# way out of it: the copies are *generated*, never committed (see .gitignore),
# and both `cargo fuzz run` locally and the CI job run this first. ../corpus
# stays the single source of truth, and nothing in fuzz/corpus/ depends on a
# checkout preserving symlinks, which is what breaks on Windows and under
# `git config core.symlinks=false`.
#
# Files that ARE committed under fuzz/corpus/ are the regression seeds: the
# minimised input for every crash a run has found. This script only ever adds,
# so it cannot delete one.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
corpus="$here/../corpus"
synthetic="$corpus/synthetic"

# target => space-separated list of corpus subdirectory names under synthetic/.
# Several targets share a crate's fixtures on purpose: a `.webloc` fixture is a
# seed for the dispatcher *and* for whichever of the two plist readers it uses,
# and starting the other one from it costs nothing.
seed_for() {
  case "$1" in
    cf_html_parse)          echo "rclip-cf-html" ;;
    codepage_decode)        echo "rclip-codepage" ;;
    uri_list_parse)         echo "rclip-uri-list" ;;
    uri_list_copied_files)  echo "rclip-uri-list" ;;
    uri_list_nautilus_text) echo "rclip-uri-list" ;;
    uri_list_kde_cut)       echo "rclip-uri-list" ;;
    dropfiles_parse)        echo "rclip-dropfiles" ;;
    dib_header)             echo "rclip-dib" ;;
    dib_decode)             echo "rclip-dib" ;;
    idlist_itemidlist)      echo "rclip-idlist" ;;
    idlist_cida)            echo "rclip-idlist" ;;
    rtf_parse)              echo "rclip-rtf" ;;
    rtf_tokenize)           echo "rclip-rtf" ;;
    rtf_document)           echo "rclip-rtf" ;;
    # A .lnk embeds a LinkTargetIDList, so the idlist fixtures are useful
    # fragments to splice from even though none of them is a whole link.
    shell_link_parse)       echo "rclip-shell-link rclip-idlist" ;;
    file_desc_group)        echo "rclip-file-desc" ;;
    file_desc_descriptor)   echo "" ;;   # built below, see the note there
    webloc_parse)           echo "rclip-webloc" ;;
    webloc_bplist)          echo "rclip-webloc" ;;
    webloc_xml)             echo "rclip-webloc" ;;
    url_file_parse)         echo "rclip-url-file" ;;
    desktop_entry_parse)    echo "rclip-desktop-entry" ;;
    bookmark_parse)         echo "rclip-bookmark" ;;
    *)                      echo "" ;;
  esac
}

# Which target a captured fixture seeds, keyed by the sidecar's "format" field.
# Captured fixtures live in corpus/<platform>/<app>/ and so do not carry the
# crate name in their path; the sidecar is the only thing that says what they
# are. Unknown formats are skipped rather than guessed at.
targets_for_format() {
  case "$1" in
    CF_HTML|HTML\ Format)                       echo "cf_html_parse" ;;
    text/uri-list)                              echo "uri_list_parse" ;;
    x-special/gnome-copied-files|x-special/mate-copied-files)
                                                echo "uri_list_copied_files" ;;
    x-special/nautilus-clipboard)               echo "uri_list_nautilus_text" ;;
    application/x-kde-cutselection)             echo "uri_list_kde_cut" ;;
    CF_HDROP|DROPFILES)                         echo "dropfiles_parse" ;;
    CF_DIB|CF_DIBV5)                            echo "dib_header dib_decode" ;;
    CFSTR_SHELLIDLIST|CIDA)                     echo "idlist_cida" ;;
    ITEMIDLIST|PIDL)                            echo "idlist_itemidlist" ;;
    RTF|public.rtf|Rich\ Text\ Format|NeXT\ Rich\ Text\ Format\ v1.0\ pasteboard\ type)
                                                echo "rtf_parse rtf_tokenize rtf_document" ;;
    public.file-url)                            echo "uri_list_parse" ;;
    LNK|ShellLink|MS-SHLLINK)                   echo "shell_link_parse" ;;
    CFSTR_FILEDESCRIPTORW|FileGroupDescriptorW) echo "file_desc_group" ;;
    webloc|inetloc|com.apple.web-internet-location)
                                                echo "webloc_parse webloc_bplist webloc_xml" ;;
    url|InternetShortcut)                       echo "url_file_parse" ;;
    desktop|desktop-entry)                      echo "desktop_entry_parse" ;;
    BookmarkData|book|alis)                     echo "bookmark_parse" ;;
    *)                                          echo "" ;;
  esac
}

targets=$(grep -o '^name = "[a-z0-9_]*"' "$here/Cargo.toml" | sed 's/name = "//; s/"//' | grep -v '^rclip.fuzz$')

copied=0
for target in $targets; do
  dest="$here/corpus/$target"
  mkdir -p "$dest"
  for dir in $(seed_for "$target"); do
    src="$synthetic/$dir"
    [ -d "$src" ] || continue
    for f in "$src"/*.bin; do
      [ -e "$f" ] || continue
      cp -f "$f" "$dest/seed-$dir-$(basename "$f")"
      copied=$((copied + 1))
    done
  done
done

# Captured fixtures, if another agent has landed any yet. Absent is normal.
if [ -d "$corpus" ]; then
  while IFS= read -r sidecar; do
    bin="${sidecar%.json}.bin"
    [ -e "$bin" ] || continue
    fmt=$(sed -n 's/.*"format"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$sidecar" | head -1)
    [ -n "$fmt" ] || continue
    for target in $(targets_for_format "$fmt"); do
      dest="$here/corpus/$target"
      mkdir -p "$dest"
      rel=$(echo "${sidecar#"$corpus"/}" | tr '/' '-')
      cp -f "$bin" "$dest/seed-${rel%.json}.bin"
      copied=$((copied + 1))
    done
  done < <(find "$corpus" -mindepth 3 -name '*.json' -not -path "$synthetic/*" 2>/dev/null)
fi

# `FileDescriptor::parse` takes exactly one 592-byte descriptor and rejects
# every other length before reading a field, so a whole CFSTR_FILEDESCRIPTORW
# payload is useless as a seed for it: it is 4 bytes too long. Cut the first
# descriptor out of each group fixture instead, which is precisely the slice a
# caller of this entry point would have.
desc_dest="$here/corpus/file_desc_descriptor"
mkdir -p "$desc_dest"
for f in "$synthetic/rclip-file-desc"/*.bin; do
  [ -e "$f" ] || continue
  size=$(wc -c < "$f" | tr -d ' ')
  if [ "$size" -ge 596 ]; then
    dd if="$f" of="$desc_dest/seed-first-descriptor-$(basename "$f")" \
       bs=1 skip=4 count=592 status=none
    copied=$((copied + 1))
  fi
done
# An all-zero descriptor: the shortest path to "parses, every flag clear".
: | dd of="$desc_dest/seed-zero.bin" bs=592 count=1 if=/dev/zero status=none
copied=$((copied + 1))

echo "seeded $copied file(s) across $(echo "$targets" | wc -w | tr -d ' ') targets"
