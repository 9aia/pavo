//! Interaction actions — collection, auto-form mapping, and
//! validation helpers for the `invoke` wire op.
//!
//! Actions are declared in three places (all the same
//! [`pavo_protocol::Action`] type): `DriverManifest.actions[]`,
//! `NodeBase.actions[]` on any view node, and `TreeItem.actions[]`
//! on tree rows. [`collect_actions`] folds them into one list a
//! host can render (buttons, palette entries, keybindings) —
//! manifest-global first, then node actions in tree pre-order,
//! then tree-item actions; when ids collide hosts dedupe with
//! item > node > manifest precedence. [`param_form_field`] maps a
//! [`ParamSchema`] onto the existing
//! [`FormField`] widget set so hosts can auto-generate forms;
//! [`validate_params`] is the shared client-side check (drivers
//! stay authoritative — `ActionResult.errors` is the truth).
//!
//! [`ops_actions`] is the zero-declaration fallback: a driver's
//! ops manifest (`{"ops":[{name,doc,params,mutates}]}` — the shape
//! `pavo-driver-kit`'s surface layer parses) derives one action
//! per op, so every ops-manifest driver gets a mutation surface
//! without declaring `actions[]`.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};

use pavo_protocol::wire::ViewResult;
use pavo_protocol::{
    Action, DangerLevel, DriverManifest, FormField, FormFieldKind, ParamKind, ParamSchema,
    TreeItem, ViewNode,
};
use serde_json::{Map, Value};

use crate::viewnode::walk;

/// Where an action was declared — scoping + display context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionScope {
    /// `DriverManifest.actions[]` — offered on every view.
    Manifest,
    /// `NodeBase.actions[]` — scoped to the node carrying it.
    Node,
    /// `TreeItem.actions[]` — scoped to one tree row.
    Item,
}

/// An [`Action`] plus the context needed to dispatch it.
///
/// `action`/`item_id` are [`Cow`]s: actions on literal tree items
/// borrow straight out of the [`ViewResult`]; actions on
/// query-bound tree items (the renderer resolves
/// `data[node.query].items` lazily) are owned clones — same
/// precedence as inline items, no caller-side materialization.
#[derive(Clone, Debug)]
pub struct ScopedAction<'a> {
    /// The action declaration.
    pub action: Cow<'a, Action>,
    /// Which declaration site it came from.
    pub scope: ActionScope,
    /// The view the action was found in (`ViewResult.view.id`) —
    /// the `invoke` op's `view` context. `None` only for manifest
    /// actions collected without a view.
    pub view: Option<String>,
    /// For [`ActionScope::Item`] — the tree item's `id` (the
    /// natural `target` for the invoke request).
    pub item_id: Option<Cow<'a, str>>,
}

fn collect_tree_items<'a>(
    items: &'a [TreeItem],
    view: &Option<String>,
    out: &mut Vec<ScopedAction<'a>>,
) {
    for item in items {
        for action in &item.actions {
            out.push(ScopedAction {
                action: Cow::Borrowed(action),
                scope: ActionScope::Item,
                view: view.clone(),
                item_id: item.id.as_deref().map(Cow::Borrowed),
            });
        }
        collect_tree_items(&item.children, view, out);
    }
}

/// Deserialize a query payload into tree items — accepts a bare
/// `TreeItem[]` or `{"items": [...]}` (same shapes renderers
/// accept).
fn payload_tree_items(payload: &Value) -> Option<Vec<TreeItem>> {
    let value = if payload.is_array() {
        payload.clone()
    } else {
        payload.get("items")?.clone()
    };
    serde_json::from_value(value).ok()
}

/// Every action visible in `view_result`, in declaration order:
/// manifest-global actions first, then node actions (pre-order
/// walk), then tree-item actions (pre-order per tree).
///
/// Hosts typically dedupe by `action.id` with scope precedence
/// item > node > manifest when ids collide.
#[must_use]
pub fn collect_actions<'a>(
    manifest: &'a DriverManifest,
    view_result: &'a ViewResult,
) -> Vec<ScopedAction<'a>> {
    let view = Some(view_result.view.id.clone());
    let mut out: Vec<ScopedAction<'a>> = manifest
        .actions
        .iter()
        .map(|action| ScopedAction {
            action: Cow::Borrowed(action),
            scope: ActionScope::Manifest,
            view: view.clone(),
            item_id: None,
        })
        .collect();
    // Query-bound trees resolve `data[query].items` lazily in the
    // renderers; do the same here so row actions survive. Resolved
    // items are owned clones (see [`ScopedAction`]).
    let mut resolved: HashMap<String, Vec<TreeItem>> = HashMap::new();
    walk(&view_result.view.root, &mut |node| {
        for action in &node.base().actions {
            out.push(ScopedAction {
                action: Cow::Borrowed(action),
                scope: ActionScope::Node,
                view: view.clone(),
                item_id: None,
            });
        }
        if let ViewNode::Tree(tree) = node {
            if !tree.items.is_empty() {
                collect_tree_items(&tree.items, &view, &mut out);
            } else if let Some(query) = &tree.base.query {
                let items = resolved.entry(query.clone()).or_insert_with(|| {
                    view_result
                        .data
                        .get(query)
                        .and_then(payload_tree_items)
                        .unwrap_or_default()
                });
                collect_tree_items_owned(items, &view, &mut out);
            }
        }
    });
    out
}

/// Item actions for an owned (query-resolved) item list. The
/// source borrow is independent of `'a` — every field is cloned
/// into the [`ScopedAction`], so the resolved-items cache may drop
/// freely.
fn collect_tree_items_owned<'a>(
    items: &[TreeItem],
    view: &Option<String>,
    out: &mut Vec<ScopedAction<'a>>,
) {
    for item in items {
        for action in &item.actions {
            out.push(ScopedAction {
                action: Cow::Owned(action.clone()),
                scope: ActionScope::Item,
                view: view.clone(),
                item_id: item.id.clone().map(Cow::Owned),
            });
        }
        collect_tree_items_owned(&item.children, view, out);
    }
}

/// Manifest actions alone — for palette enumeration before any
/// view resolves.
#[must_use]
pub fn manifest_actions(manifest: &DriverManifest) -> &[Action] {
    &manifest.actions
}

/// `menus[]` palette entries across merged chrome — see
/// [`crate::merge`]; kept here as a typed accessor so hosts don't
/// dig through `Contributes`.
#[must_use]
pub fn menu_entries(manifest: &DriverManifest) -> &[pavo_protocol::MenuEntry] {
    &manifest.contributes.menus
}

/// Map a [`ParamSchema`] onto the [`FormField`] widget set —
/// the auto-form half of the interaction layer. `ref` params
/// degrade to text with a picker hint (hosts with query data may
/// upgrade to a real picker); `string[]` degrades to comma text.
#[must_use]
pub fn param_form_field(param: &ParamSchema) -> FormField {
    let (kind, hint_suffix) = match param.kind {
        ParamKind::String => (FormFieldKind::Text, None),
        ParamKind::Text => (FormFieldKind::Textarea, None),
        ParamKind::Number | ParamKind::Integer => (FormFieldKind::Number, None),
        ParamKind::Bool => (FormFieldKind::Bool, None),
        ParamKind::Enum => (FormFieldKind::Select, None),
        ParamKind::Date => (FormFieldKind::Text, Some("YYYY-MM-DD".to_string())),
        ParamKind::Ref => (
            FormFieldKind::Text,
            param
                .ref_query
                .as_deref()
                .map(|q| format!("pick from \"{q}\"")),
        ),
        ParamKind::StringList => (FormFieldKind::Text, Some("comma-separated".to_string())),
    };
    let mut hint = param.hint.clone().or_else(|| param.doc.clone());
    if let Some(suffix) = hint_suffix {
        hint = Some(match hint {
            Some(h) => format!("{h} ({suffix})"),
            None => suffix.to_string(),
        });
    }
    FormField {
        key: param.name.clone(),
        label: param.label.clone().unwrap_or_else(|| humanize(&param.name)),
        kind,
        required: param.required,
        options: param.options.clone(),
        default: param.default.clone(),
        value: None,
        hint,
        editable: true,
        placeholder: param.placeholder.clone(),
    }
}

/// `snake_case`/`kebab-case`/`camelCase` → `Snake case` label.
fn humanize(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    let mut prev_sep = true;
    for (i, c) in name.chars().enumerate() {
        if c == '_' || c == '-' {
            if !prev_sep && !out.is_empty() {
                out.push(' ');
            }
            prev_sep = true;
            continue;
        }
        if c.is_uppercase() && i > 0 && !prev_sep {
            out.push(' ');
        }
        out.push(c.to_ascii_lowercase());
        prev_sep = false;
    }
    if out.is_empty() {
        name.to_string()
    } else {
        let mut s = out;
        if let Some(f) = s.get_mut(0..1) {
            f.make_ascii_uppercase();
        }
        s
    }
}

/// Client-side param validation — `required` presence, `options`
/// membership, `min`/`max` bounds. Returns per-param error strings
/// keyed by `ParamSchema.name` (empty map = valid). Pattern checks
/// need a regex dep the core doesn't carry — drivers enforce
/// `pattern` authoritatively.
#[must_use]
pub fn validate_params(action: &Action, params: &Map<String, Value>) -> Map<String, Value> {
    let mut errors = Map::new();
    for p in &action.params {
        let v = params.get(&p.name);
        let missing = match v {
            None | Some(Value::Null) => true,
            Some(Value::String(s)) => s.is_empty() && p.kind != ParamKind::StringList,
            Some(Value::Array(a)) => a.is_empty(),
            _ => false,
        };
        if p.required && missing {
            errors.insert(p.name.clone(), Value::from("required"));
            continue;
        }
        let Some(v) = v.filter(|v| !v.is_null()) else {
            continue;
        };
        if !p.options.is_empty() {
            let ok = match v {
                Value::String(s) => p.options.iter().any(|o| o == s),
                Value::Array(xs) => xs
                    .iter()
                    .all(|x| x.as_str().is_some_and(|s| p.options.iter().any(|o| o == s))),
                _ => false,
            };
            if !ok {
                errors.insert(p.name.clone(), Value::from("invalid option"));
                continue;
            }
        }
        if let Some(n) = v.as_f64() {
            if p.min.is_some_and(|min| n < min) || p.max.is_some_and(|max| n > max) {
                errors.insert(p.name.clone(), Value::from("out of range"));
            }
        }
    }
    errors
}

/// Derive actions from an ops manifest — the zero-declaration
/// fallback. Accepts the raw JSON a driver answers to `query
/// "ops"` (or `<drv> ops` argv): `{"ops":[{"name"|"query","doc",
/// "params":[{name,kind:arg|opt|flag,doc,schema?}],"mutates"}]}`.
///
/// Mapping: op name (sans `cli.`) → action `id`; `doc` → `label`
/// (first line) + `description`; `mutates:true` →
/// [`DangerLevel::Confirm`]; params → [`ParamSchema`]s — `flag` →
/// `bool`, `arg`/`opt` → `string`, upgraded to the param's own
/// `schema` object when present.
#[must_use]
pub fn ops_actions(manifest: &Value) -> Vec<Action> {
    let mut out = Vec::new();
    let Some(ops) = manifest.get("ops").and_then(Value::as_array) else {
        return out;
    };
    for op in ops {
        let name = op
            .get("name")
            .or_else(|| op.get("query"))
            .and_then(Value::as_str)
            .map(|s| s.strip_prefix("cli.").unwrap_or(s).to_string());
        let Some(name) = name else { continue };
        let doc = op.get("doc").and_then(Value::as_str).unwrap_or("");
        let label = doc.lines().next().unwrap_or("").trim();
        let params = op
            .get("params")
            .and_then(Value::as_array)
            .map(|ps| ps.iter().filter_map(op_param_schema).collect())
            .unwrap_or_default();
        out.push(Action {
            id: name.clone(),
            label: if label.is_empty() {
                humanize(&name)
            } else {
                label.to_string()
            },
            icon: None,
            danger: if op.get("mutates").and_then(Value::as_bool).unwrap_or(false) {
                DangerLevel::Confirm
            } else {
                DangerLevel::None
            },
            keys: Vec::new(),
            params,
            target: None,
            description: (!doc.is_empty()).then(|| doc.to_string()),
            driver: None,
            route: None,
        });
    }
    out
}

/// One ops-manifest param → [`ParamSchema`]. `flag` → `bool`;
/// `arg`/`opt`/unknown → `string`; a param's own `schema` object
/// (full ParamSchema JSON) wins when present.
fn op_param_schema(p: &Value) -> Option<ParamSchema> {
    if let Some(schema) = p.get("schema") {
        if let Ok(s) = serde_json::from_value::<ParamSchema>(schema.clone()) {
            return Some(s);
        }
    }
    let name = p
        .get("name")
        .and_then(Value::as_str)
        .or_else(|| p.as_str())?
        .to_string();
    let kind = match p.get("kind").and_then(Value::as_str) {
        Some("flag") => ParamKind::Bool,
        _ => ParamKind::String,
    };
    Some(ParamSchema {
        name,
        kind,
        required: false,
        label: None,
        doc: p.get("doc").and_then(Value::as_str).map(str::to_string),
        hint: None,
        placeholder: None,
        default: None,
        pattern: None,
        min: None,
        max: None,
        options: Vec::new(),
        ref_query: None,
    })
}

/// Dedupe helper for hosts merging declared + ops-derived actions:
/// first occurrence of an id wins (declaration order = precedence).
#[must_use]
pub fn dedupe_actions(actions: Vec<Action>) -> Vec<Action> {
    let mut seen = std::collections::BTreeSet::new();
    actions
        .into_iter()
        .filter(|a| seen.insert(a.id.clone()))
        .collect()
}

/// Sorted param map for tests/debug — params by name.
#[must_use]
pub fn params_by_name(action: &Action) -> BTreeMap<&str, &ParamSchema> {
    action.params.iter().map(|p| (p.name.as_str(), p)).collect()
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use pavo_protocol::wire::ViewResult;
    use serde_json::json;

    fn manifest_with(actions: Vec<Action>) -> DriverManifest {
        DriverManifest {
            actions,
            ..DriverManifest {
                name: "d".into(),
                title: "D".into(),
                icon: None,
                version: "0".into(),
                verbs: vec![],
                capabilities: vec![],
                deps: None,
                contributes: Default::default(),
                views: vec![],
                renderers: vec![],
                actions: vec![],
            }
        }
    }

    fn view_result(root: ViewNode) -> ViewResult {
        ViewResult {
            view: pavo_protocol::ViewSpec {
                id: "v".into(),
                title: "V".into(),
                route: "/v".into(),
                description: None,
                queries: vec![],
                root,
                item_route: None,
                filter_param: None,
            },
            data: Map::new(),
        }
    }

    #[test]
    fn collect_actions_covers_all_sites() {
        let done = Action {
            id: "done".into(),
            label: "Done".into(),
            icon: None,
            danger: DangerLevel::Confirm,
            keys: vec![],
            params: vec![],
            target: None,
            description: None,
            driver: None,
            route: None,
        };
        let manifest = manifest_with(vec![done.clone()]);
        let root: ViewNode = serde_json::from_value(json!({
            "kind":"column","children":[
                {"kind":"table","actions":[{"id":"refresh","label":"Refresh"}]},
                {"kind":"tree","items":[
                    {"id":"i1","label":"one","actions":[{"id":"archive","label":"Archive"}]},
                    {"id":"i2","label":"two","children":[
                        {"id":"i3","label":"three","actions":[{"id":"deep","label":"Deep"}]}
                    ]}
                ]}
            ]
        }))
        .unwrap();
        let vr = view_result(root);
        let scoped = collect_actions(&manifest, &vr);
        let ids: Vec<(&str, ActionScope)> = scoped
            .iter()
            .map(|s| (s.action.id.as_str(), s.scope))
            .collect();
        assert_eq!(
            ids,
            [
                ("done", ActionScope::Manifest),
                ("refresh", ActionScope::Node),
                ("archive", ActionScope::Item),
                ("deep", ActionScope::Item),
            ]
        );
        assert_eq!(scoped[2].item_id.as_deref(), Some("i1"));
        assert_eq!(scoped[3].item_id.as_deref(), Some("i3"));
        assert!(scoped.iter().all(|s| s.view.as_deref() == Some("v")));
    }

    #[test]
    fn collect_actions_resolves_query_bound_tree_items() {
        let manifest = manifest_with(vec![]);
        let root: ViewNode = serde_json::from_value(json!({
            "kind":"tree","query":"tasks","items":[]
        }))
        .unwrap();
        let mut vr = view_result(root);
        vr.data.insert(
            "tasks".into(),
            json!({"items":[
                {"id":"T-1","label":"one","actions":[{"id":"done","label":"Done"}]},
                {"id":"T-2","label":"two","children":[
                    {"id":"T-3","label":"three","actions":[{"id":"cancel","label":"Cancel"}]}
                ]}
            ]}),
        );
        let scoped = collect_actions(&manifest, &vr);
        let ids: Vec<(&str, ActionScope)> = scoped
            .iter()
            .map(|s| (s.action.id.as_str(), s.scope))
            .collect();
        assert_eq!(
            ids,
            [("done", ActionScope::Item), ("cancel", ActionScope::Item),]
        );
        assert_eq!(scoped[0].item_id.as_deref(), Some("T-1"));
        assert_eq!(scoped[1].item_id.as_deref(), Some("T-3"));
    }

    #[test]
    fn param_form_field_maps_kinds() {
        let p = ParamSchema {
            name: "count".into(),
            kind: ParamKind::Integer,
            required: true,
            label: None,
            doc: Some("how many".into()),
            hint: None,
            placeholder: None,
            default: None,
            pattern: None,
            min: Some(0.0),
            max: None,
            options: vec![],
            ref_query: None,
        };
        let f = param_form_field(&p);
        assert_eq!(f.kind, FormFieldKind::Number);
        assert_eq!(f.label, "Count");
        assert!(f.required);
        assert_eq!(f.hint.as_deref(), Some("how many"));

        let e = ParamSchema {
            kind: ParamKind::Enum,
            options: vec!["a".into(), "b".into()],
            ..p.clone()
        };
        let f = param_form_field(&e);
        assert_eq!(f.kind, FormFieldKind::Select);
        assert_eq!(f.options, vec!["a", "b"]);
    }

    #[test]
    fn validate_params_checks_required_options_bounds() {
        let action: Action = serde_json::from_value(json!({
            "id":"x","label":"X",
            "params":[
                {"name":"id","kind":"string","required":true},
                {"name":"pri","kind":"enum","options":["low","high"]},
                {"name":"n","kind":"integer","min":1,"max":5}
            ]
        }))
        .unwrap();
        let mut params = Map::new();
        params.insert("id".into(), json!("T-1"));
        params.insert("pri".into(), json!("bogus"));
        params.insert("n".into(), json!(9));
        let errors = validate_params(&action, &params);
        assert_eq!(errors.len(), 2);
        assert!(errors.contains_key("pri") && errors.contains_key("n"));

        let mut ok = Map::new();
        ok.insert("id".into(), json!("T-1"));
        ok.insert("pri".into(), json!("low"));
        ok.insert("n".into(), json!(3));
        assert!(validate_params(&action, &ok).is_empty());

        assert!(validate_params(&action, &Map::new()).contains_key("id"));
    }

    #[test]
    fn ops_actions_derives_from_manifest() {
        let v = json!({
            "engine":"myco",
            "ops":[
                {"query":"cli.status","doc":"Show status\nlonger","params":[]},
                {"name":"todo.done","doc":"Mark a task done","mutates":true,
                 "params":[
                    {"name":"id","kind":"arg","doc":"Task id"},
                    {"name":"json","kind":"flag"},
                    {"name":"pri","kind":"opt","schema":{"name":"pri","kind":"enum","options":["low","high"]}}
                 ]},
                {"surfaces_only":true}
            ]
        });
        let actions = ops_actions(&v);
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0].id, "status");
        assert_eq!(actions[0].danger, DangerLevel::None);
        assert_eq!(actions[1].id, "todo.done");
        assert_eq!(actions[1].danger, DangerLevel::Confirm);
        assert_eq!(actions[1].label, "Mark a task done");
        assert_eq!(actions[1].params.len(), 3);
        assert_eq!(actions[1].params[0].kind, ParamKind::String);
        assert_eq!(actions[1].params[1].kind, ParamKind::Bool);
        assert_eq!(actions[1].params[2].kind, ParamKind::Enum);
        assert!(ops_actions(&json!({})).is_empty());
    }
}
