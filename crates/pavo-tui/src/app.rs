//! The ratatui surface: panes, key handling, drawing.
//!
//! Layout (per the pavo design note):
//!
//! ```text
//! ┌ tabs: dashboard · navbar contributions ──────────────── [?] help ┐
//! │ drivers │ view canvas (ViewNode IR)      │ detail (selection)  │
//! │  20%    │                               │       30%           │
//! ├────────┴───────────────────────────────┴───────────────────────┤
//! │ status — driver @ view · message              statusbar slots  │
//! └───────────────────────────────────────────────────────────────┘
//! ```
//!
//! All Host work happens on the [`worker`] thread; this module only
//! draws [`model`] output and forwards [`Cmd`]s. Panes: `Rail`
//! (drivers) → `Canvas` (current view/dashboard/note) → `Detail`
//! (selected row's fields). `Tab`/`←`/`→` cycle focus.
//!
//! Platform chrome slots (`sidebar`, `header_actions`, `notifications`,
//! `jobs`) have no dedicated TUI chrome — no sidebar pane, no action
//! bar, no feed polling — so they render as contribution sections on
//! the dashboard (see `model::build_dashboard`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton};
use pavo_core::{
    builtin_binding_sources, effective_bindings, menu_binding_source, menu_command, ActionScope,
    Chrome, Command, CommandBinding, HostBuiltin, KeySpec, Params, ResolveContext, Resolved,
    ResolvedInvoke, ResolvedView, RouteEntry, RouteKind, ViewEntry,
};
use pavo_protocol::{DangerLevel, DriverManifest, MenuEntry, RenderModel};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use ratatui::Frame;

use crate::actions::{
    action_form_spec, builtin_menu_entries, coerce_params, filter_params, key_event_spec,
    palette_entries, plan_action, ui_binding_source, ConfirmCmd, ConfirmSession, FilterCmd,
    FilterSession, InvokeStep, PaletteCmd, PaletteEntry, PaletteSession, PendingInvoke,
    TypeTargetCmd, TypeTargetSession, UiAction, CHORD_TIMEOUT,
};
use crate::form::{FieldEdit, FormSession, LineEdit, SessionCmd};
use crate::keymap::UserKeymap;
use crate::model::{self, FormMark, Model, RowStyle, Selection};
use crate::worker::{Cmd, DriverRow, Ev, Worker};

/// Which pane has keyboard focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    /// Left driver rail.
    Rail,
    /// Center view canvas.
    Canvas,
    /// Right detail pane.
    Detail,
}

/// What the canvas is showing.
#[derive(Clone, Debug)]
enum Canvas {
    /// The chrome pseudo-view — views + contributions, jump targets.
    Dashboard,
    /// A resolved driver view.
    View(Box<ResolvedView>),
    /// A static note — unresolvable slot route, empty state.
    Note {
        /// Heading.
        title: String,
        /// Body fields.
        fields: Vec<(String, String)>,
    },
}

/// One top-bar tab.
#[derive(Clone, Debug)]
struct Tab {
    /// Label.
    label: String,
    /// `"dashboard"`, a namespaced view id, or `None` for
    /// driver-owned slot routes (shows a note instead).
    target: Option<String>,
    /// Fields for the note pane when `target` is `None`.
    note: Vec<(String, String)>,
}

/// Chars with hard-wired [`App::key_now`] bindings — command
/// bindings on these chars are ignored on the char path (they can
/// still be reached via the palette or a different spec). Keeps
/// driver `keys[]` from shadowing pane navigation/quit/dump.
const NAV_CHARS: &[char] = &['q', '?', 'r', 'd', 'g', 'x', 'j', 'k', 'h', 'l', ':'];

/// `ViewSpec.refresh_ms` bounds — a sub-second timer busy-loops the
/// driver's `serve`, a multi-minute one is indistinguishable from
/// manual refresh. Out-of-range hints are ignored entirely (the
/// convention is documented on the protocol field).
const MIN_REFRESH: Duration = Duration::from_secs(1);
const MAX_REFRESH: Duration = Duration::from_secs(300);

/// Rects from the last draw — mouse hit-testing.
#[derive(Clone, Copy, Debug, Default)]
struct LayoutRects {
    tabs: Rect,
    rail: Rect,
    canvas: Rect,
    detail: Rect,
    status: Rect,
}

/// Application state — everything the draw loop needs.
pub struct App {
    worker: Worker,
    booted: bool,
    drivers: Vec<DriverRow>,
    views: Vec<ViewEntry>,
    chrome: Chrome,
    routes: Vec<RouteEntry>,
    tabs: Vec<Tab>,
    current: Option<Canvas>,
    /// Renderer-plugin outputs for the current view, keyed by node
    /// tree path — filled by `Ev::View`, consumed by
    /// `model::build_view_with_plugins`.
    plugin_renders: HashMap<String, RenderModel>,
    detail: Option<Selection>,
    detail_scroll: usize,
    /// Position within `model.selectables`.
    sel_pos: usize,
    /// Canvas scroll offset `(x, y)` — `h`/`l` and wheel.
    scroll: (usize, usize),
    pane: Pane,
    rail_sel: usize,
    loading: Option<String>,
    /// `(target, message)` — shown in the canvas until the next resolve.
    load_error: Option<(String, String)>,
    /// Last successfully resolved namespaced id (for `r`).
    last_target: String,
    /// Transient status-bar message.
    status: String,
    help: bool,
    /// Canvas width from the last draw — input handlers build the
    /// model with it so row indices match what was rendered.
    canvas_w: usize,
    rects: LayoutRects,
    /// `(x_start, x_end, tab_index)` for click hit-tests.
    tab_hits: Vec<(u16, u16, usize)>,
    /// Deep-link target to resolve once boot completes.
    initial: Option<String>,
    /// Rail row armed by a first click — a second click opens it.
    open_armed: Option<usize>,
    /// Open form-editing session (overlay) — Enter on a form field
    /// or submit row in the canvas opens one; `submit` wire op
    /// fires from inside it. Also hosts action auto-forms (a
    /// `FormSession` with `action` set routes to `invoke` instead).
    form_session: Option<FormSession>,
    /// Interaction actions collected for the current view (worker
    /// ships them with [`Ev::View`]) — palette entries and key
    /// chords derive from these.
    actions: Vec<UiAction>,
    /// `contributes.menus[]` of the current view's driver.
    menus: Vec<MenuEntry>,
    /// Open command palette (`:` key) — fuzzy filter over actions +
    /// menu entries.
    palette: Option<PaletteSession>,
    /// Open `danger: confirm` modal.
    confirm: Option<ConfirmSession>,
    /// Open `danger: type-target` modal.
    type_target: Option<TypeTargetSession>,
    /// Pending chord prefix + when it was pressed — multi-char
    /// action `keys[]` specs (`"dd"`) resolve through this; a lone
    /// prefix char waits [`CHORD_TIMEOUT`] before replaying as a
    /// normal keypress.
    pending_key: Option<(String, Instant)>,
    /// Full key spec → command for bound keys. Specs are normalized
    /// [`KeySpec`] strings: bare chars/sequences match the char path,
    /// `ctrl+…` chords and named keys (`enter`, `f5`, …) match the
    /// modified/named-key path. Item-scoped actions are excluded —
    /// they carry a row target the canvas doesn't track, so they
    /// stay palette-only.
    key_chords: HashMap<String, Command>,
    /// Proper prefixes of multi-char sequence specs — tells
    /// `chord_or_key` whether a lone char might continue into a
    /// chord.
    chord_prefixes: HashSet<String>,
    /// The merged binding list `key_chords` was built from — the
    /// help overlay's "commands" section renders this.
    bindings: Vec<CommandBinding>,
    /// Driver manifests by kind (worker ships them at boot/reload) —
    /// the command layer's `ResolveContext` table.
    manifests: BTreeMap<String, DriverManifest>,
    /// Manifest used when the current driver has none (never in
    /// practice — every discovered driver reports one; keeps
    /// `ResolveContext.manifest` total).
    fallback_manifest: DriverManifest,
    /// The parsed user keymap (`keys.toml`) — overlay source for
    /// [`effective_bindings`].
    keymap: UserKeymap,
    /// View params of the current/last resolve — route `?query`
    /// args and the `host.filter` param live here; refresh and
    /// command resolution read them back.
    view_params: Params,
    /// Synthesized `host.*` builtin palette entries (built once —
    /// `$EDITOR` gating happens here).
    builtin_menus: Vec<MenuEntry>,
    /// Open `host.filter` overlay.
    filter: Option<FilterSession>,
    /// The last invoke's dispatch context — `ActionResult.route`
    /// templates interpolate over it.
    last_dispatch: Option<DispatchCtx>,
    /// Auto-refresh schedule for the current view —
    /// `(interval, next_deadline)` derived from
    /// `ViewSpec.refresh_ms` (bounds-checked). `None` on the
    /// dashboard/notes and for views without a valid hint.
    auto_refresh: Option<(Duration, Instant)>,
}

/// The invoke context remembered for `ActionResult.route`
/// interpolation — driver, opaque target JSON, and params (the same
/// shape `pavo_core::commands::interpolate_ctx_value` consumes).
#[derive(Clone, Debug)]
struct DispatchCtx {
    /// Driver kind the invoke targeted.
    driver: String,
    /// The invoke `target` JSON (item `{"id":…}`, selection, …).
    target: Option<serde_json::Value>,
    /// The invoke `params` map.
    params: Params,
}

impl App {
    /// Build the app around a spawned worker — no user keymap (the
    /// production entry point is [`App::with_keymap`]; tests and
    /// embedders get declaration suggestions only). `initial` is the
    /// deep-link target (`--view`, `--driver`, or positional
    /// `driver:view`) resolved after boot.
    #[allow(dead_code)] // production uses with_keymap; tests embed this
    #[must_use]
    pub fn new(worker: Worker, initial: Option<String>) -> Self {
        Self::with_keymap(worker, initial, UserKeymap::default())
    }

    /// [`App::new`] plus a parsed user keymap — `main` passes
    /// [`crate::keymap::load`]'s result so `keys.toml` overrides
    /// apply.
    #[must_use]
    pub fn with_keymap(worker: Worker, initial: Option<String>, keymap: UserKeymap) -> Self {
        let mut app = Self {
            worker,
            booted: false,
            drivers: Vec::new(),
            views: Vec::new(),
            chrome: Chrome::default(),
            routes: Vec::new(),
            tabs: Vec::new(),
            current: None,
            plugin_renders: HashMap::new(),
            detail: None,
            detail_scroll: 0,
            sel_pos: 0,
            scroll: (0, 0),
            pane: Pane::Rail,
            rail_sel: 0,
            loading: None,
            load_error: None,
            last_target: String::new(),
            status: "discovering drivers…".to_string(),
            help: false,
            canvas_w: 80,
            rects: LayoutRects::default(),
            tab_hits: Vec::new(),
            initial,
            open_armed: None,
            form_session: None,
            actions: Vec::new(),
            menus: Vec::new(),
            palette: None,
            confirm: None,
            type_target: None,
            pending_key: None,
            key_chords: HashMap::new(),
            chord_prefixes: HashSet::new(),
            bindings: Vec::new(),
            manifests: BTreeMap::new(),
            fallback_manifest: empty_manifest(),
            keymap,
            view_params: Params::new(),
            builtin_menus: builtin_menu_entries(),
            filter: None,
            last_dispatch: None,
            auto_refresh: None,
        };
        app.rebuild_keybinds();
        app
    }

    /// Drain pending worker events, then handle one input event.
    /// Returns `false` when the app should quit.
    pub fn step(&mut self) -> bool {
        while let Ok(ev) = self.worker.rx.try_recv() {
            self.on_event(ev);
        }
        self.maybe_auto_refresh();
        // Poll ~100ms so worker events land even without input.
        if !crossterm::event::poll(Duration::from_millis(100)).unwrap_or(false) {
            // A stale chord prefix replays as normal keypresses.
            if self
                .pending_key
                .as_ref()
                .is_some_and(|(_, t)| t.elapsed() > CHORD_TIMEOUT)
            {
                self.flush_pending();
            }
            return true;
        }
        match crossterm::event::read() {
            Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => self.on_key_event(k),
            Ok(Event::Mouse(m)) => {
                self.on_mouse(m);
                true
            }
            Ok(Event::Resize(..)) => true,
            _ => true,
        }
    }

    fn on_event(&mut self, ev: Ev) {
        match ev {
            Ev::Boot {
                drivers,
                views,
                chrome,
                routes,
                manifests,
            } => {
                self.drivers = drivers;
                self.views = views;
                self.chrome = chrome;
                self.routes = routes;
                self.manifests = manifests;
                self.booted = true;
                self.rebuild_tabs();
                self.rail_sel = 0;
                let target = self.initial.take().or_else(|| self.default_target());
                match target {
                    Some(t) => self.resolve(&t),
                    None => {
                        self.status = "no pavo drivers discovered".to_string();
                    }
                }
            }
            Ev::View {
                target,
                result,
                renders,
                actions,
                menus,
            } => {
                self.loading = None;
                self.plugin_renders = renders;
                match result {
                    Ok(rv) => {
                        self.last_target = rv.namespaced.clone();
                        self.auto_refresh = Self::refresh_schedule(&rv);
                        self.current = Some(Canvas::View(rv));
                        self.load_error = None;
                        self.sel_pos = 0;
                        self.scroll = (0, 0);
                        self.detail = None;
                        self.form_session = None;
                        self.palette = None;
                        self.confirm = None;
                        self.type_target = None;
                        self.filter = None;
                        self.apply_actions(actions, menus);
                        self.pane = Pane::Canvas;
                        self.status = format!("loaded {}", self.last_target);
                    }
                    Err(msg) => {
                        self.load_error = Some((target.clone(), msg));
                        self.status = format!("resolve failed: {target}");
                    }
                }
            }
            Ev::Reloaded {
                views,
                chrome,
                routes,
                manifests,
            } => {
                self.views = views;
                self.chrome = chrome;
                self.routes = routes;
                self.manifests = manifests;
                self.rebuild_tabs();
                self.rebuild_keybinds();
                self.status = "chrome reloaded".to_string();
            }
            Ev::Submit { result } => self.on_submit(result),
            Ev::Invoke {
                result,
                actions,
                menus,
            } => self.on_invoke(result, actions, menus),
        }
    }

    /// A `submit` wire-op answer landed. With `result.view` the
    /// canvas swaps to the fresh ViewResult and the session closes;
    /// `errors`/messages stay inside the open session (the form
    /// re-renders with them, per the protocol).
    fn on_submit(&mut self, result: Result<pavo_protocol::wire::SubmitResult, String>) {
        let Some(session) = &mut self.form_session else {
            return; // session closed while in flight — drop the answer
        };
        session.busy = false;
        match result {
            Ok(res) => {
                session.errors = res
                    .errors
                    .iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            v.as_str()
                                .map(str::to_string)
                                .unwrap_or_else(|| v.to_string()),
                        )
                    })
                    .collect();
                session.message = res.message.clone();
                if let Some(view) = res.view {
                    self.swap_result_view(*view);
                    self.status = res
                        .message
                        .clone()
                        .unwrap_or_else(|| "submitted".to_string());
                } else {
                    self.status = res
                        .message
                        .clone()
                        .unwrap_or_else(|| "submitted".to_string());
                }
            }
            Err(e) => {
                session.message = Some(format!("submit failed: {e}"));
                self.status = format!("submit failed: {e}");
            }
        }
    }

    /// Swap a fresh [`ViewResult`] into the canvas — shared by the
    /// `submit` and `invoke` result paths. Plugin renders keyed to
    /// the old tree are stale; built-in rendering covers the new
    /// one until the next resolve.
    fn swap_result_view(&mut self, view: pavo_protocol::wire::ViewResult) {
        if let Some(Canvas::View(rv)) = &self.current {
            let fresh = pavo_core::ResolvedView {
                driver: rv.driver.clone(),
                view: rv.view.clone(),
                namespaced: rv.namespaced.clone(),
                result: view,
            };
            self.auto_refresh = Self::refresh_schedule(&fresh);
            self.current = Some(Canvas::View(Box::new(fresh)));
            self.plugin_renders.clear();
            self.sel_pos = 0;
            self.scroll = (0, 0);
            self.detail = None;
            self.form_session = None;
            self.pane = Pane::Canvas;
        }
    }

    /// Store the worker-collected action/menu lists and rebuild the
    /// key-chord table.
    fn apply_actions(&mut self, actions: Vec<UiAction>, menus: Vec<MenuEntry>) {
        self.actions = actions;
        self.menus = menus;
        self.pending_key = None; // chord prefixes reference the old bindings
        self.rebuild_keybinds();
    }

    /// (Re)build `key_chords`/`chord_prefixes`/`bindings` from the
    /// unified binding sources: host builtins + menu entries
    /// (driver menus *and* the synthesized `host.*` entries) +
    /// non-item action `keys[]`, merged with the user keymap overlay
    /// via [`effective_bindings`]. Item-scoped actions are skipped —
    /// they carry a row target the canvas doesn't track, so binding
    /// them globally would invoke against the wrong item (they
    /// remain palette entries).
    fn rebuild_keybinds(&mut self) {
        self.key_chords.clear();
        self.chord_prefixes.clear();
        let mut sources = builtin_binding_sources();
        // TUI extras on top of the shared suggestions: `:` keeps the
        // legacy palette key alongside `ctrl+p`; `F` joins `/` for
        // filter; `host.editor` is only bindable when `$EDITOR`
        // exists (same gate as its palette entry).
        sources.retain(|s| {
            !matches!(s.command, Command::Host(HostBuiltin::Editor))
                || std::env::var_os("EDITOR").is_some()
        });
        for s in &mut sources {
            match s.command {
                Command::Host(HostBuiltin::Palette) => s.keys.push(":".to_string()),
                Command::Host(HostBuiltin::Filter) => s.keys.push("F".to_string()),
                _ => {}
            }
        }
        for m in self.menus.iter().chain(self.builtin_menus.iter()) {
            if let Some(s) = menu_binding_source(m) {
                sources.push(s);
            }
        }
        for u in &self.actions {
            if u.scope == ActionScope::Item {
                continue;
            }
            sources.push(ui_binding_source(u));
        }
        let overlay = self
            .keymap
            .overlay_for(self.current_driver(), self.current_view_id());
        let bindings = effective_bindings(sources, &overlay);
        for b in &bindings {
            for key in &b.keys {
                // Declared/suggested keys may predate KeySpec
                // normalization — re-validate; overlay entries were
                // already validated by effective_bindings.
                let Some(spec) = KeySpec::parse(key) else {
                    continue;
                };
                let norm = spec.as_str().to_string();
                // First binding (sorted command-id order) of a spec
                // wins.
                self.key_chords
                    .entry(norm.clone())
                    .or_insert_with(|| b.command.clone());
                if spec.is_sequence() {
                    let chars: Vec<char> = norm.chars().collect();
                    for i in 1..chars.len() {
                        self.chord_prefixes
                            .insert(chars[..i].iter().collect::<String>());
                    }
                }
            }
        }
        self.bindings = bindings;
    }

    /// The driver kind of the current view (`None` off-view).
    fn current_driver(&self) -> Option<&str> {
        match &self.current {
            Some(Canvas::View(rv)) => Some(rv.driver.as_str()),
            _ => None,
        }
    }

    /// The bare view id of the current view.
    fn current_view_id(&self) -> Option<&str> {
        match &self.current {
            Some(Canvas::View(rv)) => Some(rv.view.as_str()),
            _ => None,
        }
    }

    /// The selected canvas row's [`Selection`] (cloned — the model
    /// is rebuilt per call anyway).
    fn selection(&self) -> Option<Selection> {
        let model = self.model_for_input()?;
        let &row_idx = model.selectables.get(self.sel_pos)?;
        let sel_id = model.rows[row_idx].sel?;
        model.selections.get(sel_id).cloned()
    }

    /// The selected row's item-context JSON (`item_value` shape) —
    /// `{item.*}` template data for route interpolation.
    fn selection_item(&self) -> Option<serde_json::Value> {
        self.selection()?.item
    }

    /// The invoke `target` JSON for the current selection —
    /// `{"id": <item id>}` for tree items (the same shape item-scoped
    /// actions send), `None` otherwise.
    fn selection_target(&self) -> Option<serde_json::Value> {
        let item = self.selection_item()?;
        item.get("id")
            .and_then(serde_json::Value::as_str)
            .map(|id| serde_json::json!({"id": id}))
    }

    /// Run `f` against a [`ResolveContext`] built from the current
    /// canvas state — driver/manifest/view/drivers/target/params all
    /// borrowed from `self`, so the result must be owned (`R` can't
    /// borrow the context).
    fn with_resolve_ctx<R>(&self, f: impl FnOnce(&ResolveContext<'_>) -> R) -> R {
        let target = self.selection_target();
        let (driver, view, manifest) = match &self.current {
            Some(Canvas::View(rv)) => (
                rv.driver.as_str(),
                Some(&rv.result),
                self.manifests
                    .get(rv.driver.as_str())
                    .unwrap_or(&self.fallback_manifest),
            ),
            _ => ("", None, &self.fallback_manifest),
        };
        let ctx = ResolveContext {
            driver,
            manifest,
            view,
            drivers: &self.manifests,
            target: target.as_ref(),
            params: &self.view_params,
        };
        f(&ctx)
    }

    /// Interpolate a `{…}` route template over the current context —
    /// `item` is the selected row's `item_value` JSON when the caller
    /// has one (route commands use the live selection; action routes
    /// pass their own item-scoped `{"id":…}`).
    fn interpolate_route(&self, template: &str, item: Option<&serde_json::Value>) -> String {
        self.with_resolve_ctx(|ctx| pavo_core::interpolate(template, &ctx.interpolate_ctx(item)))
    }

    /// An `invoke` wire-op answer landed. `result.view` swaps the
    /// canvas (with freshly collected actions/menus); without one
    /// the host already invalidated its view cache, so a refresh
    /// refetches — unless `errors` rejected the invoke, in which
    /// case the status line carries them and nothing navigates.
    fn on_invoke(
        &mut self,
        result: Result<pavo_protocol::wire::ActionResult, String>,
        actions: Vec<UiAction>,
        menus: Vec<MenuEntry>,
    ) {
        match result {
            Ok(res) => {
                let mut msg = res.message.clone().unwrap_or_else(|| "invoked".to_string());
                if !res.errors.is_empty() {
                    let errs = res
                        .errors
                        .iter()
                        .map(|(k, v)| {
                            format!(
                                "{k}: {}",
                                v.as_str()
                                    .map(str::to_string)
                                    .unwrap_or_else(|| v.to_string())
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    msg = format!("{msg} ({errs})");
                }
                // `ActionResult.route` — a driver-requested redirect:
                // interpolate over the invoke context and navigate
                // instead of swapping/refreshing (route wins).
                if let Some(template) = res.route.clone() {
                    let route = self.invoke_route(&template, res.view.as_deref());
                    if !route.is_empty() {
                        self.navigate_to_route(&route);
                        self.status = msg;
                        return;
                    }
                }
                if let Some(view) = res.view {
                    self.swap_result_view(*view);
                    self.apply_actions(actions, menus);
                    self.status = msg;
                } else if res.errors.is_empty() {
                    // No fresh view — the host invalidated its cache;
                    // refetch so the canvas reflects the mutation.
                    self.refresh();
                    self.status = msg;
                } else {
                    self.status = msg;
                }
            }
            Err(e) => {
                self.status = format!("invoke failed: {e}");
            }
        }
    }

    /// Interpolate an [`ActionResult.route`] template over the invoke
    /// context remembered by [`App::send_invoke`] (`last_dispatch`),
    /// with the result's fresh view supplying `{view}`/`{data}` when
    /// present. Empty when there's no dispatch context to interpolate
    /// against (defensive — `send_invoke` always sets it).
    fn invoke_route(
        &self,
        template: &str,
        result_view: Option<&pavo_protocol::wire::ViewResult>,
    ) -> String {
        let Some(d) = &self.last_dispatch else {
            return String::new();
        };
        let ctx = pavo_core::interpolate_ctx_value(
            &d.driver,
            result_view,
            d.target.as_ref(),
            &d.params,
            None,
        );
        pavo_core::interpolate(template, &ctx)
    }

    /// Activate a collected action: an `Action.route` declaration
    /// navigates instead of invoking (interpolated like
    /// `resolve_command` does — item context `{"id":…}` for
    /// item-scoped actions); params open the auto-form; everything
    /// else goes straight to the danger gate.
    fn activate_action(&mut self, ui: UiAction) {
        if let Some(template) = ui.action.route.clone() {
            let item = ui.item_id.as_ref().map(|i| serde_json::json!({"id": i}));
            let route = self.interpolate_route(&template, item.as_ref());
            self.navigate_to_route(&route);
            return;
        }
        match plan_action(&ui.action) {
            InvokeStep::Form => self.open_action_form(ui),
            _ => self.gate_action(ui, Params::new()),
        }
    }

    /// Open the auto-form for an action's `params` — the existing
    /// [`FormSession`] editor, tagged so Enter routes to `invoke`
    /// (with param validation + the danger gate) instead of `submit`.
    fn open_action_form(&mut self, ui: UiAction) {
        let spec = action_form_spec(&ui.action);
        let mut session = FormSession::open(spec, ui.target.clone(), None);
        session.action = Some(ui);
        self.status = format!("{} — Enter invokes, Esc closes", session.spec.title);
        self.form_session = Some(session);
    }

    /// The danger gate after params are collected (or immediately
    /// for parameterless actions): `none` invokes, `confirm` opens
    /// the modal, `type-target` opens the type-the-id modal.
    fn gate_action(&mut self, ui: UiAction, params: Params) {
        self.gate_pending(PendingInvoke::new(ui, params));
    }

    /// [`App::gate_action`] over an already-assembled
    /// [`PendingInvoke`] — the command layer's dispatch path builds
    /// one with an explicit (possibly non-`{"id"}`) target JSON.
    fn gate_pending(&mut self, pending: PendingInvoke) {
        match pending.ui.action.danger {
            DangerLevel::None => self.send_invoke(pending),
            DangerLevel::Confirm => {
                self.status = format!("confirm: {} — y/n", pending.ui.action.label);
                self.confirm = Some(ConfirmSession::new(pending));
            }
            DangerLevel::TypeTarget => {
                self.status = format!("type to confirm: {}", pending.ui.action.label);
                self.type_target = Some(TypeTargetSession::new(pending));
            }
        }
    }

    /// Fire `Cmd::Invoke` for a fully-gated pending action — and
    /// remember the dispatch context for `ActionResult.route`
    /// interpolation.
    fn send_invoke(&mut self, pending: PendingInvoke) {
        let driver = pending
            .ui
            .target
            .split(':')
            .next()
            .unwrap_or_default()
            .to_string();
        self.last_dispatch = Some(DispatchCtx {
            driver,
            target: pending.target.clone(),
            params: pending.params.clone(),
        });
        let _ = self.worker.tx.send(Cmd::Invoke {
            target: pending.ui.target.clone(),
            action: pending.ui.action.id.clone(),
            view: pending.ui.view.clone(),
            action_target: pending.target.clone(),
            params: pending.params.clone(),
        });
        self.status = format!("invoking {}…", pending.ui.action.label);
    }

    /// `:`/`ctrl+p` — open the command palette over the current
    /// view's actions + menu entries + synthesized host builtins.
    /// Builtins sort *last* — driver menus/actions are the primary
    /// surface; host chrome is the fallback (and fuzzy queries like
    /// "done" shouldn't land on `host.editor` first).
    fn open_palette(&mut self) {
        let mut entries = palette_entries(&self.actions, &self.menus);
        entries.extend(palette_entries(&[], &self.builtin_menus));
        if entries.is_empty() {
            self.status = "no actions or menu entries".to_string();
            return;
        }
        self.palette = Some(PaletteSession::open(entries));
        self.status = "palette — type to filter, Enter runs".to_string();
    }

    /// Activate a palette entry. Returns `false` only when the
    /// entry resolves to `host.quit` (palette/menu commands can
    /// reach builtins — plain action entries never quit).
    fn activate_palette_entry(&mut self, entry: PaletteEntry) -> bool {
        match entry {
            PaletteEntry::Action(ui) => {
                self.activate_action(ui);
                true
            }
            PaletteEntry::Menu(m) => match menu_command(&m) {
                Some(cmd) => self.run_command(cmd),
                None => {
                    self.status = format!("menu entry {} is display-only", m.id);
                    true
                }
            },
        }
    }

    /// Navigate to an interpolated route string: `?query` pairs
    /// become view params; the path resolves through chrome routes,
    /// declared view routes, or a bare `driver:view` target. Routes
    /// like `/myco/item?id=T-1` (from `item_route`, `Action.route`,
    /// `route:` commands, `ActionResult.route`) land here.
    fn navigate_to_route(&mut self, route: &str) {
        let (path, query) = match route.split_once('?') {
            Some((p, q)) => (p, q),
            None => (route, ""),
        };
        let params = parse_route_query(query);
        if path.is_empty() {
            self.status = format!("empty route: {route}");
            return;
        }
        if path == "/" || path == "dashboard" {
            self.resolve_with_params("dashboard", Params::new());
            return;
        }
        let target = self
            .route_target(path)
            .or_else(|| {
                self.views
                    .iter()
                    .find(|v| v.route == path)
                    .map(|v| v.namespaced.clone())
            })
            .or_else(|| {
                // Bare `driver:view` (or `driver`) route — no leading
                // slash, contains a namespace separator.
                (path.contains(':') && !path.starts_with('/')).then(|| path.to_string())
            });
        match target {
            Some(t) => self.resolve_with_params(&t, params),
            None => self.status = format!("unresolvable route: {route}"),
        }
    }

    /// Keys while the palette is open — the session owns the
    /// keyboard; Enter activates the selected entry.
    fn on_palette_key(&mut self, code: KeyCode) -> bool {
        let cmd = match self.palette.as_mut() {
            Some(s) => s.key(code),
            None => return true,
        };
        match cmd {
            PaletteCmd::Close => self.palette = None,
            PaletteCmd::Activate(entry) => {
                self.palette = None;
                return self.activate_palette_entry(*entry);
            }
            PaletteCmd::None => {}
        }
        true
    }

    /// Keys while the filter overlay is open — Enter applies the
    /// text as the view's `filter_param`, Esc cancels.
    fn on_filter_key(&mut self, code: KeyCode) -> bool {
        let cmd = match self.filter.as_mut() {
            Some(s) => s.key(code),
            None => return true,
        };
        match cmd {
            FilterCmd::Close => self.filter = None,
            FilterCmd::Submit => {
                if let Some(s) = self.filter.take() {
                    let params = filter_params(&self.view_params, &s.param, &s.edit.text);
                    self.resolve_with_params(&s.target, params);
                }
            }
            FilterCmd::None => {}
        }
        true
    }

    /// `host.filter` — open the filter overlay bound to the current
    /// view's `filter_param` (convention `"q"` when undeclared).
    fn open_filter(&mut self) {
        let Some(Canvas::View(rv)) = &self.current else {
            self.status = "no view to filter".to_string();
            return;
        };
        let param = rv
            .result
            .view
            .filter_param
            .clone()
            .unwrap_or_else(|| "q".to_string());
        self.filter = Some(FilterSession::new(param.clone(), rv.namespaced.clone()));
        self.status = format!("filter {param} — Enter applies, Esc cancels");
    }

    /// Run a parsed [`Command`] — the single entry point for
    /// palette entries, menu `keys[]`, keybindings, and builtin
    /// invocations. Returns `false` only for `host.quit`.
    ///
    /// `action:`/`driver:<current>:` ids first match the collected
    /// [`UiAction`]s (preserving auto-form/danger/route semantics —
    /// ops-derived actions aren't in any manifest); misses fall
    /// through to `resolve_command` (manifest scope, `driver:`
    /// foreign dispatch, `Unsupported`).
    fn run_command(&mut self, command: Command) -> bool {
        match command {
            Command::Host(b) => self.run_builtin(b),
            Command::Route { template } => {
                // Route templates interpolate over the live
                // selection (`{item.*}` = selected tree item).
                let item = self.selection_item();
                let route = self.interpolate_route(&template, item.as_ref());
                self.navigate_to_route(&route);
                true
            }
            Command::Action { id } => {
                if self.current_driver().is_none() {
                    self.status = "no active view for action".to_string();
                    return true;
                }
                match self.actions.iter().find(|u| u.action.id == id).cloned() {
                    Some(ui) => {
                        self.activate_action(ui);
                        true
                    }
                    None => self.resolve_and_run(Command::Action { id }),
                }
            }
            Command::Driver { driver, id } => {
                if Some(driver.as_str()) == self.current_driver() {
                    match self.actions.iter().find(|u| u.action.id == id).cloned() {
                        Some(ui) => {
                            self.activate_action(ui);
                            return true;
                        }
                        None => return self.resolve_and_run(Command::Driver { driver, id }),
                    }
                }
                self.resolve_and_run(Command::Driver { driver, id })
            }
        }
    }

    /// `resolve_command` + execute the [`Resolved`] — the fallback
    /// for commands with no collected [`UiAction`] (foreign-driver
    /// `driver:` dispatches, manifest-declared ids the worker didn't
    /// collect, `route:`/`host.*` reached via `resolve_command`).
    fn resolve_and_run(&mut self, command: Command) -> bool {
        let resolved = self.with_resolve_ctx(|ctx| pavo_core::resolve_command(&command, ctx));
        self.run_resolved(resolved)
    }

    /// Execute a [`Resolved`] — dispatches become invokes (through
    /// the same danger gate), routes navigate, builtins run.
    fn run_resolved(&mut self, resolved: Resolved) -> bool {
        match resolved {
            Resolved::Dispatch(inv) => {
                self.dispatch_invoke(*inv);
                true
            }
            Resolved::Navigate { route } => {
                self.navigate_to_route(&route);
                true
            }
            Resolved::Builtin(b) => self.run_builtin(b),
            Resolved::Unsupported => {
                self.status = "unsupported command".to_string();
                true
            }
        }
    }

    /// Turn a [`ResolvedInvoke`] into the normal invoke flow —
    /// declared actions keep their params/danger; undeclared ids get
    /// a synthetic parameterless action (the driver answers
    /// `ok:false` on unknowns, same as the wire contract).
    fn dispatch_invoke(&mut self, inv: ResolvedInvoke) {
        let action = inv.declared.unwrap_or_else(|| pavo_protocol::Action {
            id: inv.action.clone(),
            label: inv.action.clone(),
            icon: None,
            danger: DangerLevel::None,
            keys: Vec::new(),
            params: Vec::new(),
            target: None,
            description: None,
            driver: None,
            route: None,
        });
        // `{"id": …}` targets recover `item_id`; arbitrary target
        // JSON is preserved on the pending invoke verbatim.
        let item_id = inv
            .target
            .as_ref()
            .and_then(|t| t.get("id"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let ui = UiAction {
            action,
            scope: ActionScope::Manifest,
            target: match &inv.view {
                Some(v) => format!("{}:{v}", inv.driver),
                None => inv.driver.clone(),
            },
            view: inv.view.clone(),
            item_id,
        };
        let mut pending = PendingInvoke::new(ui, inv.params);
        pending.target = inv.target;
        self.gate_pending(pending);
    }

    /// A `host.*` builtin — returns `false` for `host.quit` (the only
    /// builtin that ends the event loop).
    fn run_builtin(&mut self, b: HostBuiltin) -> bool {
        match b {
            HostBuiltin::Filter => self.open_filter(),
            HostBuiltin::Help => self.help = true,
            HostBuiltin::Reload => self.refresh(),
            HostBuiltin::Palette => self.open_palette(),
            HostBuiltin::Editor => self.open_in_editor(),
            HostBuiltin::Quit => return false,
        }
        true
    }

    /// `host.editor` — dump the current view's JSON to a tmp file
    /// (same artifact as `d`) and hand it to `$EDITOR`. The TUI
    /// can't suspend the alt screen cleanly, so the editor inherits
    /// the raw terminal — vim-style editors cope; GUI editors
    /// (`code`, …) just open alongside. Only offered when `$EDITOR`
    /// is set (see [`builtin_menu_entries`]).
    fn open_in_editor(&mut self) {
        let Some(path) = self.dump_view_json() else {
            self.status = "host.editor: no view to dump".to_string();
            return;
        };
        let Some(editor) = std::env::var_os("EDITOR") else {
            self.status = "host.editor: $EDITOR is not set".to_string();
            return;
        };
        let words: Vec<String> = editor
            .to_string_lossy()
            .split_whitespace()
            .map(str::to_string)
            .collect();
        let Some((prog, args)) = words.split_first() else {
            self.status = "host.editor: empty $EDITOR".to_string();
            return;
        };
        match std::process::Command::new(prog)
            .args(args)
            .arg(&path)
            .status()
        {
            Ok(_) => self.status = format!("editor closed ({})", path.display()),
            Err(e) => self.status = format!("host.editor failed: {e}"),
        }
    }

    /// Keys while the confirm modal is open — y/Enter invokes, n/Esc
    /// cancels.
    fn on_confirm_key(&mut self, code: KeyCode) -> bool {
        let cmd = match self.confirm.as_mut() {
            Some(s) => s.key(code),
            None => return true,
        };
        match cmd {
            ConfirmCmd::Close => self.confirm = None,
            ConfirmCmd::Confirm => {
                if let Some(s) = self.confirm.take() {
                    self.send_invoke(s.pending);
                }
            }
            ConfirmCmd::None => {}
        }
        true
    }

    /// Keys while the type-target modal is open — Enter on an exact
    /// id match invokes; Esc cancels.
    fn on_type_target_key(&mut self, code: KeyCode) -> bool {
        let cmd = match self.type_target.as_mut() {
            Some(s) => s.key(code),
            None => return true,
        };
        match cmd {
            TypeTargetCmd::Close => self.type_target = None,
            TypeTargetCmd::Submit => {
                if let Some(s) = self.type_target.take() {
                    self.send_invoke(s.pending);
                }
            }
            TypeTargetCmd::None => {}
        }
        true
    }

    /// First view of the first driver — the default landing.
    fn default_target(&self) -> Option<String> {
        self.drivers.first().and_then(|d| {
            self.views
                .iter()
                .find(|v| v.driver == d.kind)
                .map(|v| v.namespaced.clone())
                .or_else(|| Some(d.kind.clone()))
        })
    }

    /// Route → namespaced view target, for chrome entries.
    fn route_target(&self, route: &str) -> Option<String> {
        self.routes.iter().find_map(|r| {
            if r.route == route && matches!(r.kind, RouteKind::View) {
                Some(r.target.clone())
            } else {
                None
            }
        })
    }

    /// Rebuild the top tab bar: dashboard first, then navbar
    /// contributions — or, when no driver contributes navbar chrome,
    /// one tab per driver (the built-in fallback).
    fn rebuild_tabs(&mut self) {
        self.tabs.clear();
        self.tabs.push(Tab {
            label: "dashboard".to_string(),
            target: Some("dashboard".to_string()),
            note: Vec::new(),
        });
        if !self.chrome.navbar.is_empty() {
            for e in &self.chrome.navbar {
                let c = &e.contribution;
                let target = self.route_target(&c.route);
                let note = if target.is_none() {
                    vec![
                        ("title".to_string(), c.title.clone()),
                        ("route".to_string(), c.route.clone()),
                        ("driver".to_string(), e.driver.clone()),
                        (
                            "note".to_string(),
                            "driver-owned route — not resolvable to a view".to_string(),
                        ),
                    ]
                } else {
                    Vec::new()
                };
                self.tabs.push(Tab {
                    label: c.title.clone(),
                    target,
                    note,
                });
            }
        } else {
            for d in &self.drivers {
                let target = self
                    .views
                    .iter()
                    .find(|v| v.driver == d.kind)
                    .map(|v| v.namespaced.clone());
                self.tabs.push(Tab {
                    label: format!("{} ({})", d.title, d.views),
                    target,
                    note: vec![("driver".to_string(), d.kind.clone())],
                });
            }
        }
    }

    fn resolve(&mut self, target: &str) {
        self.resolve_with_params(target, Params::new());
    }

    /// [`App::resolve`] with explicit view params — route `?query`
    /// args and `host.filter` submissions land here. Params are
    /// remembered in `view_params` so refreshes and the command
    /// layer's interpolation context see them.
    fn resolve_with_params(&mut self, target: &str, params: Params) {
        self.form_session = None;
        self.palette = None;
        self.confirm = None;
        self.type_target = None;
        self.filter = None;
        self.pending_key = None;
        self.auto_refresh = None; // re-armed when the new view lands
        self.view_params = params.clone();
        self.apply_actions(Vec::new(), Vec::new());
        if target == "dashboard" {
            self.current = Some(Canvas::Dashboard);
            self.load_error = None;
            self.loading = None;
            self.sel_pos = 0;
            self.scroll = (0, 0);
            self.detail = None;
            self.pane = Pane::Canvas;
            self.status = "dashboard".to_string();
            return;
        }
        self.loading = Some(target.to_string());
        self.load_error = None;
        self.status = format!("loading {target}…");
        let _ = self.worker.tx.send(Cmd::Resolve {
            target: target.to_string(),
            refresh: false,
            params,
        });
    }

    fn refresh(&mut self) {
        match &self.current {
            Some(Canvas::View(_)) if !self.last_target.is_empty() => {
                let t = self.last_target.clone();
                self.loading = Some(t.clone());
                self.load_error = None;
                self.status = format!("refreshing {t}…");
                let _ = self.worker.tx.send(Cmd::Resolve {
                    target: t,
                    refresh: true,
                    params: self.view_params.clone(),
                });
            }
            Some(Canvas::Dashboard) => {
                let _ = self.worker.tx.send(Cmd::Reload);
                self.status = "reloading chrome…".to_string();
            }
            _ => {
                self.status = "nothing to refresh".to_string();
            }
        }
    }

    /// The auto-refresh schedule for a freshly resolved view —
    /// `(interval, first_deadline)` from `ViewSpec.refresh_ms`, or
    /// `None` when the hint is absent or outside
    /// [`MIN_REFRESH`]/[`MAX_REFRESH`].
    fn refresh_schedule(rv: &ResolvedView) -> Option<(Duration, Instant)> {
        let d = Duration::from_millis(rv.result.view.refresh_ms?);
        (d >= MIN_REFRESH && d <= MAX_REFRESH).then(|| (d, Instant::now() + d))
    }

    /// True while an overlay owns the keyboard/screen — palette,
    /// the danger modals, the filter input, the form editor, or
    /// help. Auto-refresh must not swap the canvas underneath a
    /// dialog the user is answering.
    fn overlay_open(&self) -> bool {
        self.palette.is_some()
            || self.confirm.is_some()
            || self.type_target.is_some()
            || self.filter.is_some()
            || self.form_session.is_some()
            || self.help
    }

    /// Fire the scheduled auto-refresh when its deadline passes.
    /// The deadline always slides a full interval forward — a
    /// skipped tick (overlay open, resolve in flight) waits rather
    /// than bursting on dismiss. The re-resolve goes out quietly
    /// (no status/loading churn every interval); it is the same
    /// `Cmd::Resolve { refresh: true }` the manual `r` path sends,
    /// so the host's view cache is invalidated identically.
    fn maybe_auto_refresh(&mut self) {
        let Some((interval, deadline)) = self.auto_refresh else {
            return;
        };
        if Instant::now() < deadline {
            return;
        }
        self.auto_refresh = Some((interval, Instant::now() + interval));
        if self.overlay_open() || self.loading.is_some() {
            return;
        }
        if matches!(self.current, Some(Canvas::View(_))) && !self.last_target.is_empty() {
            let _ = self.worker.tx.send(Cmd::Resolve {
                target: self.last_target.clone(),
                refresh: true,
                params: self.view_params.clone(),
            });
        }
    }

    /// Enter on a selectable canvas row: open the detail pane, or —
    /// on the dashboard — jump to the row's resolve target.
    fn activate_selection(&mut self) {
        let Some(model) = self.model_for_input() else {
            return;
        };
        let Some(&row_idx) = model.selectables.get(self.sel_pos) else {
            return;
        };
        let Some(sel_id) = model.rows[row_idx].sel else {
            return;
        };
        let Some(sel) = model.selections.get(sel_id).cloned() else {
            return;
        };
        // Dashboard rows carry jump targets in their fields.
        if matches!(self.current, Some(Canvas::Dashboard)) {
            if let Some(t) = field(&sel, "resolves to").or_else(|| field(&sel, "view")) {
                self.resolve(&t);
                return;
            }
        }
        // `ViewSpec.item_route` — Enter/click on a tree item
        // navigates to the interpolated route (`{item.*}` = the
        // item) instead of opening the detail pane. Rows without an
        // item payload (tables, kv, notes) keep the detail pane.
        if let Some(Canvas::View(rv)) = &self.current {
            if let Some(template) = rv.result.view.item_route.clone() {
                if let Some(item) = sel.item.clone() {
                    let route = self.interpolate_route(&template, Some(&item));
                    self.navigate_to_route(&route);
                    return;
                }
            }
        }
        // Form field / submit rows open the edit session instead of
        // the detail pane.
        if let Some(mark) = model.form_marks.iter().find(|m| m.row == row_idx).copied() {
            self.open_form_session(mark, &model);
            return;
        }
        self.detail = Some(sel);
        self.detail_scroll = 0;
        self.pane = Pane::Detail;
    }

    /// Enter on a form-marked canvas row: open the edit overlay for
    /// the marked form, focused on the marked field (or the first
    /// editable field for the submit row).
    fn open_form_session(&mut self, mark: FormMark, model: &Model) {
        let Some(spec) = model.forms.get(mark.form).cloned() else {
            return;
        };
        let Some(Canvas::View(rv)) = &self.current else {
            return;
        };
        let session = FormSession::open(spec, rv.namespaced.clone(), mark.field);
        self.status = format!("editing {} — Enter submits, Esc closes", session.spec.title);
        self.form_session = Some(session);
    }

    /// Enter inside the session → `submit` wire op (after a local
    /// required-field check). Sessions tagged with an action
    /// (auto-forms) validate against the action's `ParamSchema`s and
    /// route through the danger gate to `invoke` instead.
    fn submit_session(&mut self, values: Params) {
        let action_ui = {
            let Some(s) = &mut self.form_session else {
                return;
            };
            let missing = s.required_errors();
            if !missing.is_empty() {
                s.errors = missing;
                s.message = Some("missing required fields".to_string());
                return;
            }
            s.action.clone()
        };
        if let Some(ui) = action_ui {
            let params = coerce_params(&ui.action, values);
            let errors = pavo_core::validate_params(&ui.action, &params);
            if !errors.is_empty() {
                if let Some(s) = &mut self.form_session {
                    s.errors = errors
                        .iter()
                        .map(|(k, v)| {
                            (
                                k.clone(),
                                v.as_str()
                                    .map(str::to_string)
                                    .unwrap_or_else(|| v.to_string()),
                            )
                        })
                        .collect();
                    s.message = Some("invalid params".to_string());
                }
                return;
            }
            self.form_session = None;
            self.gate_action(ui, params);
            return;
        }
        let Some(s) = &mut self.form_session else {
            return;
        };
        s.busy = true;
        s.message = None;
        s.errors.clear();
        let target = s.target.clone();
        let form = s.spec.id.clone();
        let _ = self.worker.tx.send(Cmd::Submit {
            target,
            form,
            values,
        });
        self.status = "submitting…".to_string();
    }

    /// Keys while the form editor is open — the session owns the
    /// keyboard (printable chars edit, they never trigger app keys).
    fn on_form_key(&mut self, code: KeyCode) -> bool {
        let cmd = match self.form_session.as_mut() {
            Some(s) => s.key(code),
            None => return true,
        };
        match cmd {
            SessionCmd::Close => self.form_session = None,
            SessionCmd::Submit(values) => self.submit_session(values),
            SessionCmd::None => {}
        }
        true
    }

    fn model_for_input(&self) -> Option<Model> {
        match &self.current {
            Some(Canvas::View(rv)) => Some(model::build_view_with_plugins(
                &rv.result.view.root,
                &rv.result.data,
                self.canvas_w,
                &self.plugin_renders,
            )),
            Some(Canvas::Dashboard) => Some(model::build_dashboard(
                &self.views,
                &self.chrome,
                &|r| self.route_target(r),
                self.canvas_w,
            )),
            Some(Canvas::Note { title, fields }) => Some(model::build_note(title, fields, false)),
            None => None,
        }
    }

    /// The model the canvas should draw — errors/loading take
    /// precedence over stale content.
    fn canvas_model(&self) -> Model {
        if let Some((target, msg)) = &self.load_error {
            return model::build_note(
                &format!("error resolving {target}"),
                &[
                    ("error".to_string(), msg.clone()),
                    ("hint".to_string(), "r — retry · q — quit".to_string()),
                ],
                true,
            );
        }
        if let Some(m) = self.model_for_input() {
            return m;
        }
        match &self.loading {
            Some(t) => model::build_loading(t),
            None => model::build_note(
                "pavo-tui",
                &[(
                    "hint".to_string(),
                    "no drivers discovered — set TAMIAS_HOME or PATH".to_string(),
                )],
                false,
            ),
        }
    }

    fn move_sel(&mut self, delta: isize) {
        let Some(m) = self.model_for_input() else {
            return;
        };
        let n = m.selectables.len();
        if n == 0 {
            return;
        }
        self.sel_pos = (self.sel_pos as isize + delta).rem_euclid(n as isize) as usize;
        // Vertical visibility is enforced in draw() against the real
        // canvas rect.
    }

    /// Key entry point — tests call this (no modifiers); the event
    /// loop calls [`App::on_key_event`] with the full event.
    #[allow(dead_code)] // tests drive keys through this shim
    fn on_key(&mut self, code: KeyCode) -> bool {
        self.on_key_event(KeyEvent::new(code, KeyModifiers::NONE))
    }

    /// Key dispatch with modifiers — plain chars go through the
    /// chord/sequence engine; modified chars (`ctrl+p`) and named
    /// keys (`enter`, `f5`, `shift+tab`) match the binding table's
    /// normalized [`KeySpec`]s before falling back to built-ins.
    fn on_key_event(&mut self, ev: KeyEvent) -> bool {
        let code = ev.code;
        // A chord prefix that outlived its timeout replays as normal
        // keypresses before the new key is handled.
        if self
            .pending_key
            .as_ref()
            .is_some_and(|(_, t)| t.elapsed() > CHORD_TIMEOUT)
        {
            self.flush_pending();
        }
        // Overlays own the keyboard, topmost first: palette, the two
        // danger modals, the filter input, the form editor, then help.
        if self.palette.is_some() {
            return self.on_palette_key(code);
        }
        if self.type_target.is_some() {
            return self.on_type_target_key(code);
        }
        if self.confirm.is_some() {
            return self.on_confirm_key(code);
        }
        if self.filter.is_some() {
            return self.on_filter_key(code);
        }
        // An open form editor owns the keyboard — printable chars
        // edit fields, they must never trigger app bindings.
        if self.form_session.is_some() {
            return self.on_form_key(code);
        }
        // Help overlay captures everything except its own dismiss keys.
        if self.help {
            match code {
                KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q') => self.help = false,
                _ => {}
            }
            return true;
        }
        // Plain chars (no ctrl/alt/cmd) feed the chord engine —
        // sequences like "dd" and single-char specs live there.
        let plain_char = matches!(code, KeyCode::Char(_))
            && !ev.modifiers.intersects(
                KeyModifiers::CONTROL
                    | KeyModifiers::ALT
                    | KeyModifiers::SUPER
                    | KeyModifiers::HYPER
                    | KeyModifiers::META,
            );
        if plain_char {
            if let KeyCode::Char(c) = code {
                return self.chord_or_key(c);
            }
        }
        // Modified chars and named keys: a normalized KeySpec lookup
        // in the binding table, then the built-in bindings.
        self.flush_pending();
        if let Some(spec) = key_event_spec(code, ev.modifiers) {
            if let Some(cmd) = self.key_chords.get(&spec).cloned() {
                return self.run_command(cmd);
            }
        }
        self.key_now(code)
    }

    /// Replay a pending chord prefix as ordinary keypresses (used on
    /// timeout and when the next char can't continue the chord).
    /// Each char goes through [`App::press_char`] — a lone prefix
    /// char that is itself bound still fires its binding.
    fn flush_pending(&mut self) {
        if let Some((buf, _)) = self.pending_key.take() {
            for c in buf.chars() {
                if !self.press_char(c) {
                    break;
                }
            }
        }
    }

    /// A char pressed outside chord continuation — single-char
    /// command binding (unprotected chars only) else the built-in
    /// [`App::key_now`] bindings.
    fn press_char(&mut self, c: char) -> bool {
        let s = c.to_string();
        if !NAV_CHARS.contains(&c) {
            if let Some(cmd) = self.key_chords.get(&s).cloned() {
                return self.run_command(cmd);
            }
        }
        self.key_now(KeyCode::Char(c))
    }

    /// Char-key dispatch with chord support: a pending prefix plus
    /// `c` may complete a multi-char sequence spec; a lone char that
    /// prefixes some spec waits for [`CHORD_TIMEOUT`]; an exact
    /// single-char spec runs its command; anything else falls
    /// through to the built-in [`App::key_now`] bindings.
    /// Returns `false` when the app should quit (propagated from
    /// `q`/`Esc` or `host.quit` reached via bindings).
    fn chord_or_key(&mut self, c: char) -> bool {
        if let Some((buf, _)) = self.pending_key.take() {
            let combined = format!("{buf}{c}");
            if let Some(cmd) = self.key_chords.get(&combined).cloned() {
                return self.run_command(cmd);
            }
            if self.chord_prefixes.contains(&combined) {
                self.pending_key = Some((combined, Instant::now()));
                return true;
            }
            // Dead end — replay the buffered prefix, then treat `c`
            // fresh (it may start a chord of its own).
            for pc in buf.chars() {
                if !self.press_char(pc) {
                    return false;
                }
            }
            return self.chord_or_key(c);
        }
        let s = c.to_string();
        if self.chord_prefixes.contains(&s) {
            self.pending_key = Some((s, Instant::now()));
            return true;
        }
        // Hard-wired navigation keys win over command bindings — a
        // driver binding `q`/`j`/`d`/… can't shadow navigation.
        // (Chords starting with those chars still work: the prefix
        // waits, and the completed spec matches before `key_now`.)
        if !NAV_CHARS.contains(&c) {
            if let Some(cmd) = self.key_chords.get(&s).cloned() {
                return self.run_command(cmd);
            }
        }
        self.key_now(KeyCode::Char(c))
    }

    /// The built-in key bindings — everything that isn't an action
    /// chord or a single-char action spec lands here. Returns
    /// `false` only for the quit path.
    fn key_now(&mut self, code: KeyCode) -> bool {
        match code {
            KeyCode::Char('q') | KeyCode::Esc => {
                if self.pane == Pane::Detail {
                    self.detail = None;
                    self.pane = Pane::Canvas;
                } else {
                    return false;
                }
            }
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('r') => self.refresh(),
            KeyCode::Char('d') => self.dump_view(),
            KeyCode::Char('g') => self.dump_graph(),
            KeyCode::Char('x') => {
                self.detail = None;
                if self.pane == Pane::Detail {
                    self.pane = Pane::Canvas;
                }
            }
            KeyCode::Tab => self.cycle_pane(1),
            KeyCode::BackTab => self.cycle_pane(-1),
            KeyCode::Left => {
                if self.pane == Pane::Detail {
                    self.pane = Pane::Canvas;
                } else {
                    self.cycle_pane(-1);
                }
            }
            KeyCode::Right => self.cycle_pane(1),
            KeyCode::Char('j') | KeyCode::Down => self.nav_down(),
            KeyCode::Char('k') | KeyCode::Up => self.nav_up(),
            KeyCode::PageDown => self.page(1),
            KeyCode::PageUp => self.page(-1),
            KeyCode::Enter => self.enter(),
            KeyCode::Char('h') => self.scroll.0 = self.scroll.0.saturating_sub(4),
            KeyCode::Char('l') => self.scroll.0 += 4,
            KeyCode::Char(':') => self.open_palette(),
            _ => {}
        }
        true
    }

    fn cycle_pane(&mut self, dir: isize) {
        let order: &[Pane] = if self.detail.is_some() {
            &[Pane::Rail, Pane::Canvas, Pane::Detail]
        } else {
            &[Pane::Rail, Pane::Canvas]
        };
        let cur = order.iter().position(|p| *p == self.pane).unwrap_or(0);
        let next = (cur as isize + dir).rem_euclid(order.len() as isize) as usize;
        self.pane = order[next];
    }

    fn nav_down(&mut self) {
        match self.pane {
            Pane::Rail => {
                self.rail_sel = (self.rail_sel + 1).min(self.drivers.len().saturating_sub(1));
            }
            Pane::Canvas => self.move_sel(1),
            Pane::Detail => self.detail_scroll += 3,
        }
    }

    fn nav_up(&mut self) {
        match self.pane {
            Pane::Rail => self.rail_sel = self.rail_sel.saturating_sub(1),
            Pane::Canvas => self.move_sel(-1),
            Pane::Detail => self.detail_scroll = self.detail_scroll.saturating_sub(3),
        }
    }

    fn page(&mut self, dir: isize) {
        match self.pane {
            Pane::Canvas => {
                let n = self
                    .model_for_input()
                    .map(|m| m.selectables.len())
                    .unwrap_or(0);
                if n > 0 {
                    let step = 10isize;
                    let cur = self.sel_pos as isize;
                    let next = (cur + dir * step).clamp(0, n as isize - 1);
                    self.sel_pos = next as usize;
                }
            }
            Pane::Detail => {
                if dir > 0 {
                    self.detail_scroll += 15;
                } else {
                    self.detail_scroll = self.detail_scroll.saturating_sub(15);
                }
            }
            Pane::Rail => {}
        }
    }

    fn enter(&mut self) {
        match self.pane {
            Pane::Rail => self.open_driver(),
            Pane::Canvas => self.activate_selection(),
            Pane::Detail => self.pane = Pane::Canvas,
        }
    }

    /// Rail Enter — resolve the selected driver's first view.
    fn open_driver(&mut self) {
        let Some(d) = self.drivers.get(self.rail_sel) else {
            return;
        };
        let target = self
            .views
            .iter()
            .find(|v| v.driver == d.kind)
            .map(|v| v.namespaced.clone())
            .unwrap_or_else(|| d.kind.clone());
        self.resolve(&target);
    }

    fn on_mouse(&mut self, m: crossterm::event::MouseEvent) {
        use crossterm::event::MouseEventKind as M;
        let (col, row) = (m.column, m.row);
        match m.kind {
            M::ScrollDown => self.scroll.1 += 3,
            M::ScrollUp => self.scroll.1 = self.scroll.1.saturating_sub(3),
            M::ScrollRight => self.scroll.0 += 3,
            M::ScrollLeft => self.scroll.0 = self.scroll.0.saturating_sub(3),
            M::Down(MouseButton::Left) => {
                // Tab bar.
                if self.rects.tabs.height > 0
                    && row >= self.rects.tabs.y
                    && row < self.rects.tabs.y + self.rects.tabs.height
                {
                    if let Some((_, _, i)) = self
                        .tab_hits
                        .iter()
                        .find(|(x0, x1, _)| col >= *x0 && col < *x1)
                    {
                        self.activate_tab(*i);
                    }
                    return;
                }
                // Rail.
                if in_rect(self.rects.rail, col, row) {
                    let inner_top = self.rects.rail.y + 1;
                    let idx = (row.saturating_sub(inner_top)) as usize;
                    if idx < self.drivers.len() {
                        self.rail_sel = idx;
                        self.pane = Pane::Rail;
                        // Second click on the same driver opens it.
                        if self.open_armed == Some(idx) {
                            self.open_armed = None;
                            self.open_driver();
                        } else {
                            self.open_armed = Some(idx);
                        }
                    }
                    return;
                }
                self.open_armed = None;
                // Canvas rows.
                if in_rect(self.rects.canvas, col, row) {
                    self.click_canvas(col, row);
                }
            }
            _ => {}
        }
    }

    /// Click on the canvas: select the row; a second click on an
    /// already-selected row opens the detail pane.
    fn click_canvas(&mut self, col: u16, row: u16) {
        let Some(m) = self.model_for_input() else {
            return;
        };
        let inner_top = self.rects.canvas.y + 1; // border
        let inner_left = self.rects.canvas.x + 1;
        if row < inner_top || col < inner_left {
            return;
        }
        let row_idx = self.scroll.1 + (row - inner_top) as usize;
        let Some(ri) = m.rows.get(row_idx) else {
            return;
        };
        if ri.sel.is_none() {
            return;
        }
        // Position of this row among selectables.
        let Some(pos) = m.selectables.iter().position(|&i| i == row_idx) else {
            return;
        };
        if pos == self.sel_pos {
            // Second click on the selected row opens the detail pane.
            self.activate_selection();
        } else {
            self.sel_pos = pos;
            self.pane = Pane::Canvas;
        }
        self.open_armed = None;
    }

    fn activate_tab(&mut self, i: usize) {
        let Some(tab) = self.tabs.get(i).cloned() else {
            return;
        };
        match tab.target {
            Some(t) => self.resolve(&t),
            None => {
                self.current = Some(Canvas::Note {
                    title: tab.label.clone(),
                    fields: tab.note,
                });
                self.load_error = None;
                self.loading = None;
                self.sel_pos = 0;
                self.scroll = (0, 0);
                self.detail = None;
                self.form_session = None;
                self.palette = None;
                self.confirm = None;
                self.type_target = None;
                self.filter = None;
                self.pending_key = None;
                self.apply_actions(Vec::new(), Vec::new());
                self.pane = Pane::Canvas;
                self.status = tab.label.clone();
            }
        }
    }

    /// `d` — dump the current view as pretty JSON to a temp file.
    fn dump_view(&mut self) {
        match self.dump_view_json() {
            Some(path) => self.status = format!("wrote {}", path.display()),
            None => self.status = "nothing to dump (not a driver view)".to_string(),
        }
    }

    /// Write the current view's `{view, data}` JSON to a tmp file —
    /// shared by `d` (dump) and `host.editor`. `None` off-view.
    fn dump_view_json(&self) -> Option<std::path::PathBuf> {
        let Some(Canvas::View(rv)) = &self.current else {
            return None;
        };
        let value = serde_json::json!({
            "view": rv.result.view,
            "data": rv.result.data,
        });
        let path = std::env::temp_dir().join(format!(
            "pavo-view-{}.json",
            rv.namespaced.replace(':', "-")
        ));
        serde_json::to_string_pretty(&value)
            .map_err(std::io::Error::other)
            .and_then(|s| std::fs::write(&path, s))
            .ok()?;
        Some(path)
    }

    /// `g` — dump the current view's graph nodes as DOT.
    fn dump_graph(&mut self) {
        let Some(m) = self.model_for_input() else {
            self.status = "nothing to dump".to_string();
            return;
        };
        let Some(dot) = m.graph_dot else {
            self.status = "no graph nodes in this view".to_string();
            return;
        };
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("pavo-graph-{nanos}.dot"));
        match std::fs::write(&path, dot) {
            Ok(()) => self.status = format!("wrote {}", path.display()),
            Err(e) => self.status = format!("dump failed: {e}"),
        }
    }

    /// Draw the whole UI.
    pub fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // tab bar
                Constraint::Min(3),    // main
                Constraint::Length(1), // status bar
            ])
            .split(area);
        self.rects.tabs = chunks[0];
        self.rects.status = chunks[2];

        self.draw_tabs(f, chunks[0]);
        self.draw_main(f, chunks[1]);
        self.draw_status(f, chunks[2]);
        if self.help {
            self.draw_help(f, area);
        }
        if self.form_session.is_some() {
            self.draw_form(f, area);
        }
        if let Some(s) = &self.palette {
            self.draw_palette(f, area, s);
        }
        if let Some(s) = &self.confirm {
            self.draw_confirm(f, area, s);
        }
        if let Some(s) = &self.type_target {
            self.draw_type_target(f, area, s);
        }
        if let Some(s) = &self.filter {
            self.draw_filter(f, area, s);
        }
    }

    fn draw_tabs(&mut self, f: &mut Frame, area: Rect) {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(0), Constraint::Length(12)])
            .split(area);
        self.tab_hits.clear();
        let mut spans: Vec<Span> = vec![Span::raw(" ")];
        let mut x = area.x + 1;
        for (i, tab) in self.tabs.iter().enumerate() {
            let label = format!(" {} ", tab.label);
            let w = label.chars().count() as u16;
            // Active tab = the one matching the current target.
            let active = match (&self.current, &tab.target) {
                (Some(Canvas::Dashboard), Some(t)) => t == "dashboard",
                (Some(Canvas::View(rv)), Some(t)) => rv.namespaced == *t,
                _ => false,
            };
            let style = if active {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };
            spans.push(Span::styled(label, style));
            self.tab_hits.push((x, x + w, i));
            x += w;
            if x >= cols[0].x + cols[0].width {
                break;
            }
        }
        Paragraph::new(Line::from(spans)).render(cols[0], f.buffer_mut());
        let hint = Paragraph::new(Line::from(Span::styled(
            " [?] help ",
            Style::default().fg(Color::DarkGray),
        )));
        hint.render(cols[1], f.buffer_mut());
        self.rects.tabs = area;
    }

    fn draw_main(&mut self, f: &mut Frame, area: Rect) {
        let rail_w = (area.width / 5).clamp(12, 28);
        let detail_w = if self.detail.is_some() {
            (area.width * 3 / 10).min(area.width.saturating_sub(rail_w + 10))
        } else {
            0
        };
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(rail_w),
                Constraint::Min(10),
                Constraint::Length(detail_w),
            ])
            .split(area);
        self.rects.rail = cols[0];
        self.rects.canvas = cols[1];
        self.rects.detail = cols[2];

        self.draw_rail(f, cols[0]);
        self.draw_canvas(f, cols[1]);
        if self.detail.is_some() {
            self.draw_detail(f, cols[2]);
        }
    }

    fn draw_rail(&self, f: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled(
                " drivers ",
                Style::default().fg(Color::Yellow),
            ));
        let inner = block.inner(area);
        block.render(area, f.buffer_mut());

        let focused = self.pane == Pane::Rail;
        let lines: Vec<Line> = self
            .drivers
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let selected = i == self.rail_sel;
                let marker = if selected { "▸" } else { " " };
                let mut spans = vec![
                    Span::styled(
                        format!("{marker} "),
                        if selected {
                            Style::default().fg(Color::Cyan)
                        } else {
                            Style::default().fg(Color::DarkGray)
                        },
                    ),
                    Span::styled(
                        d.title.clone(),
                        if selected {
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default()
                        },
                    ),
                    Span::styled(
                        format!(
                            "  {} · {} view{}",
                            d.kind,
                            d.views,
                            if d.views == 1 { "" } else { "s" }
                        ),
                        Style::default().fg(Color::DarkGray),
                    ),
                ];
                if selected && focused {
                    // Highlight the whole row via a trailing-space span
                    // is unreliable; style each span's bg instead.
                    for s in &mut spans {
                        s.style = s.style.bg(Color::Rgb(40, 40, 60));
                    }
                }
                Line::from(spans)
            })
            .collect();
        Paragraph::new(Text::from(lines)).render(inner, f.buffer_mut());
    }

    fn draw_canvas(&mut self, f: &mut Frame, area: Rect) {
        let title = match &self.current {
            Some(Canvas::View(rv)) => format!(" {} ", rv.namespaced),
            Some(Canvas::Dashboard) => " dashboard ".to_string(),
            Some(Canvas::Note { title, .. }) => format!(" {title} "),
            None => " pavo ".to_string(),
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled(title, Style::default().fg(Color::Yellow)));
        let inner = block.inner(area);
        block.render(area, f.buffer_mut());

        let model = self.canvas_model();
        // Clamp scroll to content.
        let max_y = model.rows.len().saturating_sub(inner.height as usize);
        self.scroll.1 = self.scroll.1.min(max_y);
        // Keep the selected row visible.
        if let Some(&ri) = model
            .selectables
            .get(self.sel_pos)
            .filter(|_| self.pane == Pane::Canvas || self.detail.is_some())
        {
            if ri < self.scroll.1 {
                self.scroll.1 = ri;
            } else if ri >= self.scroll.1 + inner.height as usize {
                self.scroll.1 = ri + 1 - inner.height as usize;
            }
        }
        // Clamp horizontal scroll crudely: max line width vs inner.
        let max_x = model
            .rows
            .iter()
            .map(|r| r.text.chars().count())
            .max()
            .unwrap_or(0)
            .saturating_sub(inner.width as usize);
        self.scroll.0 = self.scroll.0.min(max_x);
        self.canvas_w = inner.width as usize;

        let selected_row = model
            .selectables
            .get(self.sel_pos)
            .copied()
            .filter(|_| self.pane != Pane::Rail);

        let lines: Vec<Line> = model
            .rows
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let style = row_style(r.style);
                let is_sel = selected_row == Some(i);
                let spans = vec![Span::styled(r.text.clone(), style)];
                if is_sel {
                    Line::from(spans).style(Style::default().bg(Color::Rgb(40, 40, 60)))
                } else {
                    Line::from(spans)
                }
            })
            .collect();
        Paragraph::new(Text::from(lines))
            .scroll((self.scroll.0 as u16, self.scroll.1 as u16))
            .render(inner, f.buffer_mut());
    }

    fn draw_detail(&self, f: &mut Frame, area: Rect) {
        let Some(sel) = &self.detail else {
            return;
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled(
                format!(" {} ", sel.title),
                Style::default().fg(Color::Yellow),
            ));
        let inner = block.inner(area);
        block.render(area, f.buffer_mut());

        let mut lines: Vec<Line> = Vec::new();
        for (k, v) in &sel.fields {
            for (j, vl) in v.split('\n').enumerate() {
                if k.is_empty() {
                    lines.push(Line::from(Span::styled(
                        vl.to_string(),
                        Style::default().fg(Color::Gray),
                    )));
                } else if j == 0 {
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("{k}: "),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                        Span::raw(vl.to_string()),
                    ]));
                } else {
                    lines.push(Line::from(Span::raw(format!("    {vl}"))));
                }
            }
        }
        let max_scroll = lines.len().saturating_sub(inner.height as usize);
        let sy = self.detail_scroll.min(max_scroll) as u16;
        Paragraph::new(Text::from(lines))
            .scroll((0, sy))
            .render(inner, f.buffer_mut());
    }

    fn draw_status(&self, f: &mut Frame, area: Rect) {
        let right = self
            .chrome
            .statusbar
            .iter()
            .map(|e| e.contribution.title.clone())
            .collect::<Vec<_>>()
            .join(" · ");
        let right = if right.is_empty() {
            "pavo-tui".to_string()
        } else {
            right
        };
        let rw = (right.chars().count() as u16 + 2).min(area.width / 2);
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(0), Constraint::Length(rw)])
            .split(area);
        let mut spans = vec![Span::styled(" ", Style::default())];
        // Auto-refresh countdown — a dim `⟳ Ns` ahead of the status
        // text so live views are visibly on a timer.
        if let Some((_, deadline)) = self.auto_refresh {
            let secs = deadline.saturating_duration_since(Instant::now()).as_secs();
            spans.push(Span::styled(
                format!("⟳ {secs}s "),
                Style::default().fg(Color::DarkGray),
            ));
        }
        spans.push(Span::styled(
            self.status.clone(),
            Style::default().fg(Color::DarkGray),
        ));
        let left = Paragraph::new(Line::from(spans));
        left.render(cols[0], f.buffer_mut());
        let r = Paragraph::new(Line::from(Span::styled(
            format!(" {right} "),
            Style::default().fg(Color::DarkGray),
        )));
        r.render(cols[1], f.buffer_mut());
    }

    /// The form-editing overlay: every field of the open session's
    /// form with live values — text fields show a cursor line,
    /// `select`/`bool` show their discrete state, `editable:false`
    /// fields render read-only. Submit result `message`/`errors`
    /// surface inline.
    fn draw_form(&self, f: &mut Frame, area: Rect) {
        let Some(s) = &self.form_session else {
            return;
        };
        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(Span::styled(
            format!(
                " {} — {} ({}) ",
                s.spec.title, s.spec.submit_label, s.spec.submit_op
            ),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
        for (i, field) in s.fields.iter().enumerate() {
            let focused = s.focused() == Some(i) && field.spec.editable;
            let marker = if focused { "▸" } else { " " };
            let mut head = format!("{marker} {} [{}]", field.spec.label, field.spec.kind);
            if field.spec.required {
                head.push('*');
            }
            if !field.spec.editable {
                head.push_str(" (read-only)");
            }
            lines.push(Line::from(vec![
                Span::styled(
                    head,
                    if focused {
                        Style::default().add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    },
                ),
                Span::styled(
                    format!("  {}", field.spec.key),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
            if field.spec.editable {
                lines.push(field_value_line(field, focused));
            } else {
                lines.push(Line::from(Span::styled(
                    format!("    {}", field.buf.text),
                    Style::default().fg(Color::DarkGray),
                )));
            }
            if let Some(hint) = &field.spec.hint {
                lines.push(Line::from(Span::styled(
                    format!("    — {hint}"),
                    Style::default().fg(Color::DarkGray),
                )));
            }
            if let Some(err) = s.errors.get(&field.spec.key) {
                lines.push(Line::from(Span::styled(
                    format!("    ! {err}"),
                    Style::default().fg(Color::Red),
                )));
            }
        }
        lines.push(Line::from(""));
        if let Some(msg) = &s.message {
            let bad = !s.errors.is_empty() || msg.contains("fail");
            lines.push(Line::from(Span::styled(
                format!(" {msg}"),
                Style::default().fg(if bad { Color::Red } else { Color::Green }),
            )));
        }
        let footer = if s.busy {
            " submitting… (Esc still closes) ".to_string()
        } else {
            format!(
                " Enter {} · ↑↓/j/k/Tab fields · ←→/hl edit · Esc close ",
                s.spec.submit_label
            )
        };
        lines.push(Line::from(Span::styled(
            footer,
            Style::default().fg(Color::DarkGray),
        )));

        let w = 64u16.min(area.width.saturating_sub(4));
        let h = (lines.len() as u16 + 2)
            .min(area.height.saturating_sub(2))
            .max(3);
        let x = area.x + (area.width.saturating_sub(w)) / 2;
        let y = area.y + (area.height.saturating_sub(h)) / 2;
        let rect = Rect::new(x, y, w, h);
        f.render_widget(Clear, rect);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(Span::styled(
                format!(" {} ", s.spec.title),
                Style::default().fg(Color::Yellow),
            ));
        Paragraph::new(Text::from(lines))
            .block(block)
            .render(rect, f.buffer_mut());
    }

    /// The command palette: filter line on top, fuzzy-matched
    /// entries below (section headers when the group changes),
    /// detail of the selected entry, footer hints.
    fn draw_palette(&self, f: &mut Frame, area: Rect, s: &PaletteSession) {
        let w = 76u16.min(area.width.saturating_sub(4));
        let inner_h = area.height.saturating_sub(4) as usize;
        let mut lines: Vec<Line> = Vec::new();
        // Input line with a cursor block at the edit position.
        let mut input = vec![Span::styled("> ", Style::default().fg(Color::Cyan))];
        input.extend(line_edit_spans(&s.edit, Style::default()));
        lines.push(Line::from(input));
        lines.push(Line::from(""));
        // Entries — section header whenever the group changes,
        // capped so the box never overflows.
        let mut last_section: Option<String> = None;
        let mut shown = 0usize;
        for (row, &i) in s.matches.iter().enumerate() {
            if row < s.sel.saturating_sub(inner_h.saturating_sub(1)) {
                continue; // scrolled off the top
            }
            if shown >= inner_h {
                break;
            }
            let e = &s.entries[i];
            let section = e.section();
            if section != last_section {
                last_section = section.clone();
                if let Some(sec) = &section {
                    lines.push(Line::from(Span::styled(
                        format!(" {sec} "),
                        Style::default().fg(Color::DarkGray),
                    )));
                    shown += 1;
                    if shown >= inner_h {
                        break;
                    }
                }
            }
            let selected = row == s.sel;
            let marker = if selected { "▸" } else { " " };
            let mut spans = vec![
                Span::styled(
                    format!("{marker} "),
                    if selected {
                        Style::default().fg(Color::Cyan)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    },
                ),
                Span::styled(
                    e.label().to_string(),
                    if selected {
                        Style::default().add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    },
                ),
                Span::styled(
                    format!("  {}", e.detail()),
                    Style::default().fg(Color::DarkGray),
                ),
            ];
            if selected {
                for sp in &mut spans {
                    sp.style = sp.style.bg(Color::Rgb(40, 40, 60));
                }
            }
            lines.push(Line::from(spans));
            shown += 1;
        }
        if s.matches.is_empty() {
            lines.push(Line::from(Span::styled(
                "  (no matches)",
                Style::default().fg(Color::DarkGray),
            )));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " ↑↓/jk select · Enter run · Esc close ",
            Style::default().fg(Color::DarkGray),
        )));
        let h = (lines.len() as u16 + 2)
            .min(area.height.saturating_sub(2))
            .max(4);
        let x = area.x + (area.width.saturating_sub(w)) / 2;
        let y = area.y + (area.height.saturating_sub(h)) / 2;
        let rect = Rect::new(x, y, w, h);
        f.render_widget(Clear, rect);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(Span::styled(
                " actions ",
                Style::default().fg(Color::Yellow),
            ));
        Paragraph::new(Text::from(lines))
            .block(block)
            .render(rect, f.buffer_mut());
    }

    /// The `danger: confirm` modal — action label, optional
    /// description, context hint; y/Enter confirms.
    /// The `host.filter` overlay — a centered prompt line echoing the
    /// active param name + the typed filter text with a cursor.
    fn draw_filter(&self, f: &mut Frame, area: Rect, s: &FilterSession) {
        let w = (area.width.saturating_sub(4)).clamp(20, 72);
        let h = 3u16;
        let x = area.x + area.width.saturating_sub(w) / 2;
        let y = area.y + area.height.saturating_sub(h) / 2;
        let rect = Rect::new(x, y, w, h);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(format!(" filter · {} ", s.param));
        f.render_widget(Clear, rect);
        f.render_widget(block, rect);
        let inner = Rect::new(rect.x + 1, rect.y + 1, rect.width.saturating_sub(2), 1);
        let buf = &s.edit;
        let prefix: String = buf
            .text
            .chars()
            .take(buf.cursor.min(buf.text.chars().count()))
            .collect();
        let suffix: String = buf
            .text
            .chars()
            .skip(buf.cursor.min(buf.text.chars().count()))
            .collect();
        let line = Line::from(vec![
            Span::raw(format!(" {prefix}")),
            Span::styled("█".to_string(), Style::default().bg(Color::Cyan)),
            Span::raw(suffix),
        ])
        .style(Style::default().fg(Color::White));
        f.render_widget(Paragraph::new(line).block(Block::default()), inner);
    }

    fn draw_confirm(&self, f: &mut Frame, area: Rect, s: &ConfirmSession) {
        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(Span::styled(
            format!(" Run “{}”? ", s.pending.ui.action.label),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
        if let Some(d) = &s.pending.ui.action.description {
            lines.push(Line::from(Span::styled(
                format!(" {d}"),
                Style::default().fg(Color::Gray),
            )));
        }
        if let Some(t) = &s.pending.ui.action.target {
            lines.push(Line::from(Span::styled(
                format!(" context: {t}"),
                Style::default().fg(Color::DarkGray),
            )));
        }
        if !s.pending.params.is_empty() {
            lines.push(Line::from(Span::styled(
                format!(" params: {}", s.pending.params.len()),
                Style::default().fg(Color::DarkGray),
            )));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " [y] confirm · [n/Esc] cancel ",
            Style::default().fg(Color::DarkGray),
        )));
        let w = 56u16.min(area.width.saturating_sub(4));
        let h = (lines.len() as u16 + 2)
            .min(area.height.saturating_sub(2))
            .max(4);
        let x = area.x + (area.width.saturating_sub(w)) / 2;
        let y = area.y + (area.height.saturating_sub(h)) / 2;
        let rect = Rect::new(x, y, w, h);
        f.render_widget(Clear, rect);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow))
            .title(" confirm ");
        Paragraph::new(Text::from(lines))
            .block(block)
            .render(rect, f.buffer_mut());
    }

    /// The `danger: type-target` modal — "Type <id> to confirm"
    /// with the input buffer and a mismatch error line.
    fn draw_type_target(&self, f: &mut Frame, area: Rect, s: &TypeTargetSession) {
        let mut lines: Vec<Line> = Vec::new();
        let prompt = match &s.expect {
            Some(id) => format!(" Type {id} to confirm: "),
            None => " Type to confirm: ".to_string(),
        };
        let mut prompt = vec![Span::styled(
            prompt,
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )];
        prompt.extend(line_edit_spans(&s.edit, Style::default()));
        lines.push(Line::from(prompt));
        if let Some(e) = &s.error {
            lines.push(Line::from(Span::styled(
                format!(" {e}"),
                Style::default().fg(Color::Red),
            )));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " Enter confirm · Esc cancel ",
            Style::default().fg(Color::DarkGray),
        )));
        let w = 56u16.min(area.width.saturating_sub(4));
        let h = (lines.len() as u16 + 2)
            .min(area.height.saturating_sub(2))
            .max(5);
        let x = area.x + (area.width.saturating_sub(w)) / 2;
        let y = area.y + (area.height.saturating_sub(h)) / 2;
        let rect = Rect::new(x, y, w, h);
        f.render_widget(Clear, rect);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Red))
            .title(" type to confirm ");
        Paragraph::new(Text::from(lines))
            .block(block)
            .render(rect, f.buffer_mut());
    }

    fn draw_help(&self, f: &mut Frame, area: Rect) {
        let mut lines: Vec<Line> = vec![
            Line::from(Span::styled(
                " keys ",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("  j/k · ↑/↓     navigate selectable rows / rail"),
            Line::from("  Tab · ←/→    switch panes (rail → canvas → detail)"),
            Line::from("  Enter        open driver view / row detail / jump / edit form"),
            Line::from("  in a form:   type to edit · ↑↓/j/k/Tab fields ·"),
            Line::from("               ←→/h/l cycle select·bool · Enter submit · Esc"),
            Line::from("  PgUp/PgDn    page through selection"),
            Line::from("  h/l · wheel  horizontal / vertical scroll"),
            Line::from("  r            refresh current view / reload chrome"),
            Line::from("  d            dump view JSON → tmp file"),
            Line::from("  g            dump graph DOT → tmp file"),
            Line::from("  :            command palette (actions + menus + builtins)"),
            Line::from("  / · F        filter current view (filter_param)"),
            Line::from("  x            close detail pane"),
            Line::from("  ?            this help"),
            Line::from("  q · Esc      quit (Esc closes detail/help first)"),
        ];
        // The effective binding table — commands include host
        // builtins (`host.*`), menu `keys[]`, action `keys[]`, and
        // user `keys.toml` overrides. This is the discoverable half
        // of the unified command layer.
        let mut commands: Vec<String> = Vec::new();
        for b in &self.bindings {
            for key in &b.keys {
                commands.push(format!("  {key:<10} {}", b.id));
            }
        }
        commands.sort();
        commands.dedup();
        if !commands.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                " keybindings ",
                Style::default().add_modifier(Modifier::BOLD),
            )));
            lines.extend(
                commands
                    .into_iter()
                    .map(|t| Line::from(Span::styled(t, Style::default().fg(Color::Gray)))),
            );
        }
        let bound: Vec<&UiAction> = self
            .actions
            .iter()
            .filter(|u| u.scope != ActionScope::Item && !u.action.keys.is_empty())
            .collect();
        if !bound.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                " view actions ",
                Style::default().add_modifier(Modifier::BOLD),
            )));
            for u in bound {
                let keys = u.action.keys.join(" / ");
                let mut text = format!("  {keys:<10} {}", u.action.label);
                if u.action.danger != DangerLevel::None {
                    text.push_str("  ⚠");
                }
                lines.push(Line::from(Span::styled(
                    text,
                    Style::default().fg(Color::Gray),
                )));
            }
        }
        let text = Text::from(lines);
        let w = 52u16.min(area.width.saturating_sub(4));
        let h = u16::try_from(text.height() + 2)
            .unwrap_or(u16::MAX)
            .min(area.height.saturating_sub(2))
            .max(4);
        let x = area.x + (area.width.saturating_sub(w)) / 2;
        let y = area.y + (area.height.saturating_sub(h)) / 2;
        let rect = Rect::new(x, y, w, h);
        f.render_widget(Clear, rect);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(" help ");
        Paragraph::new(text)
            .block(block)
            .render(rect, f.buffer_mut());
    }
}

/// An empty [`DriverManifest`] — the `ResolveContext.manifest`
/// fallback when the current driver isn't in `self.manifests`.
fn empty_manifest() -> DriverManifest {
    DriverManifest {
        name: String::new(),
        title: String::new(),
        icon: None,
        version: String::new(),
        verbs: Vec::new(),
        capabilities: Vec::new(),
        deps: None,
        contributes: Default::default(),
        views: Vec::new(),
        renderers: Vec::new(),
        actions: Vec::new(),
    }
}

fn in_rect(r: Rect, col: u16, row: u16) -> bool {
    col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}

/// Parse a route's `?query` string into view [`Params`] — `&`-separated
/// `k=v` pairs, `%XX`/`+` decoding, values kept as strings unless
/// they parse as JSON objects/arrays (route templates interpolate
/// scalars; drivers coerce). Bare `k` pairs get an empty string.
fn parse_route_query(query: &str) -> Params {
    let mut params = Params::new();
    if query.is_empty() {
        return params;
    }
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let key = percent_decode(k);
        if key.is_empty() {
            continue;
        }
        let raw = percent_decode(v);
        let value = if raw.starts_with('{') || raw.starts_with('[') {
            serde_json::from_str(&raw).unwrap_or(serde_json::Value::String(raw))
        } else {
            serde_json::Value::String(raw)
        };
        params.insert(key, value);
    }
    params
}

/// Minimal percent-decoding for route query strings (`%XX` → byte,
/// `+` → space). Invalid sequences pass through literally.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = &s[i + 1..i + 3];
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The value line under a form field's header in the edit overlay.
/// Text kinds render the buffer with a reversed-video cursor when
/// focused (placeholder shows dim while the buffer is empty);
/// `select`/`bool` render their discrete state.
fn field_value_line<'a>(field: &'a FieldEdit, focused: bool) -> Line<'a> {
    match field.spec.kind.as_str() {
        "select" => {
            let n = field.spec.options.len();
            let pos = field.spec.options.iter().position(|o| *o == field.buf.text);
            let current = if field.buf.text.is_empty() {
                field
                    .spec
                    .placeholder
                    .clone()
                    .unwrap_or_else(|| "—".to_string())
            } else {
                field.buf.text.clone()
            };
            let mut spans = vec![
                Span::raw(format!("    ‹{current}› ")),
                Span::styled(
                    match pos {
                        Some(i) => format!("({}/{n})", i + 1),
                        None => "(?)".to_string(),
                    },
                    Style::default().fg(Color::DarkGray),
                ),
            ];
            if !field.spec.options.is_empty() {
                spans.push(Span::styled(
                    format!("  options: {}", field.spec.options.join(" | ")),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            Line::from(spans)
        }
        "bool" => {
            let on = matches!(field.buf.text.trim(), "true" | "1" | "yes" | "on");
            Line::from(vec![
                Span::raw("    "),
                Span::styled(
                    if on { "[x]" } else { "[ ]" },
                    if focused {
                        Style::default().fg(Color::Cyan)
                    } else {
                        Style::default()
                    },
                ),
                Span::raw(format!(" {}", if on { "true" } else { "false" })),
            ])
        }
        _ => {
            // Text-like: buffer, or the placeholder while empty.
            let using_placeholder = field.buf.text.is_empty();
            let display = if using_placeholder {
                field.spec.placeholder.clone().unwrap_or_default()
            } else {
                field.buf.text.clone()
            };
            let dim = Style::default().fg(Color::DarkGray);
            let chars: Vec<char> = display.chars().collect();
            let cursor = field.buf.cursor.min(chars.len());
            let mut spans = vec![Span::raw("    ")];
            if !focused {
                spans.push(Span::styled(
                    display,
                    if using_placeholder {
                        dim
                    } else {
                        Style::default()
                    },
                ));
                return Line::from(spans);
            }
            let (before, cur, after) = if cursor < chars.len() {
                (
                    chars[..cursor].iter().collect::<String>(),
                    chars[cursor].to_string(),
                    chars[cursor + 1..].iter().collect::<String>(),
                )
            } else {
                (display.clone(), " ".to_string(), String::new())
            };
            let base = if using_placeholder {
                dim
            } else {
                Style::default()
            };
            spans.push(Span::styled(before, base));
            spans.push(Span::styled(cur, base.add_modifier(Modifier::REVERSED)));
            spans.push(Span::styled(after, base));
            Line::from(spans)
        }
    }
}

/// Render a [`LineEdit`] as spans with the cursor char in reversed
/// video — shared by the palette filter and the type-target prompt.
fn line_edit_spans<'a>(edit: &'a LineEdit, base: Style) -> Vec<Span<'a>> {
    let chars: Vec<char> = edit.text.chars().collect();
    let cursor = edit.cursor.min(chars.len());
    let (before, cur, after) = if cursor < chars.len() {
        (
            chars[..cursor].iter().collect::<String>(),
            chars[cursor].to_string(),
            chars[cursor + 1..].iter().collect::<String>(),
        )
    } else {
        (edit.text.clone(), " ".to_string(), String::new())
    };
    vec![
        Span::styled(before, base),
        Span::styled(cur, base.add_modifier(Modifier::REVERSED)),
        Span::styled(after, base),
    ]
}

fn row_style(s: RowStyle) -> Style {
    match s {
        RowStyle::Normal => Style::default(),
        RowStyle::Header => Style::default().add_modifier(Modifier::BOLD),
        RowStyle::Dim => Style::default().fg(Color::DarkGray),
        RowStyle::Error => Style::default().fg(Color::Red),
        RowStyle::Title => Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    }
}

fn field(sel: &Selection, key: &str) -> Option<String> {
    sel.fields
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    #![allow(clippy::unwrap_used)]
    //! Headless UI tests: drive [`App`] through synthetic worker
    //! events, keys and mouse clicks, and render every state onto a
    //! ratatui `TestBackend`. No TTY, no real drivers — the worker
    //! channels are plain mpsc pairs.

    use super::*;
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use pavo_core::host::{ResolvedView, RouteEntry, RouteKind, ViewEntry};
    use pavo_core::merge::SlotEntry;
    use pavo_protocol::manifest::Contribution;
    use pavo_protocol::manifest::ViewSpec;
    use pavo_protocol::wire::ViewResult;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use serde_json::{Map, Value};
    use std::sync::mpsc::channel;

    /// An app plus the command receiver (what the app asks the
    /// worker to do) and the event sender (what tests feed it).
    fn make_app() -> (
        App,
        std::sync::mpsc::Receiver<Cmd>,
        std::sync::mpsc::Sender<Ev>,
    ) {
        let (tx, rx_cmd) = channel::<Cmd>();
        let (ev_tx, ev_rx) = channel::<Ev>();
        (App::new(Worker { tx, rx: ev_rx }, None), rx_cmd, ev_tx)
    }

    fn kv_root() -> Value {
        serde_json::json!({
            "kind": "kv",
            "id": "root",
            "title": "Root",
            "pairs": [{"key": "status", "value": "ok"}],
        })
    }

    fn view_spec(id: &str) -> ViewSpec {
        ViewSpec {
            id: id.to_string(),
            title: format!("View {id}"),
            route: format!("/demo/{id}"),
            description: None,
            queries: Vec::new(),
            root: serde_json::from_value(kv_root()).unwrap(),
            item_route: None,
            filter_param: None,
            refresh_ms: None,
        }
    }

    fn view_entry(id: &str) -> ViewEntry {
        ViewEntry {
            driver: "demo".to_string(),
            driver_title: "Demo".to_string(),
            id: id.to_string(),
            namespaced: format!("demo:{id}"),
            route: format!("/demo/{id}"),
            title: format!("View {id}"),
            description: None,
            queries: Vec::new(),
        }
    }

    fn resolved(id: &str) -> ResolvedView {
        ResolvedView {
            driver: "demo".to_string(),
            view: id.to_string(),
            namespaced: format!("demo:{id}"),
            result: ViewResult {
                view: view_spec(id),
                data: Map::new(),
            },
        }
    }

    fn boot_ev() -> Ev {
        Ev::Boot {
            drivers: vec![DriverRow {
                kind: "demo".to_string(),
                title: "Demo".to_string(),
                views: 2,
            }],
            views: vec![view_entry("board"), view_entry("item")],
            chrome: Chrome {
                navbar: vec![SlotEntry {
                    driver: "demo".to_string(),
                    driver_title: "Demo".to_string(),
                    contribution: Contribution {
                        id: "demo:nav-board".to_string(),
                        title: "Board".to_string(),
                        route: "/demo/board".to_string(),
                        priority: 1,
                        icon: None,
                    },
                }],
                dashboard: Vec::new(),
                statusbar: Vec::new(),
                routes: Vec::new(),
                sidebar: Vec::new(),
                header_actions: Vec::new(),
                notifications: Vec::new(),
                jobs: Vec::new(),
                extra: Default::default(),
            },
            routes: vec![RouteEntry {
                route: "/demo/board".to_string(),
                driver: "demo".to_string(),
                title: "Board".to_string(),
                target: "demo:board".to_string(),
                kind: RouteKind::View,
            }],
            manifests: BTreeMap::from([("demo".to_string(), demo_manifest())]),
        }
    }

    /// A minimal `demo` manifest — carries one manifest action so
    /// `resolve_command`-driven tests have something to find.
    fn demo_manifest() -> DriverManifest {
        DriverManifest {
            name: "demo".to_string(),
            title: "Demo".to_string(),
            icon: None,
            version: "0.0.0".to_string(),
            verbs: vec!["serve".to_string()],
            capabilities: vec!["pavo".to_string()],
            deps: None,
            contributes: pavo_protocol::Contributes::default(),
            views: Vec::new(),
            renderers: Vec::new(),
            actions: vec![pavo_protocol::Action {
                id: "manifest.act".to_string(),
                label: "Manifest act".to_string(),
                icon: None,
                danger: DangerLevel::None,
                keys: vec![],
                params: Vec::new(),
                target: None,
                description: None,
                driver: None,
                route: None,
            }],
        }
    }

    fn draw(app: &mut App, w: u16, h: u16) -> TestBackend {
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
        terminal.backend().clone()
    }

    fn click(col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn scroll(kind: MouseEventKind) -> MouseEvent {
        MouseEvent {
            kind,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn boot_empty_draws_hint_and_status() {
        let (mut app, _rx, _ev) = make_app();
        app.on_event(Ev::Boot {
            drivers: vec![],
            views: vec![],
            chrome: Chrome::default(),
            routes: vec![],
            manifests: BTreeMap::new(),
        });
        // step() drains worker events then polls crossterm — on a
        // non-TTY poll errors → returns true immediately.
        assert!(app.step());
        let backend = draw(&mut app, 100, 30);
        let buf = backend.buffer().clone();
        // Status bar shows the "no drivers" message.
        let text: String = (0..100)
            .map(|x| buf[(x, 29u16)].symbol().to_string())
            .collect();
        assert!(text.contains("no pavo drivers"), "{text}");
    }

    #[test]
    fn boot_with_drivers_resolves_default_and_draws() {
        let (mut app, rx_cmd, ev) = make_app();
        ev.send(boot_ev()).unwrap();
        app.on_event(Ev::Boot {
            drivers: vec![DriverRow {
                kind: "demo".to_string(),
                title: "Demo".to_string(),
                views: 2,
            }],
            views: vec![view_entry("board"), view_entry("item")],
            chrome: Chrome::default(),
            routes: vec![],
            manifests: BTreeMap::new(),
        });
        // No navbar chrome → per-driver fallback tabs; boot resolves
        // the first view of the first driver.
        assert!(matches!(
            rx_cmd.try_recv(),
            Ok(Cmd::Resolve { target, .. }) if target == "demo:board"
        ));
        let _ = ev;
        // Simulate the view arriving.
        app.on_event(Ev::View {
            target: "demo:board".to_string(),
            result: Ok(Box::new(resolved("board"))),
            renders: HashMap::new(),
            actions: Vec::new(),
            menus: Vec::new(),
        });
        draw(&mut app, 120, 40);
    }

    #[test]
    fn navbar_tab_click_resolves_route_target() {
        let (mut app, rx_cmd, _ev) = make_app();
        app.on_event(boot_ev());
        draw(&mut app, 120, 40);
        // Tab bar is row 0; first tab starts at x=1, " dashboard " is
        // 11 wide, then " Board " starts around x=12.
        let hit = app.tab_hits.iter().find(|(_, _, i)| *i == 1).copied();
        let Some((x0, _, _)) = hit else {
            panic!("expected a second tab");
        };
        app.on_mouse(click(x0 + 1, 0));
        assert!(matches!(
            rx_cmd.try_recv(),
            Ok(Cmd::Resolve { target, .. }) if target == "demo:board"
        ));
    }

    #[test]
    fn navbar_slot_without_route_shows_note_tab() {
        let (mut app, _rx, _ev) = make_app();
        let mut ev = boot_ev();
        if let Ev::Boot { routes, .. } = &mut ev {
            routes.clear(); // driver-owned route — not resolvable
        }
        app.on_event(ev);
        draw(&mut app, 120, 40);
        // Clicking the note tab sets Canvas::Note.
        let hit = app.tab_hits.iter().find(|(_, _, i)| *i == 1).copied();
        let Some((x0, _, _)) = hit else {
            panic!("expected note tab");
        };
        app.on_mouse(click(x0 + 1, 0));
        assert!(matches!(app.current, Some(Canvas::Note { .. })));
        draw(&mut app, 120, 40);
    }

    #[test]
    fn rail_two_click_opens_driver_and_keys_navigate() {
        let (mut app, rx_cmd, _ev) = make_app();
        app.on_event(boot_ev());
        draw(&mut app, 120, 40);
        // Drain the boot-time Resolve.
        while rx_cmd.try_recv().is_ok() {}
        // Rail occupies left ~24 cols; first driver row is a couple
        // rows below the border.
        let rail = app.rects.rail;
        let row = rail.y + 1; // first driver row (inside border)
                              // First click arms, second opens.
        app.on_mouse(click(rail.x + 2, row));
        assert!(rx_cmd.try_recv().is_err());
        app.on_mouse(click(rail.x + 2, row));
        assert!(matches!(
            rx_cmd.try_recv(),
            Ok(Cmd::Resolve { target, .. }) if target == "demo:board"
        ));
        // Keys: move around, open help, dump, quit-path.
        assert!(app.on_key(KeyCode::Char('?')));
        assert!(app.help);
        assert!(app.on_key(KeyCode::Esc)); // dismisses help
        assert!(!app.help);
        app.on_key(KeyCode::Tab);
        app.on_key(KeyCode::BackTab);
        app.on_key(KeyCode::Right);
        app.on_key(KeyCode::Left);
        app.on_key(KeyCode::Char('j'));
        app.on_key(KeyCode::Char('k'));
        app.on_key(KeyCode::Down);
        app.on_key(KeyCode::Up);
        app.on_key(KeyCode::PageDown);
        app.on_key(KeyCode::PageUp);
        app.on_key(KeyCode::Char('h'));
        app.on_key(KeyCode::Char('l'));
        app.on_key(KeyCode::Char('d')); // no view → status only
        app.on_key(KeyCode::Char('g'));
        app.on_key(KeyCode::Char('r'));
        app.on_key(KeyCode::Enter);
        draw(&mut app, 120, 40);
        // q quits from rail/canvas (not detail).
        assert!(!app.on_key(KeyCode::Char('q')));
    }

    #[test]
    fn view_loaded_keys_cover_detail_dump_refresh() {
        let (mut app, _rx, _ev) = make_app();
        app.on_event(boot_ev());
        app.on_event(Ev::View {
            target: "demo:board".to_string(),
            result: Ok(Box::new(resolved("board"))),
            renders: HashMap::new(),
            actions: Vec::new(),
            menus: Vec::new(),
        });
        assert!(matches!(app.current, Some(Canvas::View(_))));
        assert_eq!(app.pane, Pane::Canvas);
        draw(&mut app, 120, 40);
        // Enter on the kv selection opens the detail pane.
        app.on_key(KeyCode::Enter);
        assert_eq!(app.pane, Pane::Detail);
        draw(&mut app, 120, 40);
        // Detail-pane navigation + q backs out to canvas.
        app.on_key(KeyCode::Char('j'));
        app.on_key(KeyCode::Char('k'));
        app.on_key(KeyCode::PageDown);
        app.on_key(KeyCode::PageUp);
        app.on_key(KeyCode::Enter); // detail Enter → canvas
        assert_eq!(app.pane, Pane::Canvas);
        app.on_key(KeyCode::Tab);
        assert_eq!(app.pane, Pane::Detail);
        assert!(app.on_key(KeyCode::Char('q'))); // exits detail → canvas
        assert_eq!(app.pane, Pane::Canvas);
        // Now on a real view: d dumps JSON, g reports no graph, r
        // re-resolves with refresh.
        app.on_key(KeyCode::Char('d'));
        app.on_key(KeyCode::Char('g'));
        app.on_key(KeyCode::Char('r'));
        draw(&mut app, 120, 40);
        assert!(!app.on_key(KeyCode::Char('q')));
    }

    #[test]
    fn resolve_error_and_reload_paths() {
        let (mut app, rx_cmd, _ev) = make_app();
        app.on_event(boot_ev());
        // Failed resolve → load_error note.
        app.on_event(Ev::View {
            target: "demo:missing".to_string(),
            result: Err("driver exploded".to_string()),
            renders: HashMap::new(),
            actions: Vec::new(),
            menus: Vec::new(),
        });
        assert!(app.load_error.is_some());
        draw(&mut app, 120, 40);
        // Reloaded event rebuilds tabs.
        app.on_event(Ev::Reloaded {
            views: vec![view_entry("board")],
            chrome: Chrome::default(),
            routes: vec![],
            manifests: BTreeMap::new(),
        });
        // Refresh on dashboard sends Reload.
        app.on_event(Ev::View {
            target: "dashboard".to_string(),
            result: Ok(Box::new(resolved("board"))),
            renders: HashMap::new(),
            actions: Vec::new(),
            menus: Vec::new(),
        });
        app.resolve("dashboard");
        assert!(matches!(app.current, Some(Canvas::Dashboard)));
        draw(&mut app, 120, 40);
        // Drain earlier commands, then refresh on dashboard → Reload.
        while rx_cmd.try_recv().is_ok() {}
        app.refresh();
        assert!(matches!(rx_cmd.try_recv(), Ok(Cmd::Reload)));
        // Refresh with no current → status only.
        let (mut app2, _rx2, _ev2) = make_app();
        app2.refresh();
        draw(&mut app2, 80, 24);
    }

    #[test]
    fn mouse_scroll_and_canvas_clicks() {
        let (mut app, _rx, _ev) = make_app();
        app.on_event(boot_ev());
        app.on_event(Ev::View {
            target: "demo:board".to_string(),
            result: Ok(Box::new(resolved("board"))),
            renders: HashMap::new(),
            actions: Vec::new(),
            menus: Vec::new(),
        });
        draw(&mut app, 120, 40);
        // Scroll events adjust offsets.
        app.on_mouse(scroll(MouseEventKind::ScrollDown));
        app.on_mouse(scroll(MouseEventKind::ScrollUp));
        app.on_mouse(scroll(MouseEventKind::ScrollRight));
        app.on_mouse(scroll(MouseEventKind::ScrollLeft));
        // Click inside canvas on a selectable row, then again to
        // open detail.
        let canvas = app.rects.canvas;
        let row = canvas.y + 2;
        app.on_mouse(click(canvas.x + 2, row));
        app.on_mouse(click(canvas.x + 2, row));
        // Click outside any rect is a no-op.
        app.on_mouse(click(0, 0));
        app.on_mouse(click(u16::MAX, u16::MAX));
        draw(&mut app, 120, 40);
    }

    /// A resolved view carrying an interactive form (`demo:new`
    /// shape): text + select editable, textarea read-only.
    fn resolved_form() -> ResolvedView {
        let mut spec = view_spec("new");
        spec.root = serde_json::from_value(serde_json::json!({
            "kind": "column",
            "title": "New item",
            "children": [
                {"kind": "form", "id": "add-item", "title": "Add item",
                 "submit_op": "add", "submit_label": "Add",
                 "fields": [
                    {"key": "title", "label": "Title", "kind": "text",
                     "required": true, "placeholder": "what needs doing?"},
                    {"key": "status", "label": "Status", "kind": "select",
                     "options": ["open", "wip", "done"], "default": "open"},
                    {"key": "note", "label": "Note", "kind": "textarea",
                     "editable": false, "value": "read-only"}
                 ]},
                {"kind": "markdown", "text": "try it"}
            ]
        }))
        .unwrap();
        ResolvedView {
            driver: "demo".to_string(),
            view: "new".to_string(),
            namespaced: "demo:new".to_string(),
            result: ViewResult {
                view: spec,
                data: Map::new(),
            },
        }
    }

    /// Position `sel_pos` on the canvas row carrying `mark`.
    fn select_marked_row(app: &mut App, field: Option<usize>) {
        let m = app.model_for_input().unwrap();
        let mark = m
            .form_marks
            .iter()
            .copied()
            .find(|mk| mk.field == field)
            .expect("form mark");
        app.sel_pos = m
            .selectables
            .iter()
            .position(|&i| i == mark.row)
            .expect("selectable row");
    }

    #[test]
    fn form_session_edits_and_submits() {
        let (mut app, rx_cmd, _ev) = make_app();
        app.on_event(boot_ev());
        app.on_event(Ev::View {
            target: "demo:new".to_string(),
            result: Ok(Box::new(resolved_form())),
            renders: HashMap::new(),
            actions: Vec::new(),
            menus: Vec::new(),
        });
        while rx_cmd.try_recv().is_ok() {} // drain boot Resolve
                                           // Enter on the 'title' field row opens the editor.
        select_marked_row(&mut app, Some(0));
        app.on_key(KeyCode::Enter);
        let s = app.form_session.as_ref().expect("session open");
        assert_eq!(s.spec.id.as_deref(), Some("add-item"));
        assert_eq!(s.target, "demo:new");
        draw(&mut app, 120, 40);
        // Type into the focused text field; j is a literal char.
        for c in "ship".chars() {
            app.on_key(KeyCode::Char(c));
        }
        app.on_key(KeyCode::Char('j'));
        assert_eq!(
            app.form_session.as_ref().unwrap().fields[0].buf.text,
            "shipj"
        );
        // Down to 'status' (select), Right cycles open → wip.
        app.on_key(KeyCode::Down);
        app.on_key(KeyCode::Right);
        assert_eq!(app.form_session.as_ref().unwrap().fields[1].buf.text, "wip");
        // Down wraps? order has 2 editable fields — Down wraps to title.
        app.on_key(KeyCode::Down);
        assert_eq!(app.form_session.as_ref().unwrap().focused(), Some(0));
        // Back to status and submit.
        app.on_key(KeyCode::Down);
        app.on_key(KeyCode::Enter);
        let cmd = rx_cmd.try_recv().expect("submit cmd");
        match cmd {
            Cmd::Submit {
                target,
                form,
                values,
            } => {
                assert_eq!(target, "demo:new");
                assert_eq!(form.as_deref(), Some("add-item"));
                assert_eq!(values["title"], serde_json::json!("shipj"));
                assert_eq!(values["status"], serde_json::json!("wip"));
                assert!(values.get("note").is_none(), "read-only excluded");
            }
            other => panic!("expected Submit, got {other:?}"),
        }
        assert!(app.form_session.as_ref().unwrap().busy);
        // Driver rejects with per-field errors → session stays open.
        let mut errors = Map::new();
        errors.insert("title".to_string(), serde_json::json!("too short"));
        app.on_event(Ev::Submit {
            result: Ok(pavo_protocol::wire::SubmitResult {
                message: Some("rejected".to_string()),
                errors,
                ..Default::default()
            }),
        });
        let s = app.form_session.as_ref().unwrap();
        assert!(!s.busy);
        assert_eq!(s.errors.get("title").map(String::as_str), Some("too short"));
        draw(&mut app, 120, 40);
        // Second submit succeeds with a fresh view → session closes,
        // canvas swaps.
        app.on_key(KeyCode::Enter);
        while rx_cmd.try_recv().is_ok() {}
        app.on_event(Ev::Submit {
            result: Ok(pavo_protocol::wire::SubmitResult {
                message: Some("added".to_string()),
                view: Some(Box::new(ViewResult {
                    view: view_spec("new"),
                    data: Map::new(),
                })),
                ..Default::default()
            }),
        });
        assert!(app.form_session.is_none());
        assert!(matches!(app.current, Some(Canvas::View(_))));
        assert_eq!(app.status, "added");
        draw(&mut app, 120, 40);
    }

    #[test]
    fn form_required_check_and_esc() {
        let (mut app, rx_cmd, _ev) = make_app();
        app.on_event(boot_ev());
        app.on_event(Ev::View {
            target: "demo:new".to_string(),
            result: Ok(Box::new(resolved_form())),
            renders: HashMap::new(),
            actions: Vec::new(),
            menus: Vec::new(),
        });
        while rx_cmd.try_recv().is_ok() {}
        // Submit row (field: None) opens the session at first field.
        select_marked_row(&mut app, None);
        app.on_key(KeyCode::Enter);
        assert!(app.form_session.is_some());
        // 'title' is required and empty → local rejection, no Cmd.
        app.on_key(KeyCode::Enter);
        assert!(rx_cmd.try_recv().is_err());
        assert!(app
            .form_session
            .as_ref()
            .unwrap()
            .errors
            .contains_key("title"));
        // Esc closes without submitting.
        app.on_key(KeyCode::Esc);
        assert!(app.form_session.is_none());
        draw(&mut app, 120, 40);
    }

    /// A manifest-scope [`UiAction`] for invoke-flow tests.
    fn ui_action(id: &str, danger: DangerLevel, keys: &[&str]) -> UiAction {
        UiAction {
            action: pavo_protocol::Action {
                id: id.to_string(),
                label: format!("{id} label"),
                icon: None,
                danger,
                keys: keys.iter().map(|s| s.to_string()).collect(),
                params: Vec::new(),
                target: None,
                description: None,
                driver: None,
                route: None,
            },
            scope: pavo_core::ActionScope::Manifest,
            target: "demo:board".to_string(),
            view: Some("board".to_string()),
            item_id: None,
        }
    }

    fn view_with_actions(actions: Vec<UiAction>) -> Ev {
        Ev::View {
            target: "demo:board".to_string(),
            result: Ok(Box::new(resolved("board"))),
            renders: HashMap::new(),
            actions,
            menus: Vec::new(),
        }
    }

    #[test]
    fn action_key_invokes_and_chord_works() {
        let (mut app, rx_cmd, _ev) = make_app();
        app.on_event(boot_ev());
        while rx_cmd.try_recv().is_ok() {}
        app.on_event(view_with_actions(vec![
            ui_action("refresh", DangerLevel::None, &["R"]),
            ui_action("archive", DangerLevel::None, &["gz"]),
        ]));
        // Single-char spec 'R' fires immediately → Cmd::Invoke.
        app.on_key(KeyCode::Char('R'));
        match rx_cmd.try_recv() {
            Ok(Cmd::Invoke {
                target,
                action,
                view,
                action_target,
                params,
            }) => {
                assert_eq!(target, "demo:board");
                assert_eq!(action, "refresh");
                assert_eq!(view.as_deref(), Some("board"));
                assert!(action_target.is_none());
                assert!(params.is_empty());
            }
            other => panic!("expected Invoke, got {other:?}"),
        }
        // Chord "gz": first 'g' is a builtin (dump graph) AND a
        // chord prefix — it must wait, not dump.
        app.on_key(KeyCode::Char('g'));
        assert!(rx_cmd.try_recv().is_err(), "chord prefix consumed");
        app.on_key(KeyCode::Char('z'));
        match rx_cmd.try_recv() {
            Ok(Cmd::Invoke { action, .. }) => assert_eq!(action, "archive"),
            other => panic!("expected Invoke, got {other:?}"),
        }
        // Invoke result with a fresh view swaps the canvas.
        app.on_event(Ev::Invoke {
            result: Ok(pavo_protocol::wire::ActionResult {
                message: Some("done".to_string()),
                ..Default::default()
            }),
            actions: Vec::new(),
            menus: Vec::new(),
        });
        assert_eq!(app.status, "done");
        // Result without a view refetches via Resolve(refresh).
        app.on_event(Ev::Invoke {
            result: Ok(pavo_protocol::wire::ActionResult::default()),
            actions: Vec::new(),
            menus: Vec::new(),
        });
        assert!(matches!(
            rx_cmd.try_recv(),
            Ok(Cmd::Resolve { refresh: true, .. })
        ));
        draw(&mut app, 120, 40);
    }

    #[test]
    fn confirm_modal_gates_invoke() {
        let (mut app, rx_cmd, _ev) = make_app();
        app.on_event(boot_ev());
        while rx_cmd.try_recv().is_ok() {}
        app.on_event(view_with_actions(vec![ui_action(
            "delete",
            DangerLevel::Confirm,
            &["D"],
        )]));
        app.on_key(KeyCode::Char('D'));
        // No Cmd yet — the modal is open.
        assert!(rx_cmd.try_recv().is_err());
        assert!(app.confirm.is_some());
        draw(&mut app, 120, 40);
        // 'n' cancels.
        app.on_key(KeyCode::Char('n'));
        assert!(app.confirm.is_none());
        assert!(rx_cmd.try_recv().is_err());
        // 'D' again, then 'y' → invoke fires.
        app.on_key(KeyCode::Char('D'));
        app.on_key(KeyCode::Char('y'));
        assert!(matches!(rx_cmd.try_recv(), Ok(Cmd::Invoke { .. })));
    }

    #[test]
    fn palette_filters_and_invokes_action() {
        let (mut app, rx_cmd, _ev) = make_app();
        app.on_event(boot_ev());
        while rx_cmd.try_recv().is_ok() {}
        app.on_event(view_with_actions(vec![ui_action(
            "todo.done",
            DangerLevel::None,
            &[],
        )]));
        app.on_key(KeyCode::Char(':'));
        assert!(app.palette.is_some());
        draw(&mut app, 120, 40);
        // Filter to the action, Enter activates → invoke.
        for c in "done".chars() {
            app.on_key(KeyCode::Char(c));
        }
        app.on_key(KeyCode::Enter);
        assert!(app.palette.is_none());
        assert!(matches!(
            rx_cmd.try_recv(),
            Ok(Cmd::Invoke { action, .. }) if action == "todo.done"
        ));
    }

    #[test]
    fn small_terminal_still_draws() {
        let (mut app, _rx, _ev) = make_app();
        app.on_event(boot_ev());
        app.on_event(Ev::View {
            target: "demo:board".to_string(),
            result: Ok(Box::new(resolved("board"))),
            renders: HashMap::new(),
            actions: Vec::new(),
            menus: Vec::new(),
        });
        app.help = true;
        draw(&mut app, 40, 10);
        draw(&mut app, 20, 5);
    }

    // ---------- PL-020: commands, keymap, filter, item_route ----------

    /// A resolved view whose spec declares `item_route` +
    /// `filter_param` and whose root is a tree with one item —
    /// exercises navigation + filter plumbing end to end.
    fn tree_resolved() -> ResolvedView {
        let mut spec = view_spec("board");
        spec.item_route = Some("/demo/item?id={item.id}".to_string());
        spec.filter_param = Some("q".to_string());
        spec.root = serde_json::from_value(serde_json::json!({
            "kind": "tree",
            "id": "root",
            "items": [
                {"id": "T-1", "label": "Task one", "props": {"prio": "high"}}
            ]
        }))
        .unwrap();
        ResolvedView {
            driver: "demo".to_string(),
            view: "board".to_string(),
            namespaced: "demo:board".to_string(),
            result: ViewResult {
                view: spec,
                data: Map::new(),
            },
        }
    }

    /// Point `sel_pos` at the first selectable row carrying an item
    /// payload (a tree row).
    fn select_first_item(app: &mut App) {
        let m = app.model_for_input().unwrap();
        for (pos, &row) in m.selectables.iter().enumerate() {
            if let Some(sel_id) = m.rows[row].sel {
                if m.selections[sel_id].item.is_some() {
                    app.sel_pos = pos;
                    return;
                }
            }
        }
        panic!("no item-carrying selectable row");
    }

    #[test]
    fn item_route_enter_navigates_with_item_params() {
        let (mut app, rx_cmd, _ev) = make_app();
        app.on_event(boot_ev());
        while rx_cmd.try_recv().is_ok() {}
        app.on_event(Ev::View {
            target: "demo:board".to_string(),
            result: Ok(Box::new(tree_resolved())),
            renders: HashMap::new(),
            actions: Vec::new(),
            menus: Vec::new(),
        });
        select_first_item(&mut app);
        draw(&mut app, 120, 40);
        app.on_key(KeyCode::Enter);
        match rx_cmd.try_recv() {
            Ok(Cmd::Resolve {
                target,
                refresh,
                params,
            }) => {
                assert_eq!(target, "demo:item");
                assert!(!refresh);
                assert_eq!(params["id"], serde_json::json!("T-1"));
            }
            other => panic!("expected Resolve, got {other:?}"),
        }
    }

    #[test]
    fn filter_builtin_binds_slash_and_f() {
        let (mut app, rx_cmd, _ev) = make_app();
        app.on_event(boot_ev());
        while rx_cmd.try_recv().is_ok() {}
        app.on_event(Ev::View {
            target: "demo:board".to_string(),
            result: Ok(Box::new(tree_resolved())),
            renders: HashMap::new(),
            actions: Vec::new(),
            menus: Vec::new(),
        });
        // '/' opens the overlay bound to filter_param "q".
        app.on_key(KeyCode::Char('/'));
        let s = app.filter.as_ref().expect("filter open");
        assert_eq!(s.param, "q");
        assert_eq!(s.target, "demo:board");
        draw(&mut app, 120, 40);
        for c in "high".chars() {
            app.on_key(KeyCode::Char(c));
        }
        app.on_key(KeyCode::Enter);
        assert!(app.filter.is_none());
        match rx_cmd.try_recv() {
            Ok(Cmd::Resolve {
                target,
                params,
                refresh,
            }) => {
                assert_eq!(target, "demo:board");
                assert!(!refresh);
                assert_eq!(params["q"], serde_json::json!("high"));
            }
            other => panic!("expected Resolve, got {other:?}"),
        }
        // 'F' opens it too; empty submit clears the param.
        app.on_key(KeyCode::Char('F'));
        assert!(app.filter.is_some());
        app.on_key(KeyCode::Enter);
        match rx_cmd.try_recv() {
            Ok(Cmd::Resolve { params, .. }) => assert!(params.get("q").is_none()),
            other => panic!("expected Resolve, got {other:?}"),
        }
    }

    #[test]
    fn route_command_navigates_with_query_params() {
        let (mut app, rx_cmd, _ev) = make_app();
        app.on_event(boot_ev());
        while rx_cmd.try_recv().is_ok() {}
        app.on_event(Ev::View {
            target: "demo:board".to_string(),
            result: Ok(Box::new(resolved("board"))),
            renders: HashMap::new(),
            actions: Vec::new(),
            menus: Vec::new(),
        });
        while rx_cmd.try_recv().is_ok() {}
        let cmd = Command::parse("route:/demo/item?id=T-9&tag=a%20b").unwrap();
        assert!(app.run_command(cmd));
        match rx_cmd.try_recv() {
            Ok(Cmd::Resolve {
                target,
                params,
                refresh: false,
            }) => {
                assert_eq!(target, "demo:item");
                assert_eq!(params["id"], serde_json::json!("T-9"));
                assert_eq!(params["tag"], serde_json::json!("a b"));
            }
            other => panic!("expected Resolve, got {other:?}"),
        }
        // Unresolvable route → status, no command.
        app.run_command(Command::parse("route:/nope/nowhere").unwrap());
        assert!(rx_cmd.try_recv().is_err());
        assert!(app.status.contains("unresolvable"));
    }

    #[test]
    fn menu_entry_command_resolves_builtins_and_routes() {
        let (mut app, rx_cmd, _ev) = make_app();
        app.on_event(boot_ev());
        while rx_cmd.try_recv().is_ok() {}
        let menu = |id: &str, command: &str| MenuEntry {
            id: id.to_string(),
            title: id.to_string(),
            icon: None,
            action: None,
            route: None,
            command: Some(command.to_string()),
            keys: Vec::new(),
            keywords: Vec::new(),
            section: None,
            priority: 0,
        };
        // host.* command → builtin handler.
        assert!(app.activate_palette_entry(PaletteEntry::Menu(menu("h", "host.help"))));
        assert!(app.help);
        app.help = false;
        // route: command → navigation.
        assert!(
            app.activate_palette_entry(PaletteEntry::Menu(menu("g", "route:/demo/item?id=T-4")))
        );
        assert!(matches!(
            rx_cmd.try_recv(),
            Ok(Cmd::Resolve { target, .. }) if target == "demo:item"
        ));
        // Display-only entry (no command/action/route) → status.
        let mut display = menu("d", "");
        display.command = None;
        assert!(app.activate_palette_entry(PaletteEntry::Menu(display)));
        assert!(app.status.contains("display-only"));
    }

    #[test]
    fn action_route_field_navigates_instead_of_invoking() {
        let (mut app, rx_cmd, _ev) = make_app();
        app.on_event(boot_ev());
        while rx_cmd.try_recv().is_ok() {}
        let mut a = ui_action("open", DangerLevel::None, &["O"]);
        a.action.route = Some("/demo/item?id={item.id}".to_string());
        a.item_id = Some("T-3".to_string());
        app.on_event(view_with_actions(vec![a]));
        while rx_cmd.try_recv().is_ok() {}
        app.on_key(KeyCode::Char('O'));
        // No Cmd::Invoke — the declared route navigated instead,
        // interpolated over the action's item context.
        match rx_cmd.try_recv() {
            Ok(Cmd::Resolve {
                target,
                params,
                refresh: false,
            }) => {
                assert_eq!(target, "demo:item");
                assert_eq!(params["id"], serde_json::json!("T-3"));
            }
            other => panic!("expected Resolve, got {other:?}"),
        }
        draw(&mut app, 120, 40);
    }

    #[test]
    fn action_result_route_navigates_after_invoke() {
        let (mut app, rx_cmd, _ev) = make_app();
        app.on_event(boot_ev());
        while rx_cmd.try_recv().is_ok() {}
        let mut a = ui_action("go", DangerLevel::None, &["G"]);
        a.item_id = Some("T-7".to_string());
        app.on_event(view_with_actions(vec![a]));
        while rx_cmd.try_recv().is_ok() {}
        app.on_key(KeyCode::Char('G'));
        assert!(matches!(rx_cmd.try_recv(), Ok(Cmd::Invoke { .. })));
        app.on_event(Ev::Invoke {
            result: Ok(pavo_protocol::wire::ActionResult {
                message: Some("done".to_string()),
                route: Some("/demo/item?id={target.id}".to_string()),
                ..Default::default()
            }),
            actions: Vec::new(),
            menus: Vec::new(),
        });
        match rx_cmd.try_recv() {
            Ok(Cmd::Resolve {
                target,
                params,
                refresh: false,
            }) => {
                assert_eq!(target, "demo:item");
                assert_eq!(params["id"], serde_json::json!("T-7"));
            }
            other => panic!("expected Resolve, got {other:?}"),
        }
        assert_eq!(app.status, "done");
    }

    #[test]
    fn command_layer_fallback_dispatch_and_unsupported() {
        let (mut app, rx_cmd, _ev) = make_app();
        app.on_event(boot_ev());
        while rx_cmd.try_recv().is_ok() {}
        app.on_event(Ev::View {
            target: "demo:board".to_string(),
            result: Ok(Box::new(resolved("board"))),
            renders: HashMap::new(),
            actions: Vec::new(), // nothing collected — resolve_command path
            menus: Vec::new(),
        });
        while rx_cmd.try_recv().is_ok() {}
        // action: id absent from UiActions but present in the demo
        // manifest → resolve_command → Dispatch → Cmd::Invoke.
        assert!(app.run_command(Command::parse("action:manifest.act").unwrap()));
        match rx_cmd.try_recv() {
            Ok(Cmd::Invoke {
                target,
                action,
                view,
                ..
            }) => {
                assert_eq!(target, "demo:board");
                assert_eq!(action, "manifest.act");
                assert_eq!(view.as_deref(), Some("board"));
            }
            other => panic!("expected Invoke, got {other:?}"),
        }
        // Unknown driver command → Unsupported.
        assert!(app.run_command(Command::parse("driver:ghost:nope").unwrap()));
        assert_eq!(app.status, "unsupported command");
        // host.quit propagates false.
        assert!(!app.run_command(Command::parse("host.quit").unwrap()));
    }

    #[test]
    fn user_keymap_overrides_builtin_binding() {
        let (tx, rx_cmd) = channel::<Cmd>();
        let (ev_tx, ev_rx) = channel::<Ev>();
        let keymap = crate::keymap::parse(r#""host.filter" = "ctrl+f""#);
        let mut app = App::with_keymap(Worker { tx, rx: ev_rx }, None, keymap);
        app.on_event(boot_ev());
        while rx_cmd.try_recv().is_ok() {}
        app.on_event(Ev::View {
            target: "demo:board".to_string(),
            result: Ok(Box::new(tree_resolved())),
            renders: HashMap::new(),
            actions: Vec::new(),
            menus: Vec::new(),
        });
        while rx_cmd.try_recv().is_ok() {}
        // ctrl+f (the override) opens the filter; '/' no longer does.
        app.on_key_event(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL));
        assert!(app.filter.is_some());
        app.filter = None;
        app.on_key(KeyCode::Char('/'));
        assert!(app.filter.is_none());
        let _ = ev_tx;
    }

    #[test]
    fn user_keymap_scoped_action_binding() {
        let (tx, rx_cmd) = channel::<Cmd>();
        let (_ev_tx, ev_rx) = channel::<Ev>();
        let keymap = crate::keymap::parse(
            r#"
            [demo]
            "action:refresh" = "ctrl+r"
            "#,
        );
        let mut app = App::with_keymap(Worker { tx, rx: ev_rx }, None, keymap);
        app.on_event(boot_ev());
        while rx_cmd.try_recv().is_ok() {}
        app.on_event(Ev::View {
            target: "demo:board".to_string(),
            result: Ok(Box::new(resolved("board"))),
            renders: HashMap::new(),
            actions: vec![ui_action("refresh", DangerLevel::None, &[])],
            menus: Vec::new(),
        });
        while rx_cmd.try_recv().is_ok() {}
        // The scoped override binds ctrl+r → the action invoke
        // ('r' alone stays the builtin refresh).
        app.on_key_event(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert!(matches!(
            rx_cmd.try_recv(),
            Ok(Cmd::Invoke { action, .. }) if action == "refresh"
        ));
    }

    /// `ViewSpec.refresh_ms` arms the auto-refresh schedule; the
    /// deadline firing sends a quiet `Cmd::Resolve { refresh: true }`
    /// and slides forward, overlays pause the tick, and out-of-range
    /// hints never arm it.
    #[test]
    fn auto_refresh_schedule() {
        let (mut app, rx_cmd, _ev) = make_app();
        app.on_event(boot_ev());
        while rx_cmd.try_recv().is_ok() {}

        // No hint → no schedule.
        app.on_event(view_with_actions(Vec::new()));
        while rx_cmd.try_recv().is_ok() {}
        assert!(app.auto_refresh.is_none());

        // In-range hint arms it.
        let mut rv = resolved("board");
        rv.result.view.refresh_ms = Some(2000);
        app.on_event(Ev::View {
            target: "demo:board".to_string(),
            result: Ok(Box::new(rv)),
            renders: HashMap::new(),
            actions: Vec::new(),
            menus: Vec::new(),
        });
        while rx_cmd.try_recv().is_ok() {}
        let (interval, _) = app.auto_refresh.expect("schedule armed");
        assert_eq!(interval, Duration::from_secs(2));

        // Deadline in the future → nothing sent.
        app.maybe_auto_refresh();
        assert!(rx_cmd.try_recv().is_err());

        // Past deadline → quiet refresh resolve, deadline slides.
        app.auto_refresh = app
            .auto_refresh
            .map(|(d, _)| (d, Instant::now() - Duration::from_secs(1)));
        app.maybe_auto_refresh();
        assert!(matches!(
            rx_cmd.try_recv(),
            Ok(Cmd::Resolve { refresh: true, .. })
        ));
        assert!(app
            .auto_refresh
            .is_some_and(|(_, deadline)| deadline > Instant::now()));

        // Overlay open → the tick slides but nothing is sent.
        app.help = true;
        app.auto_refresh = app
            .auto_refresh
            .map(|(d, _)| (d, Instant::now() - Duration::from_secs(1)));
        app.maybe_auto_refresh();
        assert!(rx_cmd.try_recv().is_err());
        app.help = false;

        // Out-of-range hints are ignored entirely.
        for bad in [Some(500), Some(600_000)] {
            let mut rv = resolved("board");
            rv.result.view.refresh_ms = bad;
            app.on_event(Ev::View {
                target: "demo:board".to_string(),
                result: Ok(Box::new(rv)),
                renders: HashMap::new(),
                actions: Vec::new(),
                menus: Vec::new(),
            });
            while rx_cmd.try_recv().is_ok() {}
            assert!(app.auto_refresh.is_none(), "hint {bad:?} armed");
        }

        // Navigating away disarms; the next view re-arms on land.
        app.resolve("dashboard");
        assert!(app.auto_refresh.is_none());
    }
}
