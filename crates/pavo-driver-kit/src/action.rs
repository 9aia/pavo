//! Interaction-action constructors — [`Action`], [`ParamSchema`],
//! and [`MenuEntry`] as function calls instead of struct literals.
//!
//! ```
//! # use pavo_driver_kit as kit;
//! let done = kit::action("todo.done", "Mark done")
//!     .danger(pavo_protocol::DangerLevel::Confirm)
//!     .key("dd")
//!     .param(kit::param("id").kind(pavo_protocol::ParamKind::Ref)
//!         .required().ref_query("items").build())
//!     .build();
//! assert_eq!(done.id, "todo.done");
//! ```

use pavo_protocol::{Action, DangerLevel, MenuEntry, ParamKind, ParamSchema};

/// Start an [`Action`] — `id` is the invoke dispatch key, `label`
/// the button/menu text. Chain setters, [`ActionBuilder::build`]
/// finishes.
#[must_use]
pub fn action(id: impl Into<String>, label: impl Into<String>) -> ActionBuilder {
    ActionBuilder {
        action: Action {
            id: id.into(),
            label: label.into(),
            icon: None,
            danger: DangerLevel::None,
            keys: Vec::new(),
            params: Vec::new(),
            target: None,
            description: None,
            driver: None,
            route: None,
        },
    }
}

/// Fluent builder for [`Action`].
pub struct ActionBuilder {
    action: Action,
}

impl ActionBuilder {
    /// Icon hint.
    #[must_use]
    pub fn icon(mut self, icon: impl Into<String>) -> Self {
        self.action.icon = Some(icon.into());
        self
    }

    /// Confirmation level — see [`DangerLevel`].
    #[must_use]
    pub fn danger(mut self, danger: DangerLevel) -> Self {
        self.action.danger = danger;
        self
    }

    /// Shorthand for [`DangerLevel::Confirm`].
    #[must_use]
    pub fn confirm(self) -> Self {
        self.danger(DangerLevel::Confirm)
    }

    /// Shorthand for [`DangerLevel::TypeTarget`].
    #[must_use]
    pub fn type_target(self) -> Self {
        self.danger(DangerLevel::TypeTarget)
    }

    /// Add a keyboard shortcut.
    #[must_use]
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.action.keys.push(key.into());
        self
    }

    /// Add a typed param (see [`param`]).
    #[must_use]
    pub fn param(mut self, param: ParamSchema) -> Self {
        self.action.params.push(param);
        self
    }

    /// Set all params at once.
    #[must_use]
    pub fn params(mut self, params: Vec<ParamSchema>) -> Self {
        self.action.params = params;
        self
    }

    /// Context hint (`"item"`, `"row"`, …).
    #[must_use]
    pub fn target(mut self, target: impl Into<String>) -> Self {
        self.action.target = Some(target.into());
        self
    }

    /// Longer description (palette detail, tooltips).
    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.action.description = Some(description.into());
        self
    }

    /// Reserved cross-driver target.
    #[must_use]
    pub fn driver(mut self, driver: impl Into<String>) -> Self {
        self.action.driver = Some(driver.into());
        self
    }

    /// Navigate-instead-of-dispatch — a route template the host
    /// interpolates and navigates to locally when the action is
    /// activated (`{target.id}`, `{params.x}`, `{item.id}`, …; see
    /// `pavo_core::commands::interpolate`). No `invoke` round-trip.
    #[must_use]
    pub fn route(mut self, route: impl Into<String>) -> Self {
        self.action.route = Some(route.into());
        self
    }

    /// Finish — the [`Action`] to declare.
    #[must_use]
    pub fn build(self) -> Action {
        self.action
    }
}

impl From<ActionBuilder> for Action {
    fn from(b: ActionBuilder) -> Self {
        b.build()
    }
}

/// Start a [`ParamSchema`] — `name` is the invoke `params` key.
/// Defaults: `kind: string`, not required.
#[must_use]
pub fn param(name: impl Into<String>) -> ParamBuilder {
    ParamBuilder {
        param: ParamSchema {
            name: name.into(),
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
            options: Vec::new(),
            ref_query: None,
        },
    }
}

/// Fluent builder for [`ParamSchema`].
pub struct ParamBuilder {
    param: ParamSchema,
}

impl ParamBuilder {
    /// Input kind — selects the host's widget.
    #[must_use]
    pub fn kind(mut self, kind: ParamKind) -> Self {
        self.param.kind = kind;
        self
    }

    /// Shorthand for [`ParamKind::Text`].
    #[must_use]
    pub fn text(self) -> Self {
        self.kind(ParamKind::Text)
    }

    /// Shorthand for [`ParamKind::Enum`].
    #[must_use]
    pub fn enumeration(mut self, options: Vec<String>) -> Self {
        self.param.kind = ParamKind::Enum;
        self.param.options = options;
        self
    }

    /// Shorthand for [`ParamKind::Ref`] — picker over `ref_query`.
    #[must_use]
    pub fn reference(mut self, ref_query: impl Into<String>) -> Self {
        self.param.kind = ParamKind::Ref;
        self.param.ref_query = Some(ref_query.into());
        self
    }

    /// Must be present/non-empty on invoke.
    #[must_use]
    pub fn required(mut self) -> Self {
        self.param.required = true;
        self
    }

    /// Human-facing label (hosts humanize `name` when absent).
    #[must_use]
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.param.label = Some(label.into());
        self
    }

    /// Help text under the field.
    #[must_use]
    pub fn doc(mut self, doc: impl Into<String>) -> Self {
        self.param.doc = Some(doc.into());
        self
    }

    /// Default value (JSON per kind).
    #[must_use]
    pub fn default(mut self, value: serde_json::Value) -> Self {
        self.param.default = Some(value);
        self
    }

    /// Picker source for `ref` kind.
    #[must_use]
    pub fn ref_query(mut self, ref_query: impl Into<String>) -> Self {
        self.param.ref_query = Some(ref_query.into());
        self
    }

    /// Allowed values (`enum` kind).
    #[must_use]
    pub fn options(mut self, options: Vec<String>) -> Self {
        self.param.options = options;
        self
    }

    /// Numeric bounds.
    #[must_use]
    pub fn range(mut self, min: f64, max: f64) -> Self {
        self.param.min = Some(min);
        self.param.max = Some(max);
        self
    }

    /// Finish — the [`ParamSchema`].
    #[must_use]
    pub fn build(self) -> ParamSchema {
        self.param
    }
}

impl From<ParamBuilder> for ParamSchema {
    fn from(b: ParamBuilder) -> Self {
        b.build()
    }
}

/// Start a [`MenuEntry`] — palette/quick-launch item. Chain
/// `.action(...)` or `.route(...)`, [`MenuBuilder::build`] finishes.
#[must_use]
pub fn menu(id: impl Into<String>, title: impl Into<String>) -> MenuBuilder {
    MenuBuilder {
        entry: MenuEntry {
            id: id.into(),
            title: title.into(),
            icon: None,
            action: None,
            route: None,
            command: None,
            keys: Vec::new(),
            keywords: Vec::new(),
            section: None,
            priority: 100,
        },
    }
}

/// Fluent builder for [`MenuEntry`].
pub struct MenuBuilder {
    entry: MenuEntry,
}

impl MenuBuilder {
    /// Invoke this action when picked.
    #[must_use]
    pub fn action(mut self, action: impl Into<String>) -> Self {
        self.entry.action = Some(action.into());
        self
    }

    /// Navigate to this route instead.
    #[must_use]
    pub fn route(mut self, route: impl Into<String>) -> Self {
        self.entry.route = Some(route.into());
        self
    }

    /// Generic command target — the unified grammar
    /// (`action:<id>` | `route:<path>` | `host.<builtin>` |
    /// `driver:<name>:<id>`; see `pavo_core::commands`). Wins over
    /// `action`/`route` when both are set.
    #[must_use]
    pub fn command(mut self, command: impl Into<String>) -> Self {
        self.entry.command = Some(command.into());
        self
    }

    /// Add a suggested key binding for this entry (same role as
    /// `Action.keys` — the host keymap may override).
    #[must_use]
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.entry.keys.push(key.into());
        self
    }

    /// Set all suggested keys at once.
    #[must_use]
    pub fn keys(mut self, keys: Vec<String>) -> Self {
        self.entry.keys = keys;
        self
    }

    /// Icon hint.
    #[must_use]
    pub fn icon(mut self, icon: impl Into<String>) -> Self {
        self.entry.icon = Some(icon.into());
        self
    }

    /// Fuzzy-match aliases.
    #[must_use]
    pub fn keywords(mut self, keywords: Vec<String>) -> Self {
        self.entry.keywords = keywords;
        self
    }

    /// Grouping label.
    #[must_use]
    pub fn section(mut self, section: impl Into<String>) -> Self {
        self.entry.section = Some(section.into());
        self
    }

    /// Ordering hint — lower sorts first.
    #[must_use]
    pub fn priority(mut self, priority: i32) -> Self {
        self.entry.priority = priority;
        self
    }

    /// Finish — the [`MenuEntry`].
    #[must_use]
    pub fn build(self) -> MenuEntry {
        self.entry
    }
}

impl From<MenuBuilder> for MenuEntry {
    fn from(b: MenuBuilder) -> Self {
        b.build()
    }
}
