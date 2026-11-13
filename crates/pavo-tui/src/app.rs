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

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEventKind, MouseButton};
use pavo_core::{ActionScope, Chrome, Params, ResolvedView, RouteEntry, RouteKind, ViewEntry};
use pavo_protocol::{DangerLevel, MenuEntry, RenderModel};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use ratatui::Frame;

use crate::actions::{
    action_form_spec, coerce_params, palette_entries, parse_key_spec, plan_action, ConfirmCmd,
    ConfirmSession, InvokeStep, PaletteCmd, PaletteEntry, PaletteSession, PendingInvoke,
    TypeTargetCmd, TypeTargetSession, UiAction, CHORD_TIMEOUT,
};
use crate::form::{FieldEdit, FormSession, LineEdit, SessionCmd};
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
    /// Full key spec → action for bound keys. Item-scoped actions
    /// are excluded — they carry a row target the canvas doesn't
    /// track, so they stay palette-only.
    key_chords: HashMap<String, UiAction>,
    /// Proper prefixes of multi-char specs — tells `chord_or_key`
    /// whether a lone char might continue into a chord.
    chord_prefixes: HashSet<String>,
}

impl App {
    /// Build the app around a spawned worker. `initial` is the
    /// deep-link target (`--view`, `--driver`, or positional
    /// `driver:view`) resolved after boot.
    #[must_use]
    pub fn new(worker: Worker, initial: Option<String>) -> Self {
        Self {
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
        }
    }

    /// Drain pending worker events, then handle one input event.
    /// Returns `false` when the app should quit.
    pub fn step(&mut self) -> bool {
        while let Ok(ev) = self.worker.rx.try_recv() {
            self.on_event(ev);
        }
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
            Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => self.on_key(k.code),
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
            } => {
                self.drivers = drivers;
                self.views = views;
                self.chrome = chrome;
                self.routes = routes;
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
                        self.current = Some(Canvas::View(rv));
                        self.load_error = None;
                        self.sel_pos = 0;
                        self.scroll = (0, 0);
                        self.detail = None;
                        self.form_session = None;
                        self.palette = None;
                        self.confirm = None;
                        self.type_target = None;
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
            } => {
                self.views = views;
                self.chrome = chrome;
                self.routes = routes;
                self.rebuild_tabs();
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

    /// (Re)build `key_chords`/`chord_prefixes` from the current
    /// actions' `keys[]`. Item-scoped actions are skipped — they
    /// carry a row target the canvas doesn't track, so binding them
    /// globally would invoke against the wrong item (they remain
    /// palette entries).
    fn rebuild_keybinds(&mut self) {
        self.key_chords.clear();
        self.chord_prefixes.clear();
        for ui in &self.actions {
            if ui.scope == ActionScope::Item {
                continue;
            }
            for spec in &ui.action.keys {
                let Some(seq) = parse_key_spec(spec) else {
                    continue;
                };
                if seq.is_empty() {
                    continue;
                }
                // First declaration of a spec wins.
                self.key_chords
                    .entry(seq.clone())
                    .or_insert_with(|| ui.clone());
                let chars: Vec<char> = seq.chars().collect();
                for i in 1..chars.len() {
                    self.chord_prefixes
                        .insert(chars[..i].iter().collect::<String>());
                }
            }
        }
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

    /// Activate a collected action: params open the auto-form;
    /// everything else goes straight to the danger gate.
    fn activate_action(&mut self, ui: UiAction) {
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
        let pending = PendingInvoke::new(ui, params);
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

    /// Fire `Cmd::Invoke` for a fully-gated pending action.
    fn send_invoke(&mut self, pending: PendingInvoke) {
        let _ = self.worker.tx.send(Cmd::Invoke {
            target: pending.ui.target.clone(),
            action: pending.ui.action.id.clone(),
            view: pending.ui.view.clone(),
            action_target: pending.target.clone(),
            params: pending.params.clone(),
        });
        self.status = format!("invoking {}…", pending.ui.action.label);
    }

    /// `:` — open the command palette over the current view's
    /// actions + menu entries.
    fn open_palette(&mut self) {
        let entries = palette_entries(&self.actions, &self.menus);
        if entries.is_empty() {
            self.status = "no actions or menu entries".to_string();
            return;
        }
        self.palette = Some(PaletteSession::open(entries));
        self.status = "palette — type to filter, Enter runs".to_string();
    }

    /// Activate a palette entry: actions run the normal
    /// danger/param flow; menu entries resolve their `action` (by
    /// id, `driver:` prefix tolerated — v1 stays driver-local) or
    /// `route` (chrome route → view, or a bare `driver:view` link).
    fn activate_palette_entry(&mut self, entry: PaletteEntry) {
        match entry {
            PaletteEntry::Action(ui) => self.activate_action(ui),
            PaletteEntry::Menu(m) => {
                if let Some(aid) = &m.action {
                    let bare = aid.rsplit(':').next().unwrap_or(aid);
                    match self
                        .actions
                        .iter()
                        .find(|u| u.action.id == *aid || u.action.id == bare)
                        .cloned()
                    {
                        Some(ui) => self.activate_action(ui),
                        None => self.status = format!("unknown action: {aid}"),
                    }
                } else if let Some(route) = &m.route {
                    self.activate_route(route);
                } else {
                    self.status = format!("menu entry {} has no action/route", m.id);
                }
            }
        }
    }

    /// Navigate to a menu `route`: chrome routes resolve through
    /// [`App::route_target`]; a `driver:view`-shaped route resolves
    /// directly.
    fn activate_route(&mut self, route: &str) {
        if let Some(t) = self.route_target(route) {
            self.resolve(&t);
        } else if route.contains(':') {
            self.resolve(route);
        } else {
            self.status = format!("unresolvable route: {route}");
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
                self.activate_palette_entry(*entry);
            }
            PaletteCmd::None => {}
        }
        true
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
        self.form_session = None;
        self.palette = None;
        self.confirm = None;
        self.type_target = None;
        self.pending_key = None;
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

    fn on_key(&mut self, code: KeyCode) -> bool {
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
        // danger modals, the form editor, then help.
        if self.palette.is_some() {
            return self.on_palette_key(code);
        }
        if self.type_target.is_some() {
            return self.on_type_target_key(code);
        }
        if self.confirm.is_some() {
            return self.on_confirm_key(code);
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
        match code {
            KeyCode::Char(c) => self.chord_or_key(c),
            // Non-char keys can't continue a chord — flush any
            // pending prefix, then handle normally.
            other => {
                self.flush_pending();
                self.key_now(other)
            }
        }
    }

    /// Replay a pending chord prefix as ordinary keypresses (used on
    /// timeout and when the next char can't continue the chord).
    fn flush_pending(&mut self) {
        if let Some((buf, _)) = self.pending_key.take() {
            for c in buf.chars() {
                if !self.key_now(KeyCode::Char(c)) {
                    break;
                }
            }
        }
    }

    /// Char-key dispatch with chord support: a pending prefix plus
    /// `c` may complete a multi-char action `keys[]` spec; a lone
    /// char that prefixes some spec waits for [`CHORD_TIMEOUT`]; an
    /// exact single-char spec activates its action; anything else
    /// falls through to the built-in [`App::key_now`] bindings.
    /// Returns `false` when the app should quit (propagated from
    /// `q`/`Esc` replayed out of a dead chord).
    fn chord_or_key(&mut self, c: char) -> bool {
        if let Some((buf, _)) = self.pending_key.take() {
            let combined = format!("{buf}{c}");
            if let Some(ui) = self.key_chords.get(&combined).cloned() {
                self.activate_action(ui);
                return true;
            }
            if self.chord_prefixes.contains(&combined) {
                self.pending_key = Some((combined, Instant::now()));
                return true;
            }
            // Dead end — replay the buffered prefix, then treat `c`
            // fresh (it may start a chord of its own).
            for pc in buf.chars() {
                if !self.key_now(KeyCode::Char(pc)) {
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
        // Built-in single-char bindings win over action keys — a
        // driver binding `q`/`j`/`d`/… can't shadow navigation.
        // (Chords starting with those chars still work: the prefix
        // waits, and the completed spec matches before `key_now`.)
        const BUILTIN_CHARS: &[char] = &['q', '?', 'r', 'd', 'g', 'x', 'j', 'k', 'h', 'l', ':'];
        if !BUILTIN_CHARS.contains(&c) {
            if let Some(ui) = self.key_chords.get(&s).cloned() {
                self.activate_action(ui);
                return true;
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
                self.apply_actions(Vec::new(), Vec::new());
                self.pane = Pane::Canvas;
                self.status = tab.label.clone();
            }
        }
    }

    /// `d` — dump the current view as pretty JSON to a temp file.
    fn dump_view(&mut self) {
        let Some(Canvas::View(rv)) = &self.current else {
            self.status = "nothing to dump (not a driver view)".to_string();
            return;
        };
        let value = serde_json::json!({
            "view": rv.result.view,
            "data": rv.result.data,
        });
        let path = std::env::temp_dir().join(format!(
            "pavo-view-{}.json",
            rv.namespaced.replace(':', "-")
        ));
        match serde_json::to_string_pretty(&value)
            .map_err(std::io::Error::other)
            .and_then(|s| std::fs::write(&path, s))
        {
            Ok(()) => self.status = format!("wrote {}", path.display()),
            Err(e) => self.status = format!("dump failed: {e}"),
        }
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
        let left = Paragraph::new(Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled(self.status.clone(), Style::default().fg(Color::DarkGray)),
        ]));
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
            Line::from("  :            command palette (actions + menus)"),
            Line::from("  x            close detail pane"),
            Line::from("  ?            this help"),
            Line::from("  q · Esc      quit (Esc closes detail/help first)"),
        ];
        // Bound action keys for the current view — the discoverable
        // half of the interaction protocol.
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

fn in_rect(r: Rect, col: u16, row: u16) -> bool {
    col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
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
}
