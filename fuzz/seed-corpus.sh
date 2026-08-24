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
    # The writer's round trip starts from a parsed document, so a real RTF
    # fixture is the seed -- prefixed below so the `Document` arm is taken.
    rtf_write_round_trip)   echo "" ;;
    # The four HTML entry points share the crate's fixtures. `browser-fragment`
    # in particular is the shape that matters: a `<style>` block, pretty-printed
    # indentation and inline `style=` attributes, i.e. what a browser actually
    # puts on a clipboard.
    html_tokenize)          echo "rclip-html" ;;
    html_runs)              echo "rclip-html" ;;
    html_document)          echo "rclip-html" ;;
    html_entity)            echo "rclip-html" ;;
    html_css)               echo "" ;;   # built below: bare declaration blocks
    # A .lnk embeds a LinkTargetIDList, so the idlist fixtures are useful
    # fragments to splice from even though none of them is a whole link.
    shell_link_parse)       echo "rclip-shell-link rclip-idlist" ;;
    file_desc_group)        echo "rclip-file-desc" ;;
    file_desc_descriptor)   echo "" ;;   # built below, see the note there
    webloc_parse)           echo "rclip-webloc" ;;
    webloc_bplist)          echo "rclip-webloc" ;;
    webloc_xml)             echo "rclip-webloc" ;;
    webloc_rsrc)            echo "rclip-webloc" ;;
    url_file_parse)         echo "rclip-url-file" ;;
    desktop_entry_parse)    echo "rclip-desktop-entry" ;;
    bookmark_parse)         echo "rclip-bookmark" ;;
    # `facade_decode` takes a structure rather than bytes; see the section at
    # the bottom of this file, which wraps every fixture in a payload header.
    facade_decode)          echo "" ;;
    *)                      echo "" ;;
  esac
}

# Which target a captured fixture seeds, keyed by the sidecar's "format" field.
# Captured fixtures live in corpus/<platform>/<app>/ and so do not carry the
# crate name in their path; the sidecar is the only thing that says what they
# are. Unknown formats are skipped rather than guessed at.
targets_for_format() {
  case "$1" in
    CF_HTML|HTML\ Format)                       echo "cf_html_parse html_tokenize html_runs html_document" ;;
    text/html|public.html|HTML)                 echo "html_tokenize html_runs html_document html_entity" ;;
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
                                                echo "webloc_parse webloc_bplist webloc_xml webloc_rsrc" ;;
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

# The same again for the ANSI descriptor, which is 332 bytes rather than 592
# and so is gated on a length the wide parser rejects: no input reaches both,
# and without a seed of the right size the ANSI half of the crate is never
# parsed at all. `two-descriptors-ansi.bin` is the group fixture to cut from.
for f in "$synthetic/rclip-file-desc"/*ansi*.bin; do
  [ -e "$f" ] || continue
  size=$(wc -c < "$f" | tr -d ' ')
  if [ "$size" -ge 336 ]; then
    dd if="$f" of="$desc_dest/seed-first-descriptor-ansi-$(basename "$f")" \
       bs=1 skip=4 count=332 status=none
    copied=$((copied + 1))
  fi
done
: | dd of="$desc_dest/seed-zero-ansi.bin" bs=332 count=1 if=/dev/zero status=none
copied=$((copied + 1))


# ---------------------------------------------------------------------------
# Generated seeds
#
# Three kinds of seed cannot be copied out of ../corpus:
#
#   * the two malformed shapes that a mutator will essentially never invent but
#     that a specific fix depends on,
#   * declaration blocks and reference soup for the entry points that take a
#     fragment of a document rather than a document,
#   * and the structure-aware targets, whose input is not a byte string at all.
#
# All of them are written here rather than committed, for the same reason the
# copies above are: ../corpus stays the single source of truth for anything that
# is a real fixture, and everything else is derived on the way in.
# ---------------------------------------------------------------------------

# Write stdin into one or more target corpora under the same name.
seed_into() {
  name="$1"
  shift
  tmp="$(mktemp)"
  cat > "$tmp"
  for t in "$@"; do
    mkdir -p "$here/corpus/$t"
    cp -f "$tmp" "$here/corpus/$t/$name"
    copied=$((copied + 1))
  done
  rm -f "$tmp"
}

html_all="html_tokenize html_runs html_document"

# The two recursion hazards `rclip-html`'s author found and removed during
# review. `Attrs::next` recursed once per `=` on `<a =====...>`, and
# `Tokenizer::next` recursed once per repetition on `</></>...`; both are loops
# now. A fuzzer that never generates the shape proves nothing about the fix, and
# a mutator starting from ordinary markup will not stumble onto 400 consecutive
# `=` signs, so the shapes are seeded deliberately at a length that would have
# overflowed the old code.
{
  printf '<a '
  i=0
  while [ $i -lt 400 ]; do printf '='; i=$((i + 1)); done
  printf '>text</a>'
} | seed_into seed-recursion-attr-equals.bin $html_all

{
  i=0
  while [ $i -lt 400 ]; do printf '</>'; i=$((i + 1)); done
  printf 'text'
} | seed_into seed-recursion-empty-end-tags.bin $html_all

# The same two shapes' near neighbours, which exercise the loops rather than the
# recursion: a slash run is the other way into `Attrs::next`'s "not a name"
# branch, and an empty raw-text element is the other `continue` in
# `Tokenizer::next`.
{
  printf '<a '
  i=0
  while [ $i -lt 400 ]; do printf '/'; i=$((i + 1)); done
  printf '>text'
} | seed_into seed-recursion-attr-slashes.bin $html_all

{
  i=0
  while [ $i -lt 200 ]; do printf '<style></style>'; i=$((i + 1)); done
  printf 'text'
} | seed_into seed-recursion-empty-raw-text.bin $html_all

# Malformed nesting at scale: the element stack's search-and-reconstruct walk,
# which is the part that must not loop, and the implied-close rule, without
# which a document that omits its end tags hits the depth limit on its 64th
# item.
{
  i=0
  while [ $i -lt 100 ]; do printf '<b><i>x</b>y</i>'; i=$((i + 1)); done
} | seed_into seed-mismatched-nesting.bin $html_all
{
  i=0
  while [ $i -lt 200 ]; do printf '<li>item'; i=$((i + 1)); done
} | seed_into seed-implied-close.bin $html_all

# Character references, every rule at once: the longest-match-without-semicolon
# case (`&notin` is `&notin;`, not `&not;` + `in`), the Windows-1252 remap of
# `0x80..=0x9F` in both spellings, the values that must become U+FFFD, and the
# shapes that are not references at all.
printf '%s' 'a&amp;b &notin &not; &notit; &#150; &#x96; &#129; &#0; &#xD800; &#x110000; &#99999999999999; &# &&; &lt &lt; &LT; &#38#38; &nbsp&nbsp; x&y' |
  seed_into seed-entities.bin html_entity $html_all

# `style=` declaration blocks, fed to `html_css` bare. The quoting, the
# parentheses and the entity-inside-the-splitter cases are all here, because
# each is a separate branch of `position_at_top_level` and none of them is
# reachable from a mutation of the others.
printf '%s' "color:#f00;font-weight:700;font-style:italic;text-decoration:underline line-through;font-family:'Foo; Bar';background:rgb(1 2 3 / 0.5);font-size:1.5em" |
  seed_into seed-declarations.bin html_css
printf '%s' 'font-family:&quot;Foo Bar&quot;;color:rgba(0,0,0,0);background-color:transparent;font-size:xx-large;font-size:120%;font-size:-3pt' |
  seed_into seed-declarations-entities.bin html_css
printf '%s' '#abc;#aabbcc;#abcd;#aabbccdd;rgb(300,-1,0);rgba(1,2,3);+4;-2;7;larger;smaller;12q;3ex;0px' |
  seed_into seed-values.bin html_css

# ------------------------------------------------------- rtf_write_round_trip
#
# The input is a two-variant enum whose first byte selects the arm: `b % 3 == 0`
# takes the `Document` arm and hands the rest of the buffer to
# `Document::parse`, anything else builds styled runs. Every control field is a
# single `int_in_range` over a range narrower than 256, so it costs exactly one
# byte and the headers below are hand-computable -- which is the whole reason
# the impl avoids `arbitrary_len`, which would have read its length from the
# *end* of the buffer instead.
mkdir -p "$here/corpus/rtf_write_round_trip"
for f in "$synthetic/rclip-rtf"/*.bin; do
  [ -e "$f" ] || continue
  {
    printf '%b' '\0000'
    cat "$f"
  } > "$here/corpus/rtf_write_round_trip/seed-document-$(basename "$f")"
  copied=$((copied + 1))
done
# One run of "hello", every toggle on, no explicit size, no font, no colours:
# 01 (Runs arm) 00 (one run) 05 (five bytes of text) "hello" 0F (flags) 00 00.
printf '%b' '\0001\0000\0005hello\0017\0000\0000' \
  > "$here/corpus/rtf_write_round_trip/seed-runs-hello.bin"
copied=$((copied + 1))
# The same, with text that exercises every branch of the escaper: a backslash,
# both braces, a tab, a CRLF and a non-ASCII character.
printf '%b' '\0001\0000\0014a\\\\{b}\011c\015\012\303\251' \
  > "$here/corpus/rtf_write_round_trip/seed-runs-escapes.bin"
copied=$((copied + 1))

# ------------------------------------------------------------- facade_decode
#
# The facade takes a `ClipboardPayload`, not bytes, so a fixture is only a seed
# once it is wrapped in one. The header for a single-item payload is five bytes
# -- platform, item count, identifier-source selector, identifier index, item
# index -- and the last item takes the whole rest of the buffer, so a fixture
# with five bytes in front of it *is* a pasteboard offering that fixture under
# the right identifier. Without this the mutator would have to discover the
# identifier table by brute force and would essentially never reach a decoder.
#
# The identifier index is looked up in the target's own table rather than
# hard-coded, so the two cannot drift apart.
native_index() {
  awk -v want="$1" '
    # `n` explicitly, because an uninitialised awk variable prints as the empty
    # string and index 0 -- the first identifier in the table -- would silently
    # look like "not found".
    BEGIN           { n = 0 }
    /^const NATIVE/ { on = 1; next }
    on && /^\];/    { exit }
    on && match($0, /"[^"]*"/) {
      s = substr($0, RSTART + 1, RLENGTH - 2)
      if (s == want) { print n; exit }
      n++
    }
  ' "$here/fuzz_targets/facade_decode.rs"
}

facade_dest="$here/corpus/facade_decode"
mkdir -p "$facade_dest"

# facade_seed <platform 0=win 1=mac 2=unix> <identifier> <src.bin> <name>
facade_seed() {
  idx="$(native_index "$2")"
  if [ -z "$idx" ]; then
    echo "seed-corpus: facade identifier not in the target's table: $2" >&2
    return 0
  fi
  {
    printf '%b' "$(printf '\\0%03o' "$1" 0 0 "$idx" 0)"
    cat "$3"
  } > "$facade_dest/$4"
  copied=$((copied + 1))
}

# crate directory => platform and the identifier that reaches its decoder.
# Only the formats the facade actually dispatches on are here: a `.lnk`, a
# `.webloc`, a `.url`, a `.desktop` and a bookmark all decode through
# `Link::from_file` / `Shortcut::from_lnk` rather than through a flavor, so no
# clipboard identifier reaches them and wrapping them would seed noise.
facade_wrap() {
  case "$1" in
    rclip-cf-html)    echo "0 HTML Format" ;;
    rclip-html)       echo "1 public.html" ;;
    rclip-rtf)        echo "1 public.rtf" ;;
    rclip-dib)        echo "0 CF_DIBV5" ;;
    rclip-dropfiles)  echo "0 CF_HDROP" ;;
    rclip-uri-list)   echo "2 text/uri-list" ;;
    rclip-idlist)     echo "0 Shell IDList Array" ;;
    rclip-file-desc)  echo "0 FileGroupDescriptorW" ;;
    *)                echo "" ;;
  esac
}

for dir in rclip-cf-html rclip-html rclip-rtf rclip-dib rclip-dropfiles \
           rclip-uri-list rclip-idlist rclip-file-desc; do
  wrap="$(facade_wrap "$dir")"
  [ -n "$wrap" ] || continue
  plat="${wrap%% *}"
  native="${wrap#* }"
  for f in "$synthetic/$dir"/*.bin; do
    [ -e "$f" ] || continue
    facade_seed "$plat" "$native" "$f" "seed-$dir-$(basename "$f")"
  done
done

# The same DIB and uri-list fixtures under their other spellings, because the
# platform changes the decoder and not just the name: `Flavor::FileList` is
# `CF_HDROP` on Windows, one `public.file-url` per pasteboard item on macOS and
# `text/uri-list` on Unix -- three parsers behind one flavor.
for f in "$synthetic/rclip-uri-list"/*.bin; do
  [ -e "$f" ] || continue
  facade_seed 2 "x-special/gnome-copied-files" "$f" "seed-gnome-$(basename "$f")"
  facade_seed 1 "public.file-url" "$f" "seed-fileurl-$(basename "$f")"
done
for f in "$synthetic/rclip-dib"/*.bin; do
  [ -e "$f" ] || continue
  facade_seed 0 "CF_DIB" "$f" "seed-dib-v1-$(basename "$f")"
done
for f in "$synthetic/rclip-html"/*.bin; do
  [ -e "$f" ] || continue
  facade_seed 2 "text/html" "$f" "seed-unix-html-$(basename "$f")"
done
# `CFSTR_FILEDESCRIPTORA` under its own name. `Flavor::from_windows_name` maps
# `"FileGroupDescriptor"` and `"FileGroupDescriptorW"` to the same flavor and
# the facade then always runs the *wide* parser, so this seed drives a payload
# whose 332-byte stride is being read at 592. `rclip-file-desc` is explicit
# that the two are told apart by the format name and never by sniffing, which
# is the half of that contract the facade currently drops.
for f in "$synthetic/rclip-file-desc"/*ansi*.bin; do
  [ -e "$f" ] || continue
  facade_seed 0 "FileGroupDescriptor" "$f" "seed-file-desc-ansi-$(basename "$f")"
done

# Plain text in each vocabulary. `Flavor::PlainText` is UTF-16LE on Windows and
# UTF-8 everywhere else, which is a different decoder rather than a different
# label, and it is the flavor every real payload carries.
utf16="$(mktemp)"
printf '%b' 'h\0000e\0000l\0000l\0000o\0000' > "$utf16"
facade_seed 0 "CF_UNICODETEXT" "$utf16" "seed-plain-utf16.bin"
utf8="$(mktemp)"
printf 'hello world' > "$utf8"
facade_seed 1 "public.utf8-plain-text" "$utf8" "seed-plain-utf8.bin"
facade_seed 2 "text/plain;charset=utf-8" "$utf8" "seed-plain-mime.bin"
rm -f "$utf16" "$utf8"

# A multi-item payload, which is the shape phase 5 added and which no
# single-item seed reaches: two representations in two different pasteboard
# items, so `item_count`, `group` and `all` all have something to do. The
# two-item header adds a two-byte big-endian length for every item but the
# last, which is why this one is written out by hand rather than derived.
#
#   01        platform = macOS
#   01        1 + 1 % 6 = 2 items
#   00 <i>    identifier from the table: public.html
#   00        pasteboard item 0
#   00 0c     12 bytes of payload
#   ...       the payload
#   00 <i>    identifier from the table: public.file-url
#   01        pasteboard item 1
#   ...       the rest of the buffer
html_idx="$(native_index 'public.html')"
url_idx="$(native_index 'public.file-url')"
if [ -n "$html_idx" ] && [ -n "$url_idx" ]; then
  {
    printf '%b' "$(printf '\\0%03o' 1 1 0 "$html_idx" 0 0 12)"
    printf '<b>bold</b>x'
    printf '%b' "$(printf '\\0%03o' 0 "$url_idx" 1)"
    printf 'file:///tmp/a%%20b.txt'
  } > "$facade_dest/seed-two-items.bin"
  copied=$((copied + 1))
fi

echo "seeded $copied file(s) across $(echo "$targets" | wc -w | tr -d ' ') targets"
