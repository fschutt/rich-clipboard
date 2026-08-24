//! The element stack: tokens in, styled runs out.
//!
//! Three rules do almost all of the work here, and two of them exist because
//! clipboard HTML is not well-formed HTML.
//!
//! 1. **An end tag closes the nearest matching open element, or nothing.**
//!    `<b><i></b></i>` is not an error and not a special case — it is what
//!    contenteditable, mail clients and Word's HTML export produce all day. The
//!    stack is searched from the top for a match; if one is found everything
//!    above it closes too, and if none is found the end tag is dropped. Neither
//!    branch can loop and neither can panic.
//! 2. **Nesting is bounded** at [`rclip_core::MAX_DEPTH`] with a fixed-size
//!    array and a loop. There is no recursion in this crate at all, so
//!    `<div><div><div>...` returns [`ErrorKind::DepthLimit`] rather than
//!    overflowing the stack.
//! 3. **Line breaks are emitted lazily.** A block boundary sets a flag; the
//!    flag becomes a [`RunText::Break`] only when the next text arrives. That
//!    one mechanism removes leading breaks, collapses `</p><p>` into one, and
//!    drops trailing breaks, none of which a browser shows either.

use rclip_core::{Error, ErrorKind, Result, MAX_DEPTH};

use crate::css::{self, ColorValue};
use crate::element::{self, Formatting};
use crate::style::Style;
use crate::text::{HtmlText, Whitespace};
use crate::token::{Tag, Token, Tokenizer};

/// Depth 0 is "outside any element", so the stack needs one slot more than the
/// number of elements we allow to be open.
const STACK: usize = MAX_DEPTH as usize + 1;

/// One piece of text together with the formatting in effect for it.
///
/// A run is *not* maximal: an element boundary cuts one in half even when the
/// formatting is identical on both sides, because merging needs somewhere to
/// put the joined text and this API does not allocate. [`crate::Document`]
/// (feature `alloc`) does the merge.
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Run<'a> {
    /// The content.
    pub text: RunText<'a>,
    /// The formatting in effect.
    pub style: Style<'a>,
    /// Byte offset in the input where this run starts.
    pub offset: usize,
}

/// The content of a [`Run`].
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum RunText<'a> {
    /// Character data. Decode it with [`HtmlText::chars`].
    Text(HtmlText<'a>),
    /// A line break: `<br>`, or the boundary of a block element.
    Break,
    /// A cell boundary, `<td>` to `<td>`. A tab rather than a break because a
    /// table pasted one cell per line reads worse than the original.
    Tab,
}

/// One open element.
///
/// The tag is kept, not just the name, because reconstructing a formatting
/// element after an enclosing element closed on top of it means recomputing its
/// style against a *different* parent — which needs its attributes again.
#[derive(Debug, Copy, Clone)]
struct Frame<'a> {
    tag: Tag<'a>,
    style: Style<'a>,
    /// Inside `<style>`, `<script>` or another element whose text is not text.
    dropped: bool,
    /// Inside `<pre>`.
    preserve: bool,
}

impl<'a> Frame<'a> {
    const ROOT: Frame<'static> = Frame {
        tag: Tag {
            name: "",
            attrs: &[],
            self_closing: false,
            offset: 0,
        },
        style: Style::DEFAULT,
        dropped: false,
        preserve: false,
    };

    /// The frame `tag` opens inside `parent`.
    fn nested(tag: &Tag<'a>, parent: &Frame<'a>) -> Self {
        Self {
            tag: *tag,
            style: style_for(tag, parent.style),
            dropped: parent.dropped || element::drops_content(tag.name),
            preserve: parent.preserve || element::preserves_whitespace(tag.name),
        }
    }
}

/// Pull parser over the styled text of an HTML fragment.
///
/// Yields one [`Run`] at a time and never allocates. The only error it can
/// produce is [`ErrorKind::DepthLimit`], and an `Err` is terminal.
///
/// ```
/// use rclip_html::{Runs, RunText};
///
/// let mut bold = String::new();
/// for run in Runs::new(b"<p>plain <b>bold</b></p>") {
///     let run = run.unwrap();
///     if let RunText::Text(t) = run.text {
///         if run.style.bold {
///             bold.extend(t.chars());
///         }
///     }
/// }
/// assert_eq!(bold, "bold");
/// ```
#[derive(Debug, Clone)]
pub struct Runs<'a> {
    tok: Tokenizer<'a>,
    stack: [Frame<'a>; STACK],
    /// Number of open elements. `stack[depth]` is the current state.
    depth: usize,
    /// Text held back so a break can be emitted in front of it.
    queued: Option<(HtmlText<'a>, usize)>,
    /// Line breaks owed before the next text.
    breaks: u8,
    /// A cell boundary owed before the next text.
    tab: bool,
    /// Whether any text has been emitted yet. Breaks before the first character
    /// are not breaks, they are indentation.
    started: bool,
    /// Whether a leading run of whitespace in the next text run should vanish.
    at_boundary: bool,
    /// Whether the last thing emitted already ended a line.
    after_newline: bool,
    done: bool,
}

impl<'a> Runs<'a> {
    /// Start parsing a fragment.
    ///
    /// There is no signature to check and nothing to fail on: any byte string
    /// is *some* HTML, which is the whole reason a clipboard can hand you one.
    #[must_use]
    pub fn new(input: &'a [u8]) -> Self {
        Self {
            tok: Tokenizer::new(input),
            stack: [Frame::ROOT; STACK],
            depth: 0,
            queued: None,
            breaks: 0,
            tab: false,
            started: false,
            at_boundary: true,
            after_newline: true,
            done: false,
        }
    }

    fn cur(&self) -> &Frame<'a> {
        // `depth` is bounded by the DepthLimit check in `open`, so this index
        // is never input-derived.
        &self.stack[self.depth]
    }

    fn fail(&mut self, kind: ErrorKind, at: usize) -> Option<Result<Run<'a>>> {
        self.done = true;
        Some(Err(Error::new(kind, at)))
    }

    /// Note a line break at a block boundary. Two is the cap: `<br><br>` is a
    /// blank line and `<br><br><br><br>` is still a blank line.
    fn line_break(&mut self) {
        self.breaks = self.breaks.saturating_add(1).min(2);
        // A newline supersedes a cell boundary: `</td></tr>` is a row end.
        self.tab = false;
        // Whitespace after a break is indentation. This has to be set here
        // rather than when the break is emitted, because the text token is
        // lexed — and its leading whitespace decided — before the break in
        // front of it comes out.
        self.at_boundary = true;
    }

    /// Note a block boundary, which is worth at most one break.
    fn block_break(&mut self) {
        // `<pre>a\n</pre><p>b</p>` is two lines, not three: the newline the
        // `<pre>` preserved has already ended the line that `</pre>` would.
        if !self.after_newline {
            self.breaks = self.breaks.max(1);
        }
        self.tab = false;
        self.at_boundary = true;
    }

    /// Note a cell boundary, which a pending line break outranks: the first
    /// cell of a row goes at the start of the line, not one tab into it.
    fn cell_break(&mut self) {
        if self.breaks == 0 {
            self.tab = true;
        }
    }

    fn run(&self, text: RunText<'a>, offset: usize) -> Run<'a> {
        Run {
            text,
            style: self.cur().style,
            offset,
        }
    }

    /// Push an element.
    fn open(&mut self, tag: &Tag<'a>) -> Option<Result<Run<'a>>> {
        if self.depth >= MAX_DEPTH as usize {
            return self.fail(ErrorKind::DepthLimit, tag.offset);
        }
        let frame = Frame::nested(tag, self.cur());
        self.depth += 1;
        self.stack[self.depth] = frame;
        None
    }

    /// Close what this start tag implicitly ends.
    ///
    /// `<p>a<p>b` is two paragraphs and not a paragraph inside a paragraph, and
    /// the same goes for `<li>`, `<td>` and `<tr>`. Without this, a document
    /// that omits its end tags — which is most hand-written HTML and a fair
    /// amount of generated HTML — nests one level deeper per item and hits
    /// [`ErrorKind::DepthLimit`] on its 64th list item.
    ///
    /// This is *not* the HTML5 "generate implied end tags" algorithm, which
    /// needs the full element scope machinery. It is the four cases that
    /// actually appear, each bounded by the elements it must not reach across.
    fn implied_close(&mut self, name: &str) {
        const SCOPE: [&str; 13] = [
            "table",
            "td",
            "th",
            "ul",
            "ol",
            "dl",
            "body",
            "blockquote",
            "div",
            "section",
            "article",
            "form",
            "fieldset",
        ];
        if name.eq_ignore_ascii_case("p") {
            self.close_nearest(&["p"], &SCOPE);
        } else if name.eq_ignore_ascii_case("li") {
            // A list item's own list is not a barrier to it.
            self.close_nearest(&["li"], &["table", "td", "th", "body"]);
        } else if element::is_cell(name) {
            self.close_nearest(&["td", "th"], &["table"]);
        } else if name.eq_ignore_ascii_case("tr") {
            self.close_nearest(&["td", "th"], &["table"]);
            self.close_nearest(&["tr"], &["table"]);
        }
    }

    /// Pop to and including the nearest frame named in `names`, giving up at
    /// the first frame named in `barriers`.
    fn close_nearest(&mut self, names: &[&str], barriers: &[&str]) -> bool {
        let mut i = self.depth;
        while i > 0 {
            let open = self.stack[i].tag.name;
            if names.iter().any(|n| open.eq_ignore_ascii_case(n)) {
                self.depth = i - 1;
                return true;
            }
            if barriers.iter().any(|n| open.eq_ignore_ascii_case(n)) {
                return false;
            }
            i -= 1;
        }
        false
    }

    /// Close the nearest open element named `name`, reconstructing the
    /// formatting elements that were open inside it.
    ///
    /// Two rules, and both of them are what a browser does:
    ///
    /// - A `</b>` with **no `<b>` open closes nothing**. The alternative —
    ///   closing the innermost element whatever it is — turns one stray end tag
    ///   into a document where every subsequent style is off by one.
    /// - A `</b>` that closes a `<b>` with an `<i>` still open inside it
    ///   **reopens the `<i>`**, so `<b><i>x</b>y</i>` renders `y` italic. This
    ///   is HTML5's adoption agency algorithm reduced to its one visible
    ///   consequence: the [formatting
    ///   elements](crate::element::is_formatting) come back, `<span>` and
    ///   `<div>` do not, and the styles are recomputed against the new parent
    ///   rather than carried over.
    ///
    /// Returns whether anything was closed. There is no recursion and no
    /// unbounded loop: the walk is over a fixed-size array, once.
    fn close(&mut self, name: &str) -> bool {
        let mut found = None;
        let mut i = self.depth;
        while i > 0 {
            if self.stack[i].tag.name.eq_ignore_ascii_case(name) {
                found = Some(i);
                break;
            }
            i -= 1;
        }
        let Some(at) = found else { return false };

        // Compact the survivors down over the slot being removed. `write` is
        // never ahead of `j`, so every frame is read before its slot is reused.
        let mut write = at;
        for j in (at + 1)..=self.depth {
            let tag = self.stack[j].tag;
            if !element::is_formatting(tag.name) {
                continue;
            }
            let parent = self.stack[write - 1];
            self.stack[write] = Frame::nested(&tag, &parent);
            write += 1;
        }
        self.depth = write - 1;
        true
    }
}

impl<'a> Iterator for Runs<'a> {
    type Item = Result<Run<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            // Owed breaks and tabs come out in front of the text they
            // precede, which is what makes the mechanism lazy: a break with no
            // text after it is never emitted, so a trailing `</p>` costs
            // nothing and neither does the indentation before the first word.
            if let Some((_, at)) = self.queued {
                if !self.started {
                    // Everything before the first character is indentation.
                    self.breaks = 0;
                    self.tab = false;
                }
                if self.breaks > 0 {
                    self.breaks -= 1;
                    self.at_boundary = true;
                    self.after_newline = true;
                    return Some(Ok(self.run(RunText::Break, at)));
                }
                if self.tab {
                    self.tab = false;
                    return Some(Ok(self.run(RunText::Tab, at)));
                }
            }
            if let Some((text, at)) = self.queued.take() {
                self.started = true;
                self.at_boundary = text.next_boundary(self.at_boundary);
                self.after_newline = text.ends_with_newline();
                return Some(Ok(self.run(RunText::Text(text), at)));
            }
            if self.done {
                return None;
            }

            let ws = if self.cur().preserve {
                Whitespace::Preserve
            } else {
                Whitespace::Collapse
            };
            self.tok.set_whitespace(ws, self.at_boundary);

            let Some(token) = self.tok.next() else {
                self.done = true;
                return None;
            };
            let at = self.tok.token_offset();

            match token {
                Token::Comment(_) | Token::Doctype(_) => {}
                Token::Text(text) => {
                    if self.cur().dropped || text.is_empty() {
                        continue;
                    }
                    self.queued = Some((text, at));
                }
                Token::EndTag { name, .. } => {
                    let closed = self.close(name);
                    if closed && element::is_block(name) {
                        self.block_break();
                    } else if closed && element::is_cell(name) {
                        self.cell_break();
                    }
                }
                Token::StartTag(tag) => {
                    self.implied_close(tag.name);
                    if element::is_block(tag.name) {
                        self.block_break();
                    }
                    if tag.is("br") {
                        self.line_break();
                    }
                    if element::is_cell(tag.name) {
                        self.cell_break();
                    }
                    // A void element carries no content, so it never goes on
                    // the stack. `<img>` left open would swallow the rest of
                    // the document into itself.
                    if element::is_void(tag.name) || tag.self_closing {
                        continue;
                    }
                    if let Some(e) = self.open(&tag) {
                        return Some(e);
                    }
                }
            }
        }
    }
}

/// The style an element's children inherit: the parent's, plus what the tag
/// itself says, plus what its `style=` attribute says.
///
/// In that order, because `style=` is more specific than the element and than
/// the presentational attributes — `<font color="red" style="color:blue">` is
/// blue everywhere it is rendered.
fn style_for<'a>(tag: &Tag<'a>, parent: Style<'a>) -> Style<'a> {
    let mut style = parent;
    match element::formatting(tag.name) {
        Some(Formatting::Bold) => style.bold = true,
        Some(Formatting::Italic) => style.italic = true,
        Some(Formatting::Underline) => style.underline = true,
        Some(Formatting::Strike) => style.strike = true,
        None => {}
    }
    if element::is_bold_by_default(tag.name) {
        style.bold = true;
    }
    if tag.is("font") {
        apply_font_element(tag, &mut style);
    }
    if let Some(bg) = tag.attr("bgcolor") {
        if let Some(ColorValue::Rgb(c)) = css::color(bg.as_raw()) {
            style.background = Some(c);
        }
    }
    if let Some(decls) = tag.attr("style") {
        apply_declarations(decls.as_raw(), &mut style);
    }
    style
}

/// The presentational `<font>` attributes, which pre-CSS mail still uses.
fn apply_font_element<'a>(tag: &Tag<'a>, style: &mut Style<'a>) {
    if let Some(face) = tag.attr("face") {
        if let Some(name) = first_family(face.as_raw()) {
            style.font_family = Some(name);
        }
    }
    if let Some(color) = tag.attr("color") {
        if let Some(ColorValue::Rgb(c)) = css::color(color.as_raw()) {
            style.color = Some(c);
        }
    }
    if let Some(size) = tag.attr("size") {
        if let Some(pt) = css::font_attr_size_pt(size.as_raw()) {
            style.size_pt = Some(pt);
        }
    }
}

/// Fold a `style=` declaration block into `style`.
fn apply_declarations<'a>(block: &'a [u8], style: &mut Style<'a>) {
    for decl in css::declarations(block) {
        if decl.is("font-weight") {
            if let Some(b) = css::font_weight_bold(decl.value) {
                style.bold = b;
            }
        } else if decl.is("font-style") {
            if let Some(i) = css::font_style_italic(decl.value) {
                style.italic = i;
            }
        } else if decl.is("text-decoration") || decl.is("text-decoration-line") {
            if let Some(d) = css::text_decoration(decl.value) {
                // Replaces rather than adds: one `text-decoration` declaration
                // is the whole value, so `underline` on a child of a
                // struck-through parent turns the strike off.
                style.underline = d.underline;
                style.strike = d.strike;
            }
        } else if decl.is("color") {
            match css::color(decl.value) {
                Some(ColorValue::Rgb(c)) => style.color = Some(c),
                Some(ColorValue::Transparent) => style.color = None,
                None => {}
            }
        } else if decl.is("background-color") || decl.is("background") {
            match css::color(decl.value) {
                Some(ColorValue::Rgb(c)) => style.background = Some(c),
                Some(ColorValue::Transparent) => style.background = None,
                // A `background` shorthand that is an image or a gradient
                // names no colour this crate can use. Leaving the inherited one
                // alone is the honest answer.
                None => {}
            }
        } else if decl.is("font-size") {
            if let Some(pt) = css::font_size_pt(decl.value, style.size_pt) {
                style.size_pt = Some(pt);
            }
        } else if decl.is("font-family") {
            if let Some(name) = first_family(decl.value) {
                style.font_family = Some(name);
            }
        }
    }
}

/// The first family out of a `font-family` list, quotes stripped.
///
/// The rest of the list is the fallback chain, which is advice to a layout
/// engine that has the font list of *this* machine. Nothing here does, so
/// keeping the first name — the one the author actually wanted — and dropping
/// the rest is the only choice that does not pretend to have resolved anything.
fn first_family(value: &[u8]) -> Option<HtmlText<'_>> {
    let first = value.split(|b| *b == b',').next()?;
    let name = css::unquote(css::trim(first));
    if name.is_empty() {
        return None;
    }
    Some(HtmlText::new(name, Whitespace::Preserve, false))
}
