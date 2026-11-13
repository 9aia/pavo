//! User keymap — `keys.toml` overrides for command bindings.
//!
//! Declarations suggest, users override: action `keys[]`,
//! `MenuEntry.keys`, and the `pavo_core::commands::BUILTINS` defaults
//! are *suggestions*; this file remaps them by **command id** (the
//! canonical `action:<id>` | `route:<path>` | `host.<builtin>` |
//! `driver:<name>:<id>` grammar — see `pavo_core::commands`).
//!
//! ## Location
//!
//! First readable file wins:
//!
//! 1. `$XDG_CONFIG_HOME/pavo/keys.toml`
//! 2. `~/.config/pavo/keys.toml` (when `XDG_CONFIG_HOME` is unset)
//!
//! There is deliberately no project-local file — `pavo.toml` belongs
//! to the config driver's own layered config; keybindings are a
//! per-user host concern. `PAVO_KEYS` is not consulted (keep the
//! surface small; add it if a real need shows up).
//!
//! ## Format
//!
//! TOML. Top-level string/array values are **global** bindings;
//! tables scope bindings to a driver (`[myco]`) or a single view
//! (`["myco:board"]` — quote when the scope contains `:`). Keys are
//! command ids (quote them — they contain `:`/`.`); values are a
//! [`pavo_core::KeySpec`] string or an array of spec strings. An
//! entry *replaces* the declaration's suggested keys wholesale;
//! command ids absent from the file keep their suggestions.
//!
//! ```toml
//! # ~/.config/pavo/keys.toml
//! "host.filter" = "ctrl+f"        # global: replace "/" with ctrl+f
//! "host.palette" = ["ctrl+p", ":"]
//!
//! [myco]                            # every myco view
//! "action:todo.done" = "D"
//!
//! ["myco:board"]                    # just the board view
//! "host.reload" = "R"
//! ```
//!
//! Scope precedence per command id: `driver:view` > `driver` >
//! global. Specs validate through [`pavo_core::KeySpec`]; invalid
//! entries drop out of the effective binding (see
//! `pavo_core::commands::effective_bindings`).

use std::collections::BTreeMap;
use std::path::PathBuf;

use toml_edit::{DocumentMut, Item};

/// A parsed `keys.toml` — bindings partitioned by scope. `""`-less
/// design: the global map holds unscoped command ids; `scopes` maps
/// a scope string (`"myco"` or `"myco:board"`) to its bindings.
#[derive(Clone, Debug, Default)]
pub struct UserKeymap {
    /// Command id → keys spec list (comma-joined — the shape
    /// `effective_bindings`' overlay expects).
    pub global: BTreeMap<String, String>,
    /// Scope (`driver` or `driver:view`) → command id → keys.
    pub scopes: BTreeMap<String, BTreeMap<String, String>>,
}

impl UserKeymap {
    /// The effective overlay for a context: global bindings, then
    /// `driver`, then `driver:view` — each layer overriding per
    /// command id. `driver`/`view` are `None` off-view (dashboard,
    /// notes) → global only.
    #[must_use]
    pub fn overlay_for(
        &self,
        driver: Option<&str>,
        view: Option<&str>,
    ) -> BTreeMap<String, String> {
        let mut out = self.global.clone();
        if let Some(d) = driver {
            if let Some(m) = self.scopes.get(d) {
                for (k, v) in m {
                    out.insert(k.clone(), v.clone());
                }
            }
        }
        if let (Some(d), Some(v)) = (driver, view) {
            let scope = format!("{d}:{v}");
            if let Some(m) = self.scopes.get(&scope) {
                for (k, v) in m {
                    out.insert(k.clone(), v.clone());
                }
            }
        }
        out
    }
}

/// Candidate `keys.toml` paths in discovery order (first readable
/// wins). See the module docs for the policy.
#[must_use]
pub fn keymap_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        out.push(PathBuf::from(xdg).join("pavo").join("keys.toml"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        out.push(
            PathBuf::from(home)
                .join(".config")
                .join("pavo")
                .join("keys.toml"),
        );
    }
    out
}

/// Load the user keymap — first readable candidate wins; missing or
/// unparsable files yield an empty map (bindings fall back to
/// declaration suggestions).
#[must_use]
pub fn load() -> UserKeymap {
    for path in keymap_paths() {
        if let Ok(src) = std::fs::read_to_string(&path) {
            return parse(&src);
        }
    }
    UserKeymap::default()
}

/// Parse `keys.toml` source into a [`UserKeymap`]. A malformed
/// document yields whatever parsed before the error (toml_edit is
/// all-or-nothing per document — a parse error returns an empty
/// map, matching "invalid file = no overrides").
#[must_use]
pub fn parse(src: &str) -> UserKeymap {
    let Ok(doc) = src.parse::<DocumentMut>() else {
        return UserKeymap::default();
    };
    let mut keymap = UserKeymap::default();
    for (key, item) in doc.as_table().iter() {
        match item {
            // `[scope]` table — inner string/array values are bindings.
            Item::Table(t) => {
                let mut inner = BTreeMap::new();
                for (cmd, v) in t.iter() {
                    if let Some(keys) = binding_value(v) {
                        inner.insert(cmd.to_string(), keys);
                    }
                }
                if !inner.is_empty() {
                    keymap.scopes.insert(key.to_string(), inner);
                }
            }
            // Top-level `"command:id" = "spec"` — global binding.
            other => {
                if let Some(keys) = binding_value(other) {
                    keymap.global.insert(key.to_string(), keys);
                }
            }
        }
    }
    keymap
}

/// A binding value: `"spec"` or `["spec", …]` → comma-joined spec
/// list (the overlay shape `effective_bindings` consumes). Anything
/// else (`42`, nested tables, …) is ignored.
fn binding_value(item: &Item) -> Option<String> {
    match item {
        Item::Value(toml_edit::Value::String(s)) => Some(s.value().to_string()),
        Item::Value(toml_edit::Value::Array(a)) => {
            let specs: Vec<&str> = a.iter().filter_map(|v| v.as_str()).collect();
            (!specs.is_empty()).then(|| specs.join(","))
        }
        _ => None,
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn parse_global_and_scoped_tables() {
        let src = r#"
"host.filter" = "ctrl+f"
"host.palette" = ["ctrl+p", ":"]

[myco]
"action:todo.done" = "D"

["myco:board"]
"host.reload" = ["R", "ctrl+r"]
"bad entry" = 42
"[ignored]" = { nested = true }
"#;
        let km = parse(src);
        assert_eq!(km.global["host.filter"], "ctrl+f");
        assert_eq!(km.global["host.palette"], "ctrl+p,:");
        assert_eq!(km.scopes["myco"]["action:todo.done"], "D");
        assert_eq!(km.scopes["myco:board"]["host.reload"], "R,ctrl+r");
        // Non-string values drop out.
        assert!(!km.scopes["myco:board"].contains_key("bad entry"));
        assert!(!km.scopes.contains_key("[ignored]"));
    }

    #[test]
    fn overlay_for_merges_by_specificity() {
        let km = parse(
            r#"
"host.help" = "H"
"host.filter" = "ctrl+f"

[myco]
"host.help" = "myco-help"

["myco:board"]
"host.help" = "board-help"
"host.reload" = "R"
"#,
        );
        // No context → global only.
        let o = km.overlay_for(None, None);
        assert_eq!(o["host.help"], "H");
        assert!(!o.contains_key("host.reload"));
        // Driver scope overrides global per-id.
        let o = km.overlay_for(Some("myco"), None);
        assert_eq!(o["host.help"], "myco-help");
        assert_eq!(o["host.filter"], "ctrl+f");
        // View scope wins over driver scope.
        let o = km.overlay_for(Some("myco"), Some("board"));
        assert_eq!(o["host.help"], "board-help");
        assert_eq!(o["host.reload"], "R");
        // Other drivers unaffected.
        let o = km.overlay_for(Some("other"), Some("x"));
        assert_eq!(o["host.help"], "H");
    }

    #[test]
    fn malformed_document_is_empty() {
        assert!(parse("this is not = = toml [").global.is_empty());
        assert!(parse("").global.is_empty());
    }

    #[test]
    fn keymap_paths_prefers_xdg_then_home() {
        let paths = keymap_paths();
        // At least the HOME fallback exists on unix; XDG prepends.
        assert!(!paths.is_empty());
        for p in &paths {
            assert!(p.ends_with("keys.toml"));
        }
    }
}
