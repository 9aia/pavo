//! [`KeySpec`] — the shared keyboard-binding spec grammar.
//!
//! One grammar across hosts (TUI chords, web `keydown` combos, CLI
//! flag hints): a spec is either
//!
//! - a **named key** — `enter`, `esc`, `tab`, `backspace`, `delete`,
//!   `insert`, `home`, `end`, `pageup`, `pagedown`, `up`, `down`,
//!   `left`, `right`, `space`, `plus`, `f1`–`f12` (case-insensitive,
//!   normalized lowercase);
//! - a **modified chord** — `<mod>+<mod>…+<key>` where mods are
//!   `ctrl`/`control`, `alt`, `shift`, `cmd`/`command`/`super`/`meta`
//!   (normalized to `ctrl`/`alt`/`shift`/`cmd`) and `<key>` is a
//!   single char (case preserved — `K` ≠ `k`) or a named key;
//! - a **key sequence** — two or more bare chars with no `+` or
//!   whitespace (`"dd"`, `"gq"`) — the TUI pending-chord idiom.
//!
//! Whitespace and empty specs are rejected; `+` segments must be
//! non-empty with known modifiers. [`KeySpec::parse`] returns the
//! *normalized* spelling (canonical modifier names/order, lowercase
//! named keys) so hosts compare specs by string equality.
//!
//! Host note: a sequence like `"dd"` is ambiguous with named keys by
//! design — multi-char alphabetic specs that aren't known key names
//! are sequences. Hosts that can't produce modifier chords should
//! treat `ctrl+…` specs as unbindable, not as sequences.
//!
//! This is the *validation/normalization* half of keybinding; the
//! *matching* half (crossterm `KeyCode`, DOM `KeyboardEvent`, …) is
//! host-side. `pavo-tui`'s legacy `parse_key_spec` predates this
//! module and still works — hosts may migrate to [`KeySpec`] when
//! they adopt modifier chords or the shared keymap file.

use std::fmt;
use std::str::FromStr;

/// Modifier prefixes recognized in `+`-chords, canonical name →
/// accepted spellings.
const MODIFIER_SPELLINGS: &[(&str, &[&str])] = &[
    ("ctrl", &["ctrl", "control"]),
    ("alt", &["alt", "option"]),
    ("shift", &["shift"]),
    ("cmd", &["cmd", "command", "super", "meta", "win"]),
];

/// Canonical modifier order for normalized output.
const MODIFIER_ORDER: &[&str] = &["ctrl", "alt", "shift", "cmd"];

/// Named (non-character) keys, lowercase canonical spellings.
const NAMED_KEYS: &[&str] = &[
    "enter",
    "return",
    "esc",
    "escape",
    "tab",
    "backspace",
    "delete",
    "insert",
    "home",
    "end",
    "pageup",
    "pagedown",
    "up",
    "down",
    "left",
    "right",
    "space",
    "plus",
    "capslock",
    "f1",
    "f2",
    "f3",
    "f4",
    "f5",
    "f6",
    "f7",
    "f8",
    "f9",
    "f10",
    "f11",
    "f12",
];

/// A parsed, normalized keyboard spec — see the module docs for the
/// grammar. Cheap to clone; compare via [`KeySpec::as_str`].
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeySpec(String);

impl KeySpec {
    /// Parse and normalize a spec — `None` when the grammar rejects
    /// it (empty, whitespace, unknown modifier, malformed chord).
    #[must_use]
    pub fn parse(spec: &str) -> Option<Self> {
        let t = spec.trim();
        if t.is_empty() || t.chars().any(char::is_whitespace) {
            return None;
        }
        if t.contains('+') {
            return Self::parse_chord(t);
        }
        // Bare spec: named key (canonical lowercase) or char
        // sequence ("dd"). A single char is trivially a sequence.
        let lower = t.to_ascii_lowercase();
        if NAMED_KEYS.contains(&lower.as_str()) {
            return Some(Self(lower));
        }
        // Reject control-ish junk; printable non-+ chars only.
        if t.chars().any(|c| c.is_control()) {
            return None;
        }
        Some(Self(t.to_string()))
    }

    /// The normalized spec string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// For `+`-chords, the `(modifiers, key)` split — modifiers in
    /// canonical [`MODIFIER_ORDER`], key the normalized final token.
    /// `None` for bare named keys and sequences.
    #[must_use]
    pub fn chord(&self) -> Option<(Vec<&'static str>, String)> {
        if !self.0.contains('+') {
            return None;
        }
        let (mods_raw, key) = self.0.rsplit_once('+')?;
        let mods = mods_raw
            .split('+')
            .filter_map(|m| {
                MODIFIER_SPELLINGS
                    .iter()
                    .find(|(c, _)| *c == m)
                    .map(|(c, _)| *c)
            })
            .collect();
        Some((mods, key.to_string()))
    }

    /// `true` for multi-char bare sequences (`"dd"`) — hosts without
    /// a pending-chord mechanism can't bind these.
    #[must_use]
    pub fn is_sequence(&self) -> bool {
        !self.0.contains('+')
            && self.0.chars().count() > 1
            && !NAMED_KEYS.contains(&self.0.as_str())
    }

    fn parse_chord(t: &str) -> Option<Self> {
        let (mods_raw, key_raw) = t.rsplit_once('+')?;
        let key = normalize_key(key_raw)?;
        let mut mods: Vec<&'static str> = Vec::new();
        for m in mods_raw.split('+') {
            let canon = MODIFIER_SPELLINGS
                .iter()
                .find(|(_, spellings)| spellings.iter().any(|s| s.eq_ignore_ascii_case(m)))
                .map(|(canon, _)| *canon)?;
            if mods.contains(&canon) {
                return None; // duplicate modifier
            }
            mods.push(canon);
        }
        // Canonical order, dedup already enforced.
        mods.sort_by_key(|m| {
            MODIFIER_ORDER
                .iter()
                .position(|o| o == m)
                .unwrap_or(usize::MAX)
        });
        let mut out = String::new();
        for m in &mods {
            out.push_str(m);
            out.push('+');
        }
        out.push_str(&key);
        Some(Self(out))
    }
}

/// The final token of a chord: a single char (case preserved) or a
/// canonical lowercase named key.
fn normalize_key(raw: &str) -> Option<String> {
    if raw.is_empty() {
        return None;
    }
    let mut chars = raw.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => {
            if c.is_control() || c == '+' {
                None
            } else {
                Some(c.to_string())
            }
        }
        _ => {
            let lower = raw.to_ascii_lowercase();
            NAMED_KEYS.contains(&lower.as_str()).then_some(lower)
        }
    }
}

impl fmt::Display for KeySpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for KeySpec {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s).ok_or(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn bare_specs() {
        assert_eq!(KeySpec::parse("d").unwrap().as_str(), "d");
        assert_eq!(KeySpec::parse("D").unwrap().as_str(), "D"); // case preserved
        assert_eq!(KeySpec::parse("dd").unwrap().as_str(), "dd");
        assert_eq!(KeySpec::parse("Enter").unwrap().as_str(), "enter");
        assert_eq!(KeySpec::parse(" gq ").unwrap().as_str(), "gq"); // trims
        assert!(KeySpec::parse("").is_none());
        assert!(KeySpec::parse("   ").is_none());
        assert!(KeySpec::parse("g q").is_none()); // whitespace
                                                  // Bare alphabetic words that aren't named keys are sequences.
        assert!(KeySpec::parse("ctrl").unwrap().is_sequence());
    }

    #[test]
    fn chord_specs_normalize() {
        let k = KeySpec::parse("ctrl+k").unwrap();
        assert_eq!(k.as_str(), "ctrl+k");
        assert_eq!(k.chord(), Some((vec!["ctrl"], "k".to_string())));
        assert!(!k.is_sequence());

        // Spellings + order normalize.
        let k = KeySpec::parse("command+shift+Control+K").unwrap();
        assert_eq!(k.as_str(), "ctrl+shift+cmd+K");

        // Named key as chord target.
        let k = KeySpec::parse("alt+enter").unwrap();
        assert_eq!(k.as_str(), "alt+enter");

        // Rejections.
        assert!(KeySpec::parse("ctrl+").is_none());
        assert!(KeySpec::parse("+k").is_none());
        assert!(KeySpec::parse("ctrl+bogus+key").is_none());
        assert!(KeySpec::parse("ctrl+ctrl+k").is_none()); // dup modifier
        assert!(KeySpec::parse("ctrl+multi").is_none()); // multi-char non-named key
        assert!(KeySpec::parse("ctrl+enter+x").is_none());
    }

    #[test]
    fn sequences_and_display() {
        let k = KeySpec::parse("dd").unwrap();
        assert!(k.is_sequence());
        assert_eq!(k.chord(), None);
        assert_eq!(format!("{k}"), "dd");
        assert_eq!("ctrl+x".parse::<KeySpec>().unwrap().as_str(), "ctrl+x");
        assert!("no way".parse::<KeySpec>().is_err());
    }
}
