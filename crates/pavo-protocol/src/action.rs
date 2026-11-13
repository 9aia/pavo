//! Interaction actions — the declarative verb layer of the protocol.
//!
//! An [`Action`] is everything a surface needs to offer one
//! interactive operation: a stable `id`, a `label`, an optional
//! `icon`, keyboard `keys`, typed `params` (each a
//! [`ParamSchema`] — enough for hosts to auto-generate a form), a
//! `danger` level telling hosts how hard to confirm, and an
//! optional `target` description naming the context the action
//! needs (an item id, a row, …).
//!
//! Actions are declared in three places, all the same type:
//!
//! - [`crate::manifest::DriverManifest::actions`] — driver-global
//!   verbs (new item, refresh, …).
//! - [`crate::view_node::NodeBase::actions`] — scoped to a view
//!   node; every ViewNode kind inherits the field.
//! - [`crate::view_node::TreeItem::actions`] — per-row actions on
//!   trees (the only item type with a stable `id` today).
//!
//! Hosts render them as buttons (web), keys/palette entries (TUI),
//! or subcommands/flags (CLI) and dispatch through the wire
//! `invoke` op ([`crate::wire::Request::Invoke`]). `target` on the
//! wire request is opaque JSON — typically the selected row's raw
//! value or an item id — the driver interprets it.
//!
//! Actions may also be *derived* rather than declared: drivers
//! whose ops manifest carries param schemas get actions +
//! auto-generated forms for free (see `pavo-core`'s ops→actions
//! derivation). [`Action::params`] empty means the action takes
//! only its `target` context.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How dangerous an [`Action`] is — drives host confirmation UX.
///
/// - `none` — fire immediately (read-ish ops, navigation).
/// - `confirm` — hosts show a confirm dialog/modal; CLI requires
///   `--yes` or a TTY prompt.
/// - `type-target` — hosts require the user to type the target's
///   id (e.g. `T-012`) before dispatch — the "type the repo name to
///   delete" pattern for irreversible ops.
///
/// Derivation rule: ops-manifest actions built from `mutates: true`
/// ops default to `confirm` when the driver doesn't declare a
/// level; everything else defaults to `none`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DangerLevel {
    /// No confirmation — dispatch on activation.
    #[default]
    None,
    /// One-step confirm dialog.
    Confirm,
    /// Type-the-target-id confirmation.
    TypeTarget,
}

/// The input kind a [`ParamSchema`] declares — richer than the
/// ops-manifest `arg|opt|flag` (which maps params onto argv) and
/// wider than [`crate::view_node::FormFieldKind`] (which is a
/// renderer hint). Hosts use it to pick widgets: `enum` → select,
/// `bool` → toggle, `ref` → fuzzy picker over a query's data,
/// `string[]` → token input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParamKind {
    /// Single-line string.
    #[default]
    String,
    /// Multi-line text.
    Text,
    /// Floating-point number.
    Number,
    /// Integer.
    Integer,
    /// Boolean toggle/checkbox.
    Bool,
    /// One-of selection from `options`.
    Enum,
    /// Date — `YYYY-MM-DD` convention; hosts may offer pickers.
    Date,
    /// Reference picker — the host populates choices from the
    /// view's `ref_query` binding data (objects with `id`/`label`
    /// or `id`/`title` fields, or bare strings).
    Ref,
    /// List of strings — token/tag input.
    #[serde(rename = "string[]")]
    StringList,
}

/// One typed parameter of an [`Action`] — the auto-form substrate.
///
/// Validation (`required`, `pattern`, `min`/`max`, `enum` via
/// `options`) is enforced host-side for UX *and* driver-side for
/// truth; failures come back keyed by `name` in the result's
/// `errors` map (same convention as [`crate::wire::SubmitResult`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParamSchema {
    /// Param name — the key submitted in the invoke `params` map.
    pub name: String,
    /// Input kind — selects the host's widget.
    #[serde(default)]
    pub kind: ParamKind,
    /// Whether the param must be present/non-empty on invoke.
    #[serde(default)]
    pub required: bool,
    /// Human-facing label; hosts default to a humanized `name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Help text rendered under the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    /// Hint text (secondary help, e.g. format examples).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// Placeholder shown while empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    /// Default value (JSON — string/number/bool/array per `kind`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    /// Regex the value must match (string kinds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// Numeric lower bound (`number`/`integer` kinds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    /// Numeric upper bound (`number`/`integer` kinds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// Allowed values (`enum` kind; also documents `ref` choices
    /// when the picker is static).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    /// For `ref` kind — the view query-binding name whose data
    /// populates the picker (e.g. `"items"`, `"actors"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_query: Option<String>,
}

/// One interactive operation — see the module docs for the three
/// declaration sites and the `invoke` dispatch path.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Action {
    /// Stable id — unique within its declaration scope (manifest,
    /// node, or item). Hosts namespace display as
    /// `<driver>:<view>:<id>` where needed; drivers resolve the
    /// bare id against the request's `view` context first, then
    /// manifest-global actions.
    pub id: String,
    /// Human-facing label for buttons/menu entries.
    pub label: String,
    /// Optional icon hint (same resolution rules as
    /// [`crate::manifest::DriverManifest::icon`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Confirmation level — see [`DangerLevel`].
    #[serde(default)]
    pub danger: DangerLevel,
    /// Keyboard shortcuts — single chars or chord strings
    /// (`"dd"`, `"ctrl+k"`). TUI binds them; other hosts may ignore
    /// or map them to CLI flags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<String>,
    /// Typed params — hosts auto-generate a form from these when
    /// the action is activated without a hand-authored form.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<ParamSchema>,
    /// What context the action needs — a hint for hosts ("item",
    /// "row", "selection") and for palette grouping. The actual
    /// context travels in the invoke request's `target`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Longer description for palette detail lines / tooltips.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Cross-driver target — reserved: when set, the host should
    /// resolve `driver` (a `tamias-driver-<type>` name) and issue
    /// the invoke against *that* driver's `serve`. v1 hosts may
    /// ignore it and treat the action as local (same convention as
    /// [`crate::manifest::QueryBinding::driver`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver: Option<String>,
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use serde_json::json;

    #[test]
    fn action_roundtrips_minimal_and_full() {
        let min: Action = serde_json::from_value(json!({
            "id": "refresh", "label": "Refresh"
        }))
        .unwrap();
        assert_eq!(min.danger, DangerLevel::None);
        assert!(min.params.is_empty());

        let full: Action = serde_json::from_value(json!({
            "id": "todo.done", "label": "Mark done", "icon": "check",
            "danger": "type-target", "keys": ["dd", "D"],
            "target": "item",
            "params": [
                {"name": "id", "kind": "ref", "required": true,
                 "ref_query": "items"},
                {"name": "note", "kind": "text", "hint": "what happened"}
            ]
        }))
        .unwrap();
        assert_eq!(full.danger, DangerLevel::TypeTarget);
        assert_eq!(full.params[0].kind, ParamKind::Ref);
        assert_eq!(full.params[0].ref_query.as_deref(), Some("items"));
        assert_eq!(full.params[1].kind, ParamKind::Text);
    }

    #[test]
    fn param_kind_wire_spellings() {
        // Mirrors the `kind` enum in protocol/v1/*.schema.json.
        for (wire, kind) in [
            ("string", ParamKind::String),
            ("text", ParamKind::Text),
            ("number", ParamKind::Number),
            ("integer", ParamKind::Integer),
            ("bool", ParamKind::Bool),
            ("enum", ParamKind::Enum),
            ("date", ParamKind::Date),
            ("ref", ParamKind::Ref),
            ("string[]", ParamKind::StringList),
        ] {
            let p: ParamSchema = serde_json::from_value(json!({"name":"x","kind":wire})).unwrap();
            assert_eq!(p.kind, kind, "wire spelling {wire}");
            assert_eq!(serde_json::to_value(p.kind).unwrap(), wire);
        }
    }

    #[test]
    fn danger_level_spellings() {
        for (wire, level) in [
            ("none", DangerLevel::None),
            ("confirm", DangerLevel::Confirm),
            ("type-target", DangerLevel::TypeTarget),
        ] {
            let a: Action =
                serde_json::from_value(json!({"id":"x","label":"X","danger":wire})).unwrap();
            assert_eq!(a.danger, level);
        }
    }
}
