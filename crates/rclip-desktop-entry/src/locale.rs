//! Locale postfixes and the spec's lookup order.
//!
//! §5 lets a `localestring` or `iconstring` key carry a postfix —
//! `Name[sr_YU@Latn]` — and defines a four-rung fallback ladder for choosing
//! between them. The ladder is the part implementations skip: both
//! `freedesktop_entry_parser` and `freedesktop-file-parser` do a single exact
//! match and fall straight through to the unpostfixed key, and
//! `freedesktop-desktop-entry` implements two of the four rungs.
//!
//! The full table from §5:
//!
//! | `LC_MESSAGES` | keys tried, in order |
//! |---|---|
//! | `lang_COUNTRY@MODIFIER` | `lang_COUNTRY@MODIFIER`, `lang_COUNTRY`, `lang@MODIFIER`, `lang`, unpostfixed |
//! | `lang_COUNTRY` | `lang_COUNTRY`, `lang`, unpostfixed |
//! | `lang@MODIFIER` | `lang@MODIFIER`, `lang`, unpostfixed |
//! | `lang` | `lang`, unpostfixed |
//!
//! Two consequences that are easy to get backwards, both spelled out in §5:
//!
//! - The candidates are derived from the *requested* locale only. If the
//!   request has no `MODIFIER`, no key with a modifier is ever considered —
//!   asking for `sr` must not silently return `Name[sr@Latn]`.
//! - `.ENCODING` is stripped from both sides before matching, so a request for
//!   `de_DE.UTF-8` matches `Name[de_DE]`.

/// A parsed locale, encoding already discarded.
///
/// Used for both sides of the comparison: the caller's locale and the postfix
/// on a key. Parsing both through the same type is what keeps `de_DE.UTF-8` and
/// `de_DE` from being different locales.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct Locale<'a> {
    lang: &'a str,
    country: Option<&'a str>,
    modifier: Option<&'a str>,
}

impl<'a> Locale<'a> {
    /// Parse `lang_COUNTRY.ENCODING@MODIFIER`, any part after `lang` optional.
    ///
    /// Returns `None` for an empty language, which is the only shape that
    /// cannot be a locale.
    ///
    /// Order matters: `@MODIFIER` is stripped first, then `.ENCODING`, then
    /// `_COUNTRY`. Doing it the other way round makes the `.` in
    /// `sr_YU.UTF-8@Latn` end up inside the country.
    #[must_use]
    pub fn parse(s: &'a str) -> Option<Self> {
        let (head, modifier) = match s.find('@') {
            Some(i) => (s.get(..i)?, Some(s.get(i + 1..)?)),
            None => (s, None),
        };
        // The encoding is dropped, not kept: §5 says matching ignores it.
        let head = match head.find('.') {
            Some(i) => head.get(..i)?,
            None => head,
        };
        let (lang, country) = match head.find('_') {
            Some(i) => (head.get(..i)?, Some(head.get(i + 1..)?)),
            None => (head, None),
        };
        if lang.is_empty() {
            return None;
        }
        Some(Self { lang, country, modifier })
    }

    /// Build a locale from its parts.
    #[must_use]
    pub const fn new(lang: &'a str, country: Option<&'a str>, modifier: Option<&'a str>) -> Self {
        Self { lang, country, modifier }
    }

    /// The language subtag, e.g. `sr`.
    #[must_use]
    pub const fn lang(&self) -> &'a str {
        self.lang
    }

    /// The country subtag, e.g. `YU`.
    #[must_use]
    pub const fn country(&self) -> Option<&'a str> {
        self.country
    }

    /// The modifier, e.g. `Latn`.
    #[must_use]
    pub const fn modifier(&self) -> Option<&'a str> {
        self.modifier
    }

    /// The key postfixes to try, most specific first.
    ///
    /// Ends *before* the unpostfixed key; that fallback is the caller's, since
    /// only the caller knows the key's base name.
    #[must_use]
    pub const fn candidates(&self) -> Candidates<'a> {
        Candidates { locale: *self, step: 0 }
    }

    /// Case-sensitive equality. §3 says "case is significant everywhere in the
    /// file", and POSIX locale names are already canonically cased, so folding
    /// case here would let `Name[DE]` shadow `Name[de]`.
    #[must_use]
    pub fn matches(&self, other: &Locale<'_>) -> bool {
        self.lang == other.lang
            && self.country == other.country
            && self.modifier == other.modifier
    }

    /// `true` if a key postfix string denotes this exact locale.
    #[must_use]
    pub fn matches_postfix(&self, postfix: &str) -> bool {
        Locale::parse(postfix).is_some_and(|p| self.matches(&p))
    }
}

/// Iterator over the fallback ladder of a [`Locale`].
#[derive(Debug, Copy, Clone)]
pub struct Candidates<'a> {
    locale: Locale<'a>,
    step: u8,
}

impl<'a> Iterator for Candidates<'a> {
    type Item = Locale<'a>;

    fn next(&mut self) -> Option<Locale<'a>> {
        // The four rungs, skipping any that the requested locale cannot
        // produce. `lang` alone is always the last and is never skipped.
        loop {
            let step = self.step;
            self.step = self.step.saturating_add(1);
            let l = self.locale;
            match step {
                0 => {
                    if l.country.is_some() && l.modifier.is_some() {
                        return Some(l);
                    }
                }
                1 => {
                    if l.country.is_some() {
                        return Some(Locale::new(l.lang, l.country, None));
                    }
                }
                2 => {
                    if l.modifier.is_some() {
                        return Some(Locale::new(l.lang, None, l.modifier));
                    }
                }
                3 => return Some(Locale::new(l.lang, None, None)),
                _ => return None,
            }
        }
    }
}
