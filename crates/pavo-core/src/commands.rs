//! The unified **command layer** — everything invocable resolves to
//! one grammar, one resolver, every host.
//!
//! ```text
//! action:<id>              invoke the action (driver dispatch)
//! route:<path>             host navigation (template over target/params)
//! host.<builtin>           host capability (filter, help, reload, …)
//! driver:<name>:<id>       cross-driver action (reserved; resolves locally)
//! ```
//!
//! [`Command::parse`] turns a command string into a [`Command`];
//! [`resolve_command`] maps it plus a [`ResolveContext`] (current
//! driver, manifest, view result, sibling-driver manifests, invoke
//! target/params) to a [`Resolved`] the host executes:
//!
//! - [`Resolved::Dispatch`] — send `{"op":"invoke"}` via
//!   [`crate::Host::invoke`] (the `driver`/`view`/`action`/`target`/
//!   `params` fields are already resolved; `declared` carries the
//!   [`Action`] when one was found, for params/danger UX).
//! - [`Resolved::Navigate`] — interpolate-free route string; the
//!   host navigates locally (no wire round-trip). Produced by
//!   `route:` commands, [`Action::route`], and
//!   [`crate::pavo_protocol::ViewSpec::item_route`] templates.
//! - [`Resolved::Builtin`] — a [`HostBuiltin`] the host implements
//!   itself (see [`BUILTINS`] for the catalog + suggested keys).
//! - [`Resolved::Unsupported`] — the command parsed but can't
//!   resolve here (unknown driver, …); display-only.
//!
//! Route **templates** (`{a.b.c}` placeholders) are interpolated by
//! [`interpolate`] over a JSON-ish context — see
//! [`ResolveContext::interpolate_ctx`] for the shape
//! (`{driver, view, target, params, item, data}`). `ActionResult.route`
//! (driver redirect post-invoke) uses the same rules: hosts
//! interpolate it over the invoke context and navigate.
//!
//! **Bindings**: declarations suggest, users override. [`Action::keys`],
//! [`MenuEntry::keys`], and [`BUILTINS`] default keys are *suggestions*;
//! a host keymap file (e.g. `keys.toml` — host config, not protocol)
//! remaps by command id. [`effective_bindings`] merges both into the
//! sorted binding list hosts render in help/palette UI; specs
//! validate via [`crate::keymap::KeySpec`].

use std::borrow::Cow;
use std::collections::BTreeMap;

use pavo_protocol::manifest::MenuEntry;
use pavo_protocol::wire::ViewResult;
use pavo_protocol::{Action, DriverManifest, TreeItem};
use serde_json::{json, Map, Value};

use crate::actions::{collect_actions, ActionScope, ScopedAction};
use crate::Params;

/// A host capability command — synthesized by the host, never sent
/// to a driver. See [`BUILTINS`] for labels/keys/descriptions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HostBuiltin {
    /// `host.filter` — filter dialog bound to the view's
    /// `filter_param` (convention `q`); submit re-resolves the view.
    Filter,
    /// `host.help` — help/keybinding overlay.
    Help,
    /// `host.reload` — re-resolve the current view (bust caches).
    Reload,
    /// `host.editor` — open the current target in `$EDITOR`.
    Editor,
    /// `host.palette` — open the command palette.
    Palette,
    /// `host.quit` — exit the host.
    Quit,
}

impl HostBuiltin {
    /// Parse the bare builtin name (`"filter"`, …) — the part after
    /// `host.` in the command grammar.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "filter" => Some(Self::Filter),
            "help" => Some(Self::Help),
            "reload" => Some(Self::Reload),
            "editor" => Some(Self::Editor),
            "palette" => Some(Self::Palette),
            "quit" => Some(Self::Quit),
            _ => None,
        }
    }

    /// The builtin's short name (`"filter"`).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Filter => "filter",
            Self::Help => "help",
            Self::Reload => "reload",
            Self::Editor => "editor",
            Self::Palette => "palette",
            Self::Quit => "quit",
        }
    }

    /// The canonical command id (`"host.filter"`).
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::Filter => "host.filter",
            Self::Help => "host.help",
            Self::Reload => "host.reload",
            Self::Editor => "host.editor",
            Self::Palette => "host.palette",
            Self::Quit => "host.quit",
        }
    }
}

/// One row of the [`BUILTINS`] catalog — everything a host needs to
/// synthesize builtins into palettes and default keymaps.
#[derive(Clone, Copy, Debug)]
pub struct BuiltinDecl {
    /// The builtin itself.
    pub builtin: HostBuiltin,
    /// Canonical command id (`host.<name>`).
    pub id: &'static str,
    /// Palette/help label.
    pub label: &'static str,
    /// Suggested default keys — *suggestions*; the user keymap
    /// overrides by command id (see [`effective_bindings`]).
    pub keys: &'static [&'static str],
    /// One-line description for palette detail / help screens.
    pub description: &'static str,
}

/// The host-builtin catalog — synthesized into every host's palette
/// and default keymap. Order is display order.
pub const BUILTINS: &[BuiltinDecl] = &[
    BuiltinDecl {
        builtin: HostBuiltin::Filter,
        id: "host.filter",
        label: "Filter view",
        keys: &["/"],
        description: "Filter the current view's filter param (default \"q\") and re-resolve.",
    },
    BuiltinDecl {
        builtin: HostBuiltin::Help,
        id: "host.help",
        label: "Help",
        keys: &["?"],
        description: "Show this help / keybinding list.",
    },
    BuiltinDecl {
        builtin: HostBuiltin::Reload,
        id: "host.reload",
        label: "Reload view",
        keys: &["R"],
        description: "Re-resolve the current view, bypassing caches.",
    },
    BuiltinDecl {
        builtin: HostBuiltin::Editor,
        id: "host.editor",
        label: "Open in editor",
        keys: &["e"],
        description: "Open the current target in $EDITOR (host-dependent).",
    },
    BuiltinDecl {
        builtin: HostBuiltin::Palette,
        id: "host.palette",
        label: "Command palette",
        keys: &["ctrl+p"],
        description: "Open the fuzzy command palette.",
    },
    BuiltinDecl {
        builtin: HostBuiltin::Quit,
        id: "host.quit",
        label: "Quit",
        keys: &["q"],
        description: "Exit pavo.",
    },
];

/// One invocable command — the unified grammar. Serializable via
/// [`Command::id`]/[`std::fmt::Display`] (the canonical string form
/// is what keymaps, palette rows, and `MenuEntry.command` carry).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// `action:<id>` — dispatch an action on the *current* driver.
    Action {
        /// The action id (resolved against node/item/manifest scope
        /// like the `invoke` op).
        id: String,
    },
    /// `route:<path>` — host-local navigation; `<path>` is a route
    /// template interpolated over the invoke context.
    Route {
        /// The route template (may contain `{…}` placeholders).
        template: String,
    },
    /// `host.<builtin>` — a host capability (see [`BUILTINS`]).
    Host(HostBuiltin),
    /// `driver:<name>:<id>` — an action on a *named* driver
    /// (cross-driver; v1 resolves it against that driver's manifest
    /// when the host knows it, else [`Resolved::Unsupported`]).
    Driver {
        /// Driver name — the `<type>` in `tamias-driver-<type>`.
        driver: String,
        /// The action id on that driver.
        id: String,
    },
}

impl Command {
    /// Parse the command grammar — `None` for anything outside the
    /// four namespaces (unknown namespaces degrade to display-only;
    /// hosts keep the raw string for labels).
    #[must_use]
    pub fn parse(spec: &str) -> Option<Self> {
        let s = spec.trim();
        if s.is_empty() {
            return None;
        }
        if let Some(rest) = s.strip_prefix("action:") {
            return (!rest.is_empty()).then(|| Self::Action {
                id: rest.to_string(),
            });
        }
        if let Some(rest) = s.strip_prefix("route:") {
            return (!rest.is_empty()).then(|| Self::Route {
                template: rest.to_string(),
            });
        }
        if let Some(rest) = s.strip_prefix("driver:") {
            let (driver, id) = rest.split_once(':')?;
            if driver.is_empty() || id.is_empty() {
                return None;
            }
            return Some(Self::Driver {
                driver: driver.to_string(),
                id: id.to_string(),
            });
        }
        if let Some(name) = s.strip_prefix("host.") {
            return HostBuiltin::parse(name).map(Self::Host);
        }
        None
    }

    /// The canonical command-id string — round-trips through
    /// [`Command::parse`]. This is the key commands are bound by in
    /// user keymaps (`keys.toml`).
    #[must_use]
    pub fn id(&self) -> String {
        match self {
            Self::Action { id } => format!("action:{id}"),
            Self::Route { template } => format!("route:{template}"),
            Self::Host(b) => b.id().to_string(),
            Self::Driver { driver, id } => format!("driver:{driver}:{id}"),
        }
    }
}

impl std::fmt::Display for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.id())
    }
}

/// What [`resolve_command`] needs to know — everything is borrowed
/// so hosts can rebuild it cheaply per keypress/palette-open.
#[derive(Clone, Copy)]
pub struct ResolveContext<'a> {
    /// Current driver kind (the `<type>` in `tamias-driver-<type>`)
    /// — the default dispatch target for `action:` commands.
    pub driver: &'a str,
    /// Current driver's manifest — action lookup (manifest scope)
    /// and view fallback.
    pub manifest: &'a DriverManifest,
    /// Current [`ViewResult`] when a view is resolved — enables
    /// node/item-scoped action lookup and `data` in interpolation.
    pub view: Option<&'a ViewResult>,
    /// Manifests of *all* discovered drivers (including the current
    /// one) — the lookup table for `driver:<name>:<id>` commands.
    /// Hosts build this once per session from [`crate::Host`].
    pub drivers: &'a BTreeMap<String, DriverManifest>,
    /// The invoke `target` JSON (selected row, item id object, …) —
    /// becomes `target` on dispatches and `{target.*}` in templates.
    pub target: Option<&'a Value>,
    /// Collected invoke params — `{params.*}` in templates, carried
    /// verbatim on dispatches.
    pub params: &'a Params,
}

/// The JSON-ish interpolation context — what [`interpolate`]
/// resolves `{a.b}` paths against. Built by
/// [`ResolveContext::interpolate_ctx`]; hosts interpolating
/// `item_route`/`ActionResult.route` should use the same shape.
///
/// Keys present (when their source exists):
///
/// - `driver` — current driver name (string)
/// - `view` — current view id (string)
/// - `data` — the view's query-binding data map
/// - `target` — the invoke target JSON (verbatim)
/// - `params` — the invoke params map
/// - `item` — the selected [`TreeItem`] as JSON (see
///   [`item_value`]) — only when the caller supplies one
#[must_use]
pub fn interpolate_ctx_value(
    driver: &str,
    view: Option<&ViewResult>,
    target: Option<&Value>,
    params: &Params,
    item: Option<&Value>,
) -> Value {
    let mut map = Map::new();
    map.insert("driver".into(), Value::from(driver));
    if let Some(vr) = view {
        map.insert("view".into(), Value::from(vr.view.id.clone()));
        map.insert("data".into(), Value::Object(vr.data.clone()));
    }
    if let Some(t) = target {
        map.insert("target".into(), t.clone());
    }
    map.insert("params".into(), Value::Object(params.clone()));
    if let Some(i) = item {
        map.insert("item".into(), i.clone());
    }
    Value::Object(map)
}

impl ResolveContext<'_> {
    /// The interpolation context for this resolve — see
    /// [`interpolate_ctx_value`]. `item` is a [`item_value`]-shaped
    /// JSON (or any JSON the host wants addressable as `{item.*}`).
    #[must_use]
    pub fn interpolate_ctx(&self, item: Option<&Value>) -> Value {
        interpolate_ctx_value(self.driver, self.view, self.target, self.params, item)
    }
}

/// A [`TreeItem`] as interpolation JSON — `{item.id}`,
/// `{item.label}`, `{item.detail}`, `{item.icon}`, plus every
/// `props` key flattened to top level (`{item.priority}` works when
/// the item carries `props.priority`). Standard fields win over
/// same-named props. `children`/`actions` are *not* included.
#[must_use]
pub fn item_value(item: &TreeItem) -> Value {
    let mut map = Map::new();
    for (k, v) in &item.props {
        map.insert(k.clone(), v.clone());
    }
    if let Some(id) = &item.id {
        map.insert("id".into(), Value::from(id.clone()));
    }
    map.insert("label".into(), Value::from(item.label.clone()));
    if let Some(d) = &item.detail {
        map.insert("detail".into(), Value::from(d.clone()));
    }
    if let Some(i) = &item.icon {
        map.insert("icon".into(), Value::from(i.clone()));
    }
    Value::Object(map)
}

/// Interpolate a route template — `{path.to.value}` placeholders
/// resolved against `ctx` (see [`interpolate_ctx_value`] for the
/// shape). Rules:
///
/// - Paths are dot-separated; each segment indexes an object key or
///   an array position (`{items.0.id}`).
/// - Missing keys, `null`s, and non-container traversals interpolate
///   to the empty string.
/// - Values render as: strings verbatim, numbers/bools via their
///   JSON spelling, arrays/objects as compact JSON.
/// - `{{` and `}}` escape literal braces. No nesting inside `{…}`.
/// - A lone `{` or `}` (unmatched, unescaped) stays literal.
#[must_use]
pub fn interpolate(template: &str, ctx: &Value) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(pos) = rest.find(['{', '}']) {
        out.push_str(&rest[..pos]);
        let tail = &rest[pos..];
        if let Some(stripped) = tail.strip_prefix("{{") {
            out.push('{');
            rest = stripped;
            continue;
        }
        if let Some(stripped) = tail.strip_prefix("}}") {
            out.push('}');
            rest = stripped;
            continue;
        }
        if let Some(stripped) = tail.strip_prefix('}') {
            out.push('}');
            rest = stripped;
            continue;
        }
        // '{' — find the closing '}'.
        match tail[1..].find('}') {
            Some(end) => {
                let path = tail[1..1 + end].trim();
                out.push_str(&lookup(ctx, path));
                rest = &tail[1 + end + 1..];
            }
            None => {
                out.push('{');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Dot-path lookup over a JSON value — see [`interpolate`].
fn lookup(ctx: &Value, path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    let mut cur = ctx;
    for seg in path.split('.') {
        cur = match cur {
            Value::Object(map) => match map.get(seg) {
                Some(v) => v,
                None => return String::new(),
            },
            Value::Array(arr) => match seg.parse::<usize>().ok().and_then(|i| arr.get(i)) {
                Some(v) => v,
                None => return String::new(),
            },
            _ => return String::new(),
        };
    }
    value_string(cur)
}

/// JSON value → interpolated string.
fn value_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// The outcome of [`resolve_command`] — what the host executes.
#[derive(Clone, Debug)]
pub enum Resolved {
    /// Dispatch `action` on `driver` via the `invoke` wire op.
    /// Boxed — [`ResolvedInvoke`] is large relative to the other
    /// variants and resolutions are produced per activation.
    Dispatch(Box<ResolvedInvoke>),
    /// Navigate locally to `route` (already interpolated).
    Navigate {
        /// The interpolated route (`/myco/item?id=T-1`).
        route: String,
    },
    /// A host builtin — filter/help/reload/editor/palette/quit.
    Builtin(HostBuiltin),
    /// Parsed but unresolvable here (unknown driver, …) — render
    /// display-only.
    Unsupported,
}

/// A resolved `invoke` dispatch — every field ready for
/// [`crate::Host::invoke`] (or the driver's `serve` directly).
#[derive(Clone, Debug)]
pub struct ResolvedInvoke {
    /// Driver kind to invoke on.
    pub driver: String,
    /// View context for the invoke request (`None` → host falls
    /// back like [`crate::Host::invoke`]: target's view, then the
    /// driver's first view).
    pub view: Option<String>,
    /// The action id to send.
    pub action: String,
    /// The declared [`Action`] when found in scope — hosts use it
    /// for `params`/`danger` UX (auto-form, confirm gates).
    /// `None` when the id isn't declared locally (ops-derived
    /// actions the host didn't collect, or driver-side ids) —
    /// dispatch anyway; the driver answers `ok:false` on unknowns.
    pub declared: Option<Action>,
    /// The invoke `target` JSON — `{"id": <item>}` for item-scoped
    /// actions, else the context's target.
    pub target: Option<Value>,
    /// The invoke `params` map.
    pub params: Params,
}

/// An action found by scope lookup, owned (collect borrows the view
/// result; resolutions outlive the borrow).
struct FoundAction {
    action: Action,
    view: Option<String>,
    item_id: Option<String>,
}

fn scope_rank(scope: ActionScope) -> u8 {
    match scope {
        ActionScope::Item => 3,
        ActionScope::Node => 2,
        ActionScope::Manifest => 1,
    }
}

/// Find `id` among collected scoped actions — highest scope
/// precedence wins (item > node > manifest), first occurrence kept
/// on ties (same policy as host-side dedupe).
fn lookup_scoped(scoped: &[ScopedAction<'_>], id: &str) -> Option<FoundAction> {
    let mut best: Option<&ScopedAction<'_>> = None;
    for s in scoped {
        if s.action.id != id {
            continue;
        }
        match best {
            Some(b) if scope_rank(s.scope) <= scope_rank(b.scope) => {}
            _ => best = Some(s),
        }
    }
    best.map(|s| FoundAction {
        action: s.action.clone().into_owned(),
        view: s.view.clone(),
        item_id: s.item_id.as_deref().map(str::to_string),
    })
}

/// Look up `id` in `manifest` + optional `view_result` — node and
/// item actions need the view; manifest-only lookups cover
/// cross-driver resolves (no foreign view result available).
fn lookup_action(
    manifest: &DriverManifest,
    view: Option<&ViewResult>,
    id: &str,
) -> Option<FoundAction> {
    match view {
        Some(vr) => lookup_scoped(&collect_actions(manifest, vr), id),
        None => manifest
            .actions
            .iter()
            .find(|a| a.id == id)
            .map(|a| FoundAction {
                action: a.clone(),
                view: None,
                item_id: None,
            }),
    }
}

/// Resolve an action id on `driver`'s manifest — shared by
/// `action:` (current driver + view) and `driver:` (named driver,
/// manifest scope only unless it's the current one).
fn resolve_action(
    driver: &str,
    manifest: &DriverManifest,
    view: Option<&ViewResult>,
    id: &str,
    ctx: &ResolveContext<'_>,
) -> Resolved {
    let found = lookup_action(manifest, view, id);
    // An action declaring `route` navigates instead of dispatching —
    // interpolate over the invoke context (item = {"id":…} for
    // item-scoped finds).
    if let Some(f) = &found {
        if let Some(template) = &f.action.route {
            let item = f.item_id.as_ref().map(|i| json!({"id": i}));
            return Resolved::Navigate {
                route: interpolate(template, &ctx.interpolate_ctx(item.as_ref())),
            };
        }
    }
    let (declared, target, view_id) = match found {
        Some(f) => (
            Some(f.action),
            f.item_id
                .map(|i| json!({"id": i}))
                .or_else(|| ctx.target.cloned()),
            f.view.or_else(|| view.map(|v| v.view.id.clone())),
        ),
        None => (None, ctx.target.cloned(), view.map(|v| v.view.id.clone())),
    };
    Resolved::Dispatch(Box::new(ResolvedInvoke {
        driver: driver.to_string(),
        view: view_id,
        action: id.to_string(),
        declared,
        target,
        params: ctx.params.clone(),
    }))
}

/// Resolve a [`Command`] against the current context — the single
/// entry point hosts call for palette activation, keybindings, and
/// menu entries. See the module docs for the [`Resolved`] variants.
#[must_use]
pub fn resolve_command(command: &Command, ctx: &ResolveContext<'_>) -> Resolved {
    match command {
        Command::Host(b) => Resolved::Builtin(*b),
        Command::Route { template } => Resolved::Navigate {
            route: interpolate(template, &ctx.interpolate_ctx(None)),
        },
        Command::Action { id } => resolve_action(ctx.driver, ctx.manifest, ctx.view, id, ctx),
        Command::Driver { driver, id } => {
            if driver == ctx.driver {
                // Same driver — full scope (node/item actions too).
                resolve_action(driver, ctx.manifest, ctx.view, id, ctx)
            } else {
                match ctx.drivers.get(driver) {
                    // Foreign driver: manifest scope only — the
                    // host has no ViewResult for it.
                    Some(manifest) => resolve_action(driver, manifest, None, id, ctx),
                    None => Resolved::Unsupported,
                }
            }
        }
    }
}

/// The command a [`MenuEntry`] resolves to — `command` wins when
/// present; else the legacy pair: `action` (`"<id>"` →
/// `action:<id>`, `"<driver>:<id>"` → `driver:<driver>:<id>`) or
/// `route` (`route:<path>`). `None` = display-only entry.
#[must_use]
pub fn menu_command(entry: &MenuEntry) -> Option<Command> {
    if let Some(raw) = &entry.command {
        if let Some(cmd) = Command::parse(raw) {
            return Some(cmd);
        }
        // Unparseable command — fall through to legacy fields rather
        // than dropping the entry entirely.
    }
    if let Some(action) = &entry.action {
        if let Some((driver, id)) = action.split_once(':') {
            if !driver.is_empty() && !id.is_empty() {
                return Some(Command::Driver {
                    driver: driver.to_string(),
                    id: id.to_string(),
                });
            }
        }
        if !action.is_empty() {
            return Some(Command::Action { id: action.clone() });
        }
    }
    entry
        .route
        .as_ref()
        .and_then(|r| Command::parse(&format!("route:{r}")))
}

/// Resolve a menu entry end-to-end: [`menu_command`] +
/// [`resolve_command`]. `None` when the entry is display-only.
#[must_use]
pub fn resolve_menu_entry(entry: &MenuEntry, ctx: &ResolveContext<'_>) -> Option<Resolved> {
    menu_command(entry).map(|cmd| resolve_command(&cmd, ctx))
}

/// A bindable command + display metadata — input to
/// [`effective_bindings`].
#[derive(Clone, Debug)]
pub struct BindingSource<'a> {
    /// What activation resolves to.
    pub command: Command,
    /// Display label (palette/help).
    pub label: Cow<'a, str>,
    /// Secondary text — description or raw command id.
    pub detail: Option<Cow<'a, str>>,
    /// Grouping label (`"actions"`, `"view"`, `"item"`, menu
    /// `section`, `"host"`).
    pub section: Option<Cow<'a, str>>,
    /// Suggested keys — overridden wholesale by a user-keymap entry
    /// for the same command id.
    pub keys: Vec<String>,
}

/// A [`BindingSource`] for a collected [`ScopedAction`] — the
/// command is `action:<id>` locally, or `driver:<drv>:<id>` when the
/// action declares a cross-driver `driver`.
#[must_use]
pub fn action_binding_source<'a>(scoped: &'a ScopedAction<'a>) -> BindingSource<'a> {
    let action = scoped.action.as_ref();
    let command = match &action.driver {
        Some(driver) => Command::Driver {
            driver: driver.clone(),
            id: action.id.clone(),
        },
        None => Command::Action {
            id: action.id.clone(),
        },
    };
    BindingSource {
        command,
        label: Cow::Borrowed(action.label.as_str()),
        detail: action
            .description
            .as_deref()
            .map(Cow::Borrowed)
            .or(Some(Cow::Borrowed(action.id.as_str()))),
        section: Some(Cow::Owned(match scoped.scope {
            ActionScope::Manifest => "actions".to_string(),
            ActionScope::Node => "view".to_string(),
            ActionScope::Item => "item".to_string(),
        })),
        keys: action.keys.clone(),
    }
}

/// A [`BindingSource`] for a [`MenuEntry`] — `None` for
/// display-only entries (no resolvable command).
#[must_use]
pub fn menu_binding_source(entry: &MenuEntry) -> Option<BindingSource<'_>> {
    let command = menu_command(entry)?;
    let detail = command.id();
    Some(BindingSource {
        command,
        label: Cow::Borrowed(entry.title.as_str()),
        detail: Some(Cow::Owned(detail)),
        section: entry.section.as_deref().map(Cow::Borrowed),
        keys: entry.keys.clone(),
    })
}

/// [`BindingSource`]s for every [`BUILTINS`] row — section `"host"`.
#[must_use]
pub fn builtin_binding_sources() -> Vec<BindingSource<'static>> {
    BUILTINS
        .iter()
        .map(|b| BindingSource {
            command: Command::Host(b.builtin),
            label: Cow::Borrowed(b.label),
            detail: Some(Cow::Borrowed(b.description)),
            section: Some(Cow::Borrowed("host")),
            keys: b.keys.iter().map(|k| k.to_string()).collect(),
        })
        .collect()
}

/// One merged key→command binding — a row of the host's effective
/// keymap (help screen, `?` overlay, conflict detection).
#[derive(Clone, Debug)]
pub struct CommandBinding {
    /// Canonical command id (`action:x`, `host.filter`, …).
    pub id: String,
    /// The resolved command.
    pub command: Command,
    /// Display label.
    pub label: String,
    /// Secondary text.
    pub detail: Option<String>,
    /// Grouping label.
    pub section: Option<String>,
    /// Effective keys — the user-keymap override when present, else
    /// the declaration's suggestions. Validated through
    /// [`crate::keymap::KeySpec`]; invalid overlay entries drop out.
    pub keys: Vec<String>,
}

/// Merge declared/suggested bindings with a user-keymap overlay.
///
/// `overlay` maps **command id → keys**: the value is a
/// comma-separated list of [`crate::keymap::KeySpec`]s
/// (`"ctrl+p,/"`), or a single spec. An overlay entry *replaces* the
/// declaration's suggested keys wholesale; command ids absent from
/// the overlay keep their suggestions. Sources producing the same
/// command id dedupe to the first occurrence.
///
/// Returns the list sorted by command id — deterministic for help
/// screens and tests.
#[must_use]
pub fn effective_bindings<'a>(
    sources: impl IntoIterator<Item = BindingSource<'a>>,
    overlay: &BTreeMap<String, String>,
) -> Vec<CommandBinding> {
    let mut out: Vec<CommandBinding> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for src in sources {
        let id = src.command.id();
        if !seen.insert(id.clone()) {
            continue;
        }
        let keys = match overlay.get(&id) {
            Some(spec) => spec
                .split(',')
                .filter_map(|k| {
                    let k = k.trim();
                    (!k.is_empty()).then(|| k.to_string())
                })
                .filter(|k| crate::keymap::KeySpec::parse(k).is_some())
                .collect(),
            None => src.keys,
        };
        out.push(CommandBinding {
            id,
            command: src.command,
            label: src.label.into_owned(),
            detail: src.detail.map(Cow::into_owned),
            section: src.section.map(Cow::into_owned),
            keys,
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use pavo_protocol::wire::ViewResult;
    use pavo_protocol::ViewSpec;
    use serde_json::json;

    fn manifest(actions: Vec<Action>) -> DriverManifest {
        DriverManifest {
            name: "demo".into(),
            title: "Demo".into(),
            icon: None,
            version: "0".into(),
            verbs: vec![],
            capabilities: vec![],
            deps: None,
            contributes: Default::default(),
            views: vec![],
            renderers: vec![],
            actions,
        }
    }

    fn action(id: &str) -> Action {
        Action {
            id: id.into(),
            label: id.into(),
            icon: None,
            danger: Default::default(),
            keys: vec![],
            params: vec![],
            target: None,
            description: None,
            driver: None,
            route: None,
        }
    }

    fn view_result(root: pavo_protocol::ViewNode) -> ViewResult {
        ViewResult {
            view: ViewSpec {
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

    fn ctx<'a>(
        driver: &'a str,
        manifest: &'a DriverManifest,
        view: Option<&'a ViewResult>,
        drivers: &'a BTreeMap<String, DriverManifest>,
        target: Option<&'a Value>,
        params: &'a Params,
    ) -> ResolveContext<'a> {
        ResolveContext {
            driver,
            manifest,
            view,
            drivers,
            target,
            params,
        }
    }

    // ---------- Command::parse / id ----------

    #[test]
    fn command_parse_and_id_roundtrip() {
        for (raw, id) in [
            ("action:todo.done", "action:todo.done"),
            (
                "route:/myco/item?id={item.id}",
                "route:/myco/item?id={item.id}",
            ),
            ("host.filter", "host.filter"),
            ("driver:myco:item.reorder", "driver:myco:item.reorder"),
        ] {
            let c = Command::parse(raw).unwrap();
            assert_eq!(c.id(), id);
            assert_eq!(Command::parse(&c.id()).unwrap(), c);
        }
        assert!(matches!(
            Command::parse("action:x"),
            Some(Command::Action { id }) if id == "x"
        ));
        assert!(matches!(
            Command::parse("host.quit"),
            Some(Command::Host(HostBuiltin::Quit))
        ));
        assert!(matches!(
            Command::parse("driver:d:a"),
            Some(Command::Driver { driver, id }) if driver == "d" && id == "a"
        ));
        // Rejections / unknown namespaces.
        for bad in [
            "",
            "  ",
            "action:",
            "route:",
            "driver:x",
            "driver::y",
            "driver:x:",
            "host.nope",
            "bogus:thing",
            "menu:1",
        ] {
            assert!(Command::parse(bad).is_none(), "{bad} should not parse");
        }
    }

    // ---------- interpolate ----------

    #[test]
    fn interpolate_paths_scalars_and_escapes() {
        let ctx = json!({
            "driver": "myco",
            "view": "board",
            "target": {"id": "T-1", "nested": {"x": 2}},
            "params": {"q": "open", "n": 3},
            "item": {"id": "I-9", "label": "nine", "priority": 1},
            "data": {"items": [{"id": "a"}, {"id": "b"}]}
        });
        assert_eq!(interpolate("/i?id={target.id}", &ctx), "/i?id=T-1");
        assert_eq!(interpolate("{target.nested.x}", &ctx), "2");
        assert_eq!(interpolate("{params.q}/{params.n}", &ctx), "open/3");
        assert_eq!(interpolate("{item.id}:{item.priority}", &ctx), "I-9:1");
        assert_eq!(interpolate("{data.items.1.id}", &ctx), "b");
        // Missing keys / nulls → empty; literal braces via escapes.
        assert_eq!(interpolate("[{missing.x}]", &ctx), "[]");
        assert_eq!(
            interpolate("{{literal}} {target.id}", &ctx),
            "{literal} T-1"
        );
        // Lone braces stay literal.
        assert_eq!(interpolate("{oops", &ctx), "{oops");
        assert_eq!(interpolate("a}b", &ctx), "a}b");
        // Objects/arrays render as compact JSON.
        assert_eq!(
            interpolate("{target}", &ctx),
            r#"{"id":"T-1","nested":{"x":2}}"#
        );
    }

    #[test]
    fn item_value_flattens_props() {
        let item: TreeItem = serde_json::from_value(json!({
            "id":"T-1","label":"Task","detail":"d","icon":"i",
            "props":{"priority":"high","id":"prop-loses"}
        }))
        .unwrap();
        let v = item_value(&item);
        assert_eq!(v["id"], "T-1"); // standard field beats prop
        assert_eq!(v["priority"], "high");
        assert_eq!(v["label"], "Task");
        assert!(v.get("children").is_none());
        assert!(v.get("actions").is_none());
    }

    // ---------- resolve_command ----------

    #[test]
    fn resolve_builtin_route_and_action() {
        let m = manifest(vec![action("refresh")]);
        let drivers = BTreeMap::from([("demo".to_string(), m.clone())]);
        let params = Params::new();
        let c = ctx("demo", &m, None, &drivers, None, &params);

        assert!(matches!(
            resolve_command(&Command::parse("host.help").unwrap(), &c),
            Resolved::Builtin(HostBuiltin::Help)
        ));
        let r = resolve_command(
            &Command::parse("route:/x?id={target.id}").unwrap(),
            &ctx(
                "demo",
                &m,
                None,
                &drivers,
                Some(&json!({"id":"T"})),
                &params,
            ),
        );
        assert!(matches!(r, Resolved::Navigate { route } if route == "/x?id=T"));

        let r = resolve_command(&Command::parse("action:refresh").unwrap(), &c);
        match r {
            Resolved::Dispatch(d) => {
                assert_eq!(d.driver, "demo");
                assert_eq!(d.action, "refresh");
                assert!(d.declared.is_some());
                assert_eq!(d.view, None);
            }
            other => panic!("expected dispatch, got {other:?}"),
        }
        // Undeclared ids still dispatch (ops-derived / driver-side).
        let r = resolve_command(&Command::parse("action:mystery").unwrap(), &c);
        assert!(matches!(r, Resolved::Dispatch(d) if d.declared.is_none()));
    }

    #[test]
    fn resolve_action_route_navigates_and_scopes() {
        // Manifest action with route → Navigate, interpolated.
        let mut nav = action("open");
        nav.route = Some("/demo/item?id={target.id}".into());
        let m = manifest(vec![nav]);
        let drivers = BTreeMap::from([("demo".to_string(), m.clone())]);
        let params = Params::new();
        let target = json!({"id": "T-7"});
        let c = ctx("demo", &m, None, &drivers, Some(&target), &params);
        let r = resolve_command(&Command::parse("action:open").unwrap(), &c);
        assert!(matches!(r, Resolved::Navigate { route } if route == "/demo/item?id=T-7"));

        // Item-scoped action resolves with {"id": item} target.
        let root: pavo_protocol::ViewNode = serde_json::from_value(json!({
            "kind":"tree","items":[{"id":"I-1","label":"one",
                "actions":[{"id":"archive","label":"Archive"}]}]
        }))
        .unwrap();
        let vr = view_result(root);
        let c2 = ctx("demo", &m, Some(&vr), &drivers, None, &params);
        let r = resolve_command(&Command::parse("action:archive").unwrap(), &c2);
        match r {
            Resolved::Dispatch(d) => {
                assert_eq!(d.view.as_deref(), Some("v"));
                assert_eq!(d.target, Some(json!({"id": "I-1"})));
                assert!(d.declared.is_some());
            }
            other => panic!("expected dispatch, got {other:?}"),
        }
    }

    #[test]
    fn resolve_driver_command_scopes() {
        let local = manifest(vec![action("local")]);
        let mut foreign_actions = vec![action("ping")];
        foreign_actions[0].route = None;
        let foreign = DriverManifest {
            name: "other".into(),
            ..manifest(foreign_actions)
        };
        let drivers = BTreeMap::from([
            ("demo".to_string(), local.clone()),
            ("other".to_string(), foreign.clone()),
        ]);
        let params = Params::new();
        let c = ctx("demo", &local, None, &drivers, None, &params);

        // Foreign driver, manifest scope.
        let r = resolve_command(&Command::parse("driver:other:ping").unwrap(), &c);
        assert!(matches!(
            r,
            Resolved::Dispatch(d) if d.driver == "other" && d.action == "ping"
        ));
        // Same driver via driver: gets full view scope.
        let r = resolve_command(&Command::parse("driver:demo:local").unwrap(), &c);
        assert!(matches!(r, Resolved::Dispatch(d) if d.driver == "demo"));
        // Unknown driver → Unsupported.
        let r = resolve_command(&Command::parse("driver:ghost:x").unwrap(), &c);
        assert!(matches!(r, Resolved::Unsupported));
    }

    // ---------- menu compat ----------

    #[test]
    fn menu_command_compat_precedence() {
        let entry = |command: Option<&str>, action: Option<&str>, route: Option<&str>| MenuEntry {
            id: "m".into(),
            title: "M".into(),
            icon: None,
            action: action.map(str::to_string),
            route: route.map(str::to_string),
            command: command.map(str::to_string),
            keys: vec![],
            keywords: vec![],
            section: None,
            priority: 100,
        };
        // command wins over action/route.
        let c = menu_command(&entry(Some("host.quit"), Some("x"), Some("/y"))).unwrap();
        assert_eq!(c, Command::Host(HostBuiltin::Quit));
        // Unparseable command falls back to legacy fields.
        let c = menu_command(&entry(Some("bogus!"), Some("todo.add"), None)).unwrap();
        assert_eq!(
            c,
            Command::Action {
                id: "todo.add".into()
            }
        );
        // Legacy action forms.
        assert_eq!(
            menu_command(&entry(None, Some("todo.add"), None)).unwrap(),
            Command::Action {
                id: "todo.add".into()
            }
        );
        assert_eq!(
            menu_command(&entry(None, Some("myco:item.reorder"), None)).unwrap(),
            Command::Driver {
                driver: "myco".into(),
                id: "item.reorder".into()
            }
        );
        // Route fallback.
        assert_eq!(
            menu_command(&entry(None, None, Some("/myco/board"))).unwrap(),
            Command::Route {
                template: "/myco/board".into()
            }
        );
        // Display-only.
        assert!(menu_command(&entry(None, None, None)).is_none());
    }

    // ---------- bindings ----------

    #[test]
    fn effective_bindings_merges_overlay() {
        let mut a = action("refresh");
        a.keys = vec!["r".into()];
        let scoped = ScopedAction {
            action: Cow::Owned(a),
            scope: ActionScope::Manifest,
            view: Some("v".into()),
            item_id: None,
        };
        let menu = MenuEntry {
            id: "m".into(),
            title: "M".into(),
            icon: None,
            action: Some("todo.add".into()),
            route: None,
            command: None,
            keys: vec!["n".into()],
            keywords: vec![],
            section: Some("myco".into()),
            priority: 100,
        };
        let sources = vec![
            action_binding_source(&scoped),
            menu_binding_source(&menu).unwrap(),
        ]
        .into_iter()
        .chain(builtin_binding_sources());

        let mut overlay = BTreeMap::new();
        overlay.insert("action:refresh".to_string(), "ctrl+r".to_string());
        overlay.insert(
            "host.filter".to_string(),
            "/,ctrl+f".to_string(), // comma list; both valid
        );
        overlay.insert("host.quit".to_string(), "not a key!!".to_string()); // all invalid → empty

        let bindings = effective_bindings(sources, &overlay);
        let by_id: BTreeMap<_, _> = bindings.iter().map(|b| (b.id.as_str(), b)).collect();
        assert_eq!(by_id["action:refresh"].keys, vec!["ctrl+r"]);
        assert_eq!(by_id["action:todo.add"].keys, vec!["n"]);
        assert_eq!(by_id["host.filter"].keys, vec!["/", "ctrl+f"]);
        assert!(by_id["host.quit"].keys.is_empty()); // invalid overlay → no keys
        assert!(by_id.contains_key("host.help"));
        // Sorted by command id.
        let ids: Vec<&str> = bindings.iter().map(|b| b.id.as_str()).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(ids, sorted);
    }
}
