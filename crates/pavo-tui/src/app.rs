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

use std::time::Duration;

use crossterm::event::{Event, KeyCode, KeyEventKind, MouseButton};
use pavo_core::{Chrome, ResolvedView, RouteEntry, RouteKind, ViewEntry};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use ratatui::Frame;

use crate::model::{self, Model, RowStyle, Selection};
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
            Ev::View { target, result } => {
                self.loading = None;
                match result {
                    Ok(rv) => {
                        self.last_target = rv.namespaced.clone();
                        self.current = Some(Canvas::View(rv));
                        self.load_error = None;
                        self.sel_pos = 0;
                        self.scroll = (0, 0);
                        self.detail = None;
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
        }
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
        self.detail = Some(sel);
        self.detail_scroll = 0;
        self.pane = Pane::Detail;
    }

    fn model_for_input(&self) -> Option<Model> {
        match &self.current {
            Some(Canvas::View(rv)) => Some(model::build_view(
                &rv.result.view.root,
                &rv.result.data,
                self.canvas_w,
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
        // Help overlay captures everything except its own dismiss keys.
        if self.help {
            match code {
                KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q') => self.help = false,
                _ => {}
            }
            return true;
        }
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

    fn draw_help(&self, f: &mut Frame, area: Rect) {
        let text = Text::from(vec![
            Line::from(Span::styled(
                " keys ",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("  j/k · ↑/↓     navigate selectable rows / rail"),
            Line::from("  Tab · ←/→    switch panes (rail → canvas → detail)"),
            Line::from("  Enter        open driver view / row detail / jump"),
            Line::from("  PgUp/PgDn    page through selection"),
            Line::from("  h/l · wheel  horizontal / vertical scroll"),
            Line::from("  r            refresh current view / reload chrome"),
            Line::from("  d            dump view JSON → tmp file"),
            Line::from("  g            dump graph DOT → tmp file"),
            Line::from("  x            close detail pane"),
            Line::from("  ?            this help"),
            Line::from("  q · Esc      quit (Esc closes detail/help first)"),
        ]);
        let w = 46u16.min(area.width.saturating_sub(4));
        let h = 15u16.min(area.height.saturating_sub(2));
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
