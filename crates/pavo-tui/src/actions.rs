//! Interaction actions (P-032) — the TUI's side of the `invoke`
//! protocol.
//!
//! [`UiAction`] is the owned form of [`pavo_core::ScopedAction`] the
//! worker ships to the app (scoped actions borrow the view result;
//! the UI needs 'static data across frames). [`PaletteEntry`],
//! [`PaletteSession`], [`ConfirmSession`] and [`TypeTargetSession`]
//! are the overlay sessions; [`action_form_spec`] compiles an
//! action's `params` into the existing [`FormSpec`]/[`FormSession`]
//! machinery so auto-forms reuse the form editor verbatim.
//!
//! Dispatch flow ([`InvokeStep`]): params → auto-form; on submit the
//! collected values are [`coerce_params`]-ed and
//! [`pavo_core::validate_params`]-checked, then the danger gate runs
//! — `none` invokes immediately, `confirm` opens [`ConfirmSession`],
//! `type-target` opens [`TypeTargetSession`].

use std::collections::HashMap;
use std::time::Duration;

use crossterm::event::KeyCode;
use pavo_core::{param_form_field, ActionScope, Params};
use pavo_protocol::{Action, DangerLevel, FormFieldKind, MenuEntry};
use serde_json::Value;

use crate::form::LineEdit;
use crate::model::{FormFieldSpec, FormSpec};

/// How long a pending chord prefix waits for its second key before
/// the buffered char replays as a normal keypress.
pub const CHORD_TIMEOUT: Duration = Duration::from_millis(600);

/// An [`Action`] plus everything the TUI needs to dispatch it —
/// owned clone of `pavo_core::ScopedAction` (which borrows the view
/// result the worker collected it from).
#[derive(Clone, Debug)]
pub struct UiAction {
    /// The action declaration.
    pub action: Action,
    /// Which declaration site it came from (keybinding + palette
    /// grouping context).
    pub scope: ActionScope,
    /// Namespaced `driver:view` — the `invoke` op's `target` arg
    /// (driver resolution + default view context).
    pub target: String,
    /// Bare view id — the invoke request's `view` context override.
    pub view: Option<String>,
    /// Tree item id for [`ActionScope::Item`] actions — becomes the
    /// invoke `target` payload `{"id": …}`.
    pub item_id: Option<String>,
}

impl UiAction {
    /// The opaque `target` JSON for the invoke request: tree-item
    /// actions carry `{"id": <item_id>}` (the protocol README's
    /// example shape); everything else sends no target — the driver
    /// resolves node/manifest actions from the `view` context.
    #[must_use]
    pub fn action_target(&self) -> Option<Value> {
        self.item_id
            .as_ref()
            .map(|id| serde_json::json!({"id": id}))
    }
}

/// Scope precedence for id collisions — item > node > manifest, per
/// `pavo_core::actions::collect_actions` docs.
fn scope_rank(scope: ActionScope) -> u8 {
    match scope {
        ActionScope::Item => 3,
        ActionScope::Node => 2,
        ActionScope::Manifest => 1,
    }
}

/// Dedupe collected actions by `action.id`, keeping the
/// highest-precedence scope (item > node > manifest). The surviving
/// entry keeps the position of its id's first occurrence, so
/// declaration order still drives display/keybinding order.
#[must_use]
pub fn dedupe_ui_actions(actions: Vec<UiAction>) -> Vec<UiAction> {
    let mut out: Vec<UiAction> = Vec::with_capacity(actions.len());
    let mut index: HashMap<String, usize> = HashMap::new();
    for a in actions {
        match index.get(&a.action.id) {
            Some(&i) if scope_rank(out[i].scope) >= scope_rank(a.scope) => {}
            Some(&i) => out[i] = a,
            None => {
                index.insert(a.action.id.clone(), out.len());
                out.push(a);
            }
        }
    }
    out
}

/// What happens when an action is activated — the pure decision the
/// app executes. `Form` means "open the auto-form first"; the other
/// variants are the post-params danger gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvokeStep {
    /// Params present — compile the auto-form; danger is re-checked
    /// after the form submits.
    Form,
    /// No params, `danger: none` — invoke immediately.
    Invoke,
    /// `danger: confirm` — confirm modal, then invoke.
    Confirm,
    /// `danger: type-target` — type-the-target-id modal, then invoke.
    TypeTarget,
}

/// The entry-point decision: params always win (the form collects
/// them first; danger gates the collected invoke afterwards).
#[must_use]
pub fn plan_action(action: &Action) -> InvokeStep {
    if !action.params.is_empty() {
        return InvokeStep::Form;
    }
    danger_step(action.danger)
}

/// The post-form decision — same as [`plan_action`] minus the form
/// branch (params were already collected).
#[must_use]
pub fn danger_step(danger: DangerLevel) -> InvokeStep {
    match danger {
        DangerLevel::None => InvokeStep::Invoke,
        DangerLevel::Confirm => InvokeStep::Confirm,
        DangerLevel::TypeTarget => InvokeStep::TypeTarget,
    }
}

/// An action awaiting (or having passed) the danger gate — what the
/// confirm/type-target sessions carry and what `Cmd::Invoke`
/// eventually sends.
#[derive(Clone, Debug)]
pub struct PendingInvoke {
    /// The action + dispatch context.
    pub ui: UiAction,
    /// Collected `ParamSchema.name` → value (empty for
    /// parameterless actions).
    pub params: Params,
    /// The opaque target JSON actually sent — the type-target
    /// session derives its expected id from it.
    pub target: Option<Value>,
}

impl PendingInvoke {
    /// Assemble from a [`UiAction`] and collected params.
    #[must_use]
    pub fn new(ui: UiAction, params: Params) -> Self {
        let target = ui.action_target();
        Self { ui, params, target }
    }
}

/// Extract the id a `type-target` session expects the user to type:
/// a bare string target is itself the id; an object target's `"id"`
/// string field wins. `None` → the session accepts any non-empty
/// input (the target shape is driver-defined).
#[must_use]
pub fn target_id(target: &Option<Value>) -> Option<String> {
    match target {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(Value::Object(map)) => map
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        _ => None,
    }
}

/// Coerce a submitted form's `values` into invoke `params`: the
/// `string[]` kind degrades to comma text in the form (per
/// `param_form_field`), so split it back into a JSON array here.
/// Everything else passes through — the form editor already coerces
/// bool/number per field kind.
#[must_use]
pub fn coerce_params(action: &Action, values: Params) -> Params {
    let mut out = values;
    for p in &action.params {
        if p.kind != pavo_protocol::ParamKind::StringList {
            continue;
        }
        if let Some(Value::String(s)) = out.get(&p.name) {
            let items: Vec<Value> = s
                .split(',')
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(|t| Value::String(t.to_string()))
                .collect();
            out.insert(p.name.clone(), Value::Array(items));
        }
    }
    out
}

/// Compile an action's `params` into a [`FormSpec`] for the existing
/// form editor — [`param_form_field`] picks the widget kinds,
/// [`FormFieldSpec`] carries them into [`crate::form::FormSession`].
#[must_use]
pub fn action_form_spec(action: &Action) -> FormSpec {
    let fields = action
        .params
        .iter()
        .map(|p| {
            let f = param_form_field(p);
            FormFieldSpec {
                key: f.key,
                label: f.label,
                kind: kind_name(f.kind).to_string(),
                required: f.required,
                editable: f.editable,
                options: f.options,
                default: f.default,
                value: f.value,
                hint: f.hint,
                placeholder: f.placeholder,
            }
        })
        .collect();
    FormSpec {
        id: None,
        title: action.label.clone(),
        submit_op: "invoke".to_string(),
        submit_label: "Invoke".to_string(),
        fields,
    }
}

/// [`FormFieldKind`] → the kind string [`FormFieldSpec`] carries
/// (same spellings the canvas renderer and editor already use).
fn kind_name(kind: FormFieldKind) -> &'static str {
    match kind {
        FormFieldKind::Text => "text",
        FormFieldKind::Textarea => "textarea",
        FormFieldKind::Number => "number",
        FormFieldKind::Bool => "bool",
        FormFieldKind::Select => "select",
    }
}

// ---------- palette ----------

/// One command-palette row — either a collected [`UiAction`] or a
/// manifest `menus[]` entry (which itself resolves to an action or a
/// route at activation time).
#[derive(Clone, Debug)]
pub enum PaletteEntry {
    /// A declared/derived action — Enter runs the danger/param flow.
    Action(UiAction),
    /// A `contributes.menus[]` entry — `action` invokes (resolved
    /// against collected actions by id), `route` navigates.
    Menu(MenuEntry),
}

impl PaletteEntry {
    /// Display label.
    #[must_use]
    pub fn label(&self) -> &str {
        match self {
            Self::Action(u) => &u.action.label,
            Self::Menu(m) => &m.title,
        }
    }

    /// Secondary line — description, action id, or route.
    #[must_use]
    pub fn detail(&self) -> String {
        match self {
            Self::Action(u) => u
                .action
                .description
                .clone()
                .unwrap_or_else(|| u.action.id.clone()),
            Self::Menu(m) => m
                .action
                .clone()
                .or_else(|| m.route.clone())
                .unwrap_or_else(|| m.id.clone()),
        }
    }

    /// Extra fuzzy-match terms beyond the label.
    fn haystack(&self) -> String {
        let mut s = String::new();
        match self {
            Self::Action(u) => {
                s.push_str(&u.action.id);
                s.push(' ');
                s.push_str(&u.action.label);
                if let Some(d) = &u.action.description {
                    s.push(' ');
                    s.push_str(d);
                }
                for k in &u.action.keys {
                    s.push(' ');
                    s.push_str(k);
                }
            }
            Self::Menu(m) => {
                s.push_str(&m.id);
                s.push(' ');
                s.push_str(&m.title);
                for k in &m.keywords {
                    s.push(' ');
                    s.push_str(k);
                }
            }
        }
        s
    }

    /// Grouping label for section headers (`None` = ungrouped).
    #[must_use]
    pub fn section(&self) -> Option<String> {
        match self {
            Self::Action(u) => Some(match u.scope {
                ActionScope::Manifest => "actions".to_string(),
                ActionScope::Node => "view".to_string(),
                ActionScope::Item => "item".to_string(),
            }),
            Self::Menu(m) => m.section.clone(),
        }
    }
}

/// Case-insensitive subsequence match — the dmenu-style fuzzy
/// filter. Empty query matches everything.
#[must_use]
pub fn fuzzy_match(query: &str, text: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let mut q = query.chars().flat_map(char::to_lowercase);
    let mut want = q.next();
    for c in text.chars().flat_map(char::to_lowercase) {
        match want {
            Some(w) if c == w => want = q.next(),
            _ => {}
        }
        if want.is_none() {
            return true;
        }
    }
    want.is_none()
}

/// Build the palette's entry list: curated `menus[]` first (sorted
/// by `priority`, then title), then the collected actions in
/// declaration order.
#[must_use]
pub fn palette_entries(actions: &[UiAction], menus: &[MenuEntry]) -> Vec<PaletteEntry> {
    let mut ms: Vec<&MenuEntry> = menus.iter().collect();
    ms.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then_with(|| a.title.cmp(&b.title))
    });
    ms.into_iter()
        .map(|m| PaletteEntry::Menu(m.clone()))
        .chain(actions.iter().cloned().map(PaletteEntry::Action))
        .collect()
}

/// What a palette keypress asks the app to do.
#[derive(Clone, Debug)]
pub enum PaletteCmd {
    /// Key consumed, nothing further.
    None,
    /// `Esc` — close without activating.
    Close,
    /// `Enter` — activate this entry (action or menu route).
    Activate(Box<PaletteEntry>),
}

/// The command-palette overlay: a [`LineEdit`] filter over
/// [`palette_entries`], ↑↓/jk selection, Enter activates.
#[derive(Clone, Debug)]
pub struct PaletteSession {
    /// The filter input.
    pub edit: LineEdit,
    /// All entries (pre-sorted — see [`palette_entries`]).
    pub entries: Vec<PaletteEntry>,
    /// Indices into `entries` matching the current filter.
    pub matches: Vec<usize>,
    /// Position within `matches`.
    pub sel: usize,
}

impl PaletteSession {
    /// Open over `entries`; the filter starts empty (everything
    /// matches, in the given order).
    #[must_use]
    pub fn open(entries: Vec<PaletteEntry>) -> Self {
        let mut s = Self {
            edit: LineEdit::default(),
            entries,
            matches: Vec::new(),
            sel: 0,
        };
        s.refilter();
        s
    }

    /// Recompute `matches` from the filter text; selection resets
    /// to the top.
    fn refilter(&mut self) {
        let q = self.edit.text.clone();
        self.matches = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| fuzzy_match(&q, &e.haystack()))
            .map(|(i, _)| i)
            .collect();
        self.sel = 0;
    }

    /// The selected entry, when the filter matched anything.
    #[must_use]
    pub fn current(&self) -> Option<&PaletteEntry> {
        self.matches.get(self.sel).map(|&i| &self.entries[i])
    }

    /// Move the selection `delta` rows through the matches (wraps).
    fn move_sel(&mut self, delta: isize) {
        let n = self.matches.len();
        if n == 0 {
            return;
        }
        self.sel = (self.sel as isize + delta).rem_euclid(n as isize) as usize;
    }

    /// Route one key into the session — always consumes (the overlay
    /// owns the keyboard while open).
    pub fn key(&mut self, code: KeyCode) -> PaletteCmd {
        match code {
            KeyCode::Esc => PaletteCmd::Close,
            KeyCode::Enter => match self.current() {
                Some(e) => PaletteCmd::Activate(Box::new(e.clone())),
                None => PaletteCmd::None,
            },
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_sel(-1);
                PaletteCmd::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_sel(1);
                PaletteCmd::None
            }
            KeyCode::PageUp => {
                self.move_sel(-(self.matches.len() as isize).max(1));
                PaletteCmd::None
            }
            KeyCode::PageDown => {
                self.move_sel(self.matches.len() as isize);
                PaletteCmd::None
            }
            KeyCode::Backspace => {
                self.edit.backspace();
                self.refilter();
                PaletteCmd::None
            }
            KeyCode::Delete => {
                self.edit.delete();
                self.refilter();
                PaletteCmd::None
            }
            KeyCode::Home => {
                self.edit.home();
                PaletteCmd::None
            }
            KeyCode::End => {
                self.edit.end();
                PaletteCmd::None
            }
            KeyCode::Left => {
                self.edit.move_cursor(-1);
                PaletteCmd::None
            }
            KeyCode::Right => {
                self.edit.move_cursor(1);
                PaletteCmd::None
            }
            KeyCode::Char(c) => {
                self.edit.insert(c);
                self.refilter();
                PaletteCmd::None
            }
            _ => PaletteCmd::None,
        }
    }
}

// ---------- confirm / type-target ----------

/// What a confirm-modal keypress asks the app to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfirmCmd {
    /// Key consumed, nothing further.
    None,
    /// `Esc`/`n` — dismiss without invoking.
    Close,
    /// `Enter`/`y` — proceed with the pending invoke.
    Confirm,
}

/// The `danger: confirm` modal — label + description + target hint,
/// y/Enter confirms, n/Esc cancels.
#[derive(Clone, Debug)]
pub struct ConfirmSession {
    /// The invoke waiting on confirmation.
    pub pending: PendingInvoke,
}

impl ConfirmSession {
    /// Wrap `pending` for display.
    #[must_use]
    pub fn new(pending: PendingInvoke) -> Self {
        Self { pending }
    }

    /// Route one key — the modal owns the keyboard while open.
    pub fn key(&mut self, code: KeyCode) -> ConfirmCmd {
        match code {
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => ConfirmCmd::Close,
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => ConfirmCmd::Confirm,
            _ => ConfirmCmd::None,
        }
    }
}

/// What a type-target keypress asks the app to do.
#[derive(Clone, Debug)]
pub enum TypeTargetCmd {
    /// Key consumed, nothing further.
    None,
    /// `Esc` — dismiss without invoking.
    Close,
    /// `Enter` — the typed value matched (or any non-empty value when
    /// no expected id could be derived); proceed.
    Submit,
}

/// The `danger: type-target` modal — "Type <id> to confirm". Proceeds
/// only on an exact match of the expected id; when the target JSON
/// carries no derivable id, any non-empty input confirms (the value
/// is still shown back in the prompt).
#[derive(Clone, Debug)]
pub struct TypeTargetSession {
    /// The invoke waiting on confirmation.
    pub pending: PendingInvoke,
    /// The expected id (`None` → any non-empty input accepted).
    pub expect: Option<String>,
    /// The input buffer.
    pub edit: LineEdit,
    /// Set after a mismatched Enter — shown inline until the next
    /// keystroke.
    pub error: Option<String>,
}

impl TypeTargetSession {
    /// Open over `pending`; derives `expect` from the target JSON.
    #[must_use]
    pub fn new(pending: PendingInvoke) -> Self {
        let expect = target_id(&pending.target);
        Self {
            pending,
            expect,
            edit: LineEdit::default(),
            error: None,
        }
    }

    /// Route one key — the modal owns the keyboard while open.
    pub fn key(&mut self, code: KeyCode) -> TypeTargetCmd {
        match code {
            KeyCode::Esc => TypeTargetCmd::Close,
            KeyCode::Enter => {
                let typed = self.edit.text.trim();
                let ok = match &self.expect {
                    Some(want) => typed == want,
                    None => !typed.is_empty(),
                };
                if ok {
                    TypeTargetCmd::Submit
                } else {
                    self.error = Some(match &self.expect {
                        Some(want) => format!("type {want} exactly to confirm"),
                        None => "type something to confirm".to_string(),
                    });
                    TypeTargetCmd::None
                }
            }
            KeyCode::Backspace => {
                self.edit.backspace();
                self.error = None;
                TypeTargetCmd::None
            }
            KeyCode::Delete => {
                self.edit.delete();
                self.error = None;
                TypeTargetCmd::None
            }
            KeyCode::Home => {
                self.edit.home();
                TypeTargetCmd::None
            }
            KeyCode::End => {
                self.edit.end();
                TypeTargetCmd::None
            }
            KeyCode::Left => {
                self.edit.move_cursor(-1);
                TypeTargetCmd::None
            }
            KeyCode::Right => {
                self.edit.move_cursor(1);
                TypeTargetCmd::None
            }
            KeyCode::Char(c) => {
                self.edit.insert(c);
                self.error = None;
                TypeTargetCmd::None
            }
            _ => TypeTargetCmd::None,
        }
    }
}

// ---------- action key specs ----------

/// Parse an action `keys[]` entry into the char sequence the app
/// matches. Returns `None` for specs the TUI can't produce —
/// modifier chords like `"ctrl+k"` (the key path only sees
/// [`KeyCode::Char`] values; `App::on_key` discards modifiers).
/// Multi-char sequences like `"dd"` become two-key chords via the
/// app's pending-key mechanism.
#[must_use]
pub fn parse_key_spec(spec: &str) -> Option<String> {
    if spec.is_empty() || spec.contains('+') || spec.chars().any(char::is_whitespace) {
        return None;
    }
    Some(spec.to_string())
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use pavo_protocol::ParamKind;
    use serde_json::json;

    fn action(id: &str) -> Action {
        Action {
            id: id.to_string(),
            label: id.to_string(),
            icon: None,
            danger: DangerLevel::None,
            keys: vec![],
            params: vec![],
            target: None,
            description: None,
            driver: None,
        }
    }

    fn ui(id: &str, scope: ActionScope) -> UiAction {
        UiAction {
            action: action(id),
            scope,
            target: "demo:v".to_string(),
            view: Some("v".to_string()),
            item_id: None,
        }
    }

    #[test]
    fn fuzzy_match_is_subsequence_case_insensitive() {
        assert!(fuzzy_match("", "anything"));
        assert!(fuzzy_match("db", "Dashboard Board"));
        assert!(fuzzy_match("DK", "dungeon keeper"));
        assert!(!fuzzy_match("xyz", "dashboard"));
        assert!(fuzzy_match("todo.d", "todo.done — mark done"));
    }

    #[test]
    fn plan_prefers_form_then_gates_on_danger() {
        let mut a = action("x");
        assert_eq!(plan_action(&a), InvokeStep::Invoke);
        a.danger = DangerLevel::Confirm;
        assert_eq!(plan_action(&a), InvokeStep::Confirm);
        a.danger = DangerLevel::TypeTarget;
        assert_eq!(plan_action(&a), InvokeStep::TypeTarget);
        a.params.push(pavo_protocol::ParamSchema {
            name: "n".into(),
            kind: ParamKind::String,
            required: false,
            label: None,
            doc: None,
            hint: None,
            placeholder: None,
            default: None,
            pattern: None,
            min: None,
            max: None,
            options: vec![],
            ref_query: None,
        });
        a.danger = DangerLevel::Confirm;
        assert_eq!(plan_action(&a), InvokeStep::Form);
        assert_eq!(danger_step(a.danger), InvokeStep::Confirm);
    }

    #[test]
    fn target_id_derives_from_object_or_string() {
        assert_eq!(
            target_id(&Some(json!({"id": "T-012"}))),
            Some("T-012".to_string())
        );
        assert_eq!(target_id(&Some(json!("T-012"))), Some("T-012".to_string()));
        assert_eq!(target_id(&Some(json!({"ref": "x"}))), None);
        assert_eq!(target_id(&None), None);
        assert_eq!(target_id(&Some(json!({"id": ""}))), None);
    }

    #[test]
    fn coerce_params_splits_string_lists() {
        let mut a = action("tag");
        a.params.push(pavo_protocol::ParamSchema {
            name: "tags".into(),
            kind: ParamKind::StringList,
            required: false,
            label: None,
            doc: None,
            hint: None,
            placeholder: None,
            default: None,
            pattern: None,
            min: None,
            max: None,
            options: vec![],
            ref_query: None,
        });
        let mut values = Params::new();
        values.insert("tags".into(), json!("a, b ,,c"));
        let out = coerce_params(&a, values);
        assert_eq!(out["tags"], json!(["a", "b", "c"]));
    }

    #[test]
    fn action_form_spec_maps_param_kinds() {
        let mut a = action("new");
        for (name, kind) in [
            ("s", ParamKind::String),
            ("t", ParamKind::Text),
            ("n", ParamKind::Integer),
            ("b", ParamKind::Bool),
            ("e", ParamKind::Enum),
        ] {
            a.params.push(pavo_protocol::ParamSchema {
                name: name.into(),
                kind,
                required: false,
                label: None,
                doc: None,
                hint: None,
                placeholder: None,
                default: None,
                pattern: None,
                min: None,
                max: None,
                options: if kind == ParamKind::Enum {
                    vec!["x".into()]
                } else {
                    vec![]
                },
                ref_query: None,
            });
        }
        let spec = action_form_spec(&a);
        assert_eq!(spec.submit_op, "invoke");
        let kinds: Vec<&str> = spec.fields.iter().map(|f| f.kind.as_str()).collect();
        assert_eq!(kinds, ["text", "textarea", "number", "bool", "select"]);
    }

    #[test]
    fn dedupe_prefers_item_over_node_over_manifest() {
        let mut m = ui("dup", ActionScope::Manifest);
        m.action.label = "manifest".into();
        let mut n = ui("dup", ActionScope::Node);
        n.action.label = "node".into();
        let mut i = ui("dup", ActionScope::Item);
        i.action.label = "item".into();
        i.item_id = Some("i1".into());
        let other = ui("other", ActionScope::Manifest);
        let out = dedupe_ui_actions(vec![m, n, i, other]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].action.label, "item");
        assert_eq!(out[0].item_id.as_deref(), Some("i1"));
        // Manifest-first ordering with lower precedence later does
        // not displace.
        let out2 = dedupe_ui_actions(vec![
            ui("a", ActionScope::Manifest),
            ui("a", ActionScope::Item),
        ]);
        assert_eq!(out2[0].scope, ActionScope::Item);
    }

    #[test]
    fn key_spec_parsing() {
        assert_eq!(parse_key_spec("dd").as_deref(), Some("dd"));
        assert_eq!(parse_key_spec("D").as_deref(), Some("D"));
        assert_eq!(parse_key_spec("ctrl+k"), None);
        assert_eq!(parse_key_spec(""), None);
        assert_eq!(parse_key_spec("g q"), None);
    }

    #[test]
    fn palette_filters_and_activates() {
        let entries = palette_entries(
            &[
                ui("todo.done", ActionScope::Manifest),
                ui("x", ActionScope::Node),
            ],
            &[MenuEntry {
                id: "m1".into(),
                title: "New task".into(),
                icon: None,
                action: Some("todo.add".into()),
                route: None,
                keywords: vec!["create".into()],
                section: Some("myco".into()),
                priority: 1,
            }],
        );
        assert_eq!(entries.len(), 3);
        // Menus sort before actions.
        assert!(matches!(entries[0], PaletteEntry::Menu(_)));
        let mut s = PaletteSession::open(entries);
        assert_eq!(s.matches.len(), 3);
        // Filter to the menu entry via a keyword.
        for c in "create".chars() {
            s.key(KeyCode::Char(c));
        }
        assert_eq!(s.matches.len(), 1);
        match s.key(KeyCode::Enter) {
            PaletteCmd::Activate(e) => match *e {
                PaletteEntry::Menu(m) => assert_eq!(m.id, "m1"),
                other => panic!("expected menu activate, got {other:?}"),
            },
            other => panic!("expected activate, got {other:?}"),
        }
        // Esc closes; empty filter + Down wraps selection.
        let mut s2 = PaletteSession::open(s.entries.clone());
        s2.key(KeyCode::Down);
        assert_eq!(s2.sel, 1);
        s2.key(KeyCode::Up);
        assert_eq!(s2.sel, 0);
        assert!(matches!(s2.key(KeyCode::Esc), PaletteCmd::Close));
    }

    #[test]
    fn confirm_and_type_target_sessions() {
        let pending = PendingInvoke::new(
            UiAction {
                item_id: Some("T-1".into()),
                ..ui("del", ActionScope::Item)
            },
            Params::new(),
        );
        // Confirm: y/Enter confirm, n/Esc close.
        let mut c = ConfirmSession::new(pending.clone());
        assert_eq!(c.key(KeyCode::Char('x')), ConfirmCmd::None);
        assert_eq!(c.key(KeyCode::Char('y')), ConfirmCmd::Confirm);
        let mut c2 = ConfirmSession::new(pending.clone());
        assert_eq!(c2.key(KeyCode::Esc), ConfirmCmd::Close);

        // Type-target: exact id required.
        let mut t = TypeTargetSession::new(pending.clone());
        assert_eq!(t.expect.as_deref(), Some("T-1"));
        for ch in "T-2".chars() {
            t.key(KeyCode::Char(ch));
        }
        assert!(matches!(t.key(KeyCode::Enter), TypeTargetCmd::None));
        assert!(t.error.is_some());
        // Fix the buffer, then Enter submits.
        t.edit.set("T-1".to_string());
        assert!(matches!(t.key(KeyCode::Enter), TypeTargetCmd::Submit));

        // No derivable id → any non-empty input confirms.
        let free = PendingInvoke::new(ui("wipe", ActionScope::Manifest), Params::new());
        let mut t2 = TypeTargetSession::new(free);
        assert_eq!(t2.expect, None);
        assert!(matches!(t2.key(KeyCode::Enter), TypeTargetCmd::None));
        t2.key(KeyCode::Char('x'));
        assert!(matches!(t2.key(KeyCode::Enter), TypeTargetCmd::Submit));
    }
}
