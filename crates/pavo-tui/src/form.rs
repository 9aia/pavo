//! Interactive-form editing state for the TUI.
//!
//! A [`FormSession`] opens when the user presses Enter on a form
//! field (or the form's submit row) in the canvas — the model marks
//! those rows via [`crate::model::FormMark`]. While a session is
//! open the app routes keys here instead of the normal bindings:
//! arrows/Tab move between editable fields, printable chars edit the
//! focused field's [`LineEdit`] buffer, `Enter` submits through the
//! worker (`submit` wire op), `Esc` discards.
//!
//! Field kinds: `text`/`textarea`/`number` (and unknown kinds) edit
//! as a single-line buffer; `select` cycles `options`; `bool`
//! toggles. `editable:false` fields render read-only on the canvas
//! and never enter a session's submitted `values`.

use std::collections::HashMap;

use crossterm::event::KeyCode;
use pavo_core::Params;
use serde_json::{Map, Value};

use crate::model::{FormFieldSpec, FormSpec};

/// A minimal single-line editor: text plus a char-index cursor.
/// Fields are short — O(n) char-vector edits are fine.
#[derive(Clone, Debug, Default)]
pub struct LineEdit {
    /// Buffer contents.
    pub text: String,
    /// Cursor position as a *char* index into `text` (0..=len).
    pub cursor: usize,
}

impl LineEdit {
    /// Seed from a JSON value (`value`/`default`): strings verbatim,
    /// numbers/bools via Display, anything else compact JSON.
    #[must_use]
    pub fn from_json(v: Option<&Value>) -> Self {
        let text = match v {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Null) | None => String::new(),
            Some(other) => match other {
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                _ => serde_json::to_string(other).unwrap_or_default(),
            },
        };
        let cursor = text.chars().count();
        Self { text, cursor }
    }

    fn chars(&self) -> Vec<char> {
        self.text.chars().collect()
    }

    /// Insert `c` at the cursor.
    pub fn insert(&mut self, c: char) {
        let mut cs = self.chars();
        let at = self.cursor.min(cs.len());
        cs.insert(at, c);
        self.text = cs.into_iter().collect();
        self.cursor = at + 1;
    }

    /// Delete the char before the cursor.
    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let mut cs = self.chars();
        cs.remove(self.cursor - 1);
        self.cursor -= 1;
        self.text = cs.into_iter().collect();
    }

    /// Delete the char at the cursor.
    pub fn delete(&mut self) {
        let mut cs = self.chars();
        if self.cursor < cs.len() {
            cs.remove(self.cursor);
            self.text = cs.into_iter().collect();
        }
    }

    /// Move the cursor `delta` chars (negative = left), clamped.
    pub fn move_cursor(&mut self, delta: isize) {
        let len = self.text.chars().count() as isize;
        self.cursor = (self.cursor as isize + delta).clamp(0, len) as usize;
    }

    /// Cursor to start/end of buffer.
    pub fn home(&mut self) {
        self.cursor = 0;
    }

    /// Cursor to end of buffer.
    pub fn end(&mut self) {
        self.cursor = self.text.chars().count();
    }

    /// Replace the whole buffer (select/bool cycling writes through
    /// this).
    pub fn set(&mut self, text: String) {
        self.cursor = text.chars().count();
        self.text = text;
    }
}

/// One editable field's live edit state.
#[derive(Clone, Debug)]
pub struct FieldEdit {
    /// The field's declaration (cloned from the model's spec).
    pub spec: FormFieldSpec,
    /// Current buffer — text for `text`/`textarea`/`number`,
    /// `"true"`/`"false"` for `bool`, the selected option for
    /// `select`.
    pub buf: LineEdit,
}

impl FieldEdit {
    /// Whether this field takes free-text input (vs. discrete
    /// cycling for `select`/`bool`).
    #[must_use]
    pub fn is_text(&self) -> bool {
        !matches!(self.spec.kind.as_str(), "select" | "bool")
    }

    /// Cycle a `select` field through `options` (`delta` ±1, wraps).
    /// No-op for other kinds or empty options.
    pub fn cycle_select(&mut self, delta: isize) {
        if self.spec.kind != "select" || self.spec.options.is_empty() {
            return;
        }
        let cur = self.spec.options.iter().position(|o| *o == self.buf.text);
        let n = self.spec.options.len() as isize;
        let next = match cur {
            Some(i) => (i as isize + delta).rem_euclid(n) as usize,
            // No current match (empty/custom buffer) → start at an end.
            None if delta >= 0 => 0,
            None => (n - 1) as usize,
        };
        self.buf.set(self.spec.options[next].clone());
    }

    /// Toggle a `bool` field. No-op for other kinds.
    pub fn toggle_bool(&mut self) {
        if self.spec.kind != "bool" {
            return;
        }
        let on = matches!(self.buf.text.trim(), "true" | "1" | "yes" | "on");
        self.buf.set((!on).to_string());
    }

    /// `Left`/`Right` semantics: cursor move for text fields, value
    /// step for `select`, explicit set for `bool`.
    pub fn step(&mut self, dir: isize) {
        match self.spec.kind.as_str() {
            "select" => self.cycle_select(dir),
            "bool" => self.buf.set((dir > 0).to_string()),
            _ => self.buf.move_cursor(dir),
        }
    }
}

/// An open form-editing session — one per form node at a time.
#[derive(Clone, Debug)]
pub struct FormSession {
    /// The form's declaration (id/title/submit_op/submit_label).
    pub spec: FormSpec,
    /// Submit target — the namespaced `driver:view` ref.
    pub target: String,
    /// Per-field edit state, aligned with `spec.fields`.
    pub fields: Vec<FieldEdit>,
    /// Position within `order` (the editable-field indices).
    pub focus: usize,
    /// Indices into `fields` that accept input, in declaration order.
    pub order: Vec<usize>,
    /// Per-field validation errors from the last submit
    /// (`FormField::key` → message).
    pub errors: HashMap<String, String>,
    /// Status line — submit message, transport error, or a local
    /// "missing required" notice.
    pub message: Option<String>,
    /// A submit is in flight — input is ignored except `Esc`.
    pub busy: bool,
}

impl FormSession {
    /// Open a session over `spec`, targeting `driver:view`. Buffer
    /// seeds come from `value` then `default`; focus lands on the
    /// first editable field (`field` overrides when it names an
    /// editable field).
    #[must_use]
    pub fn open(spec: FormSpec, target: String, field: Option<usize>) -> Self {
        let fields = spec
            .fields
            .iter()
            .map(|f| FieldEdit {
                spec: f.clone(),
                buf: LineEdit::from_json(f.value.as_ref().or(f.default.as_ref())),
            })
            .collect::<Vec<_>>();
        let order = fields
            .iter()
            .enumerate()
            .filter(|(_, f)| f.spec.editable)
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        let mut session = Self {
            spec,
            target,
            fields,
            focus: 0,
            order,
            errors: HashMap::new(),
            message: None,
            busy: false,
        };
        session.focus = session
            .order
            .iter()
            .position(|&i| Some(i) == field)
            .unwrap_or(0);
        session
    }

    /// The focused field's index into `fields`, if any editable
    /// fields exist.
    #[must_use]
    pub fn focused(&self) -> Option<usize> {
        self.order.get(self.focus).copied()
    }

    /// The focused [`FieldEdit`].
    #[must_use]
    pub fn focused_field(&self) -> Option<&FieldEdit> {
        self.focused().map(|i| &self.fields[i])
    }

    /// Mutable access to the focused [`FieldEdit`].
    pub fn focused_field_mut(&mut self) -> Option<&mut FieldEdit> {
        let i = self.focused()?;
        self.fields.get_mut(i)
    }

    /// Move focus `delta` positions through the editable fields
    /// (wraps, like canvas selection).
    pub fn move_focus(&mut self, delta: isize) {
        let n = self.order.len();
        if n == 0 {
            return;
        }
        self.focus = (self.focus as isize + delta).rem_euclid(n as isize) as usize;
    }

    /// Jump focus to the first (`delta < 0`) or last editable field.
    pub fn focus_edge(&mut self, last: bool) {
        if !self.order.is_empty() {
            self.focus = if last { self.order.len() - 1 } else { 0 };
        }
    }

    /// Route one key into the session. Returns `true` when the key
    /// was consumed (always — the session owns the keyboard while
    /// open). `Esc` is handled by the caller (it closes the session);
    /// `Enter` submits via the returned [`SessionCmd`].
    pub fn key(&mut self, code: KeyCode) -> SessionCmd {
        match code {
            KeyCode::Esc => SessionCmd::Close,
            KeyCode::Enter => {
                if self.busy {
                    SessionCmd::None
                } else {
                    SessionCmd::Submit(self.values())
                }
            }
            // j/k move between fields everywhere — on choice fields
            // they're not printable anyway, and keeping the canvas's
            // vim-style nav inside the editor is less surprising.
            KeyCode::Char('j') if self.is_choice_focused() => {
                self.move_focus(1);
                SessionCmd::None
            }
            KeyCode::Char('k') if self.is_choice_focused() => {
                self.move_focus(-1);
                SessionCmd::None
            }
            KeyCode::Up => {
                self.move_focus(-1);
                SessionCmd::None
            }
            KeyCode::Down => {
                self.move_focus(1);
                SessionCmd::None
            }
            KeyCode::Tab => {
                self.move_focus(1);
                SessionCmd::None
            }
            KeyCode::BackTab => {
                self.move_focus(-1);
                SessionCmd::None
            }
            KeyCode::PageDown => {
                self.focus_edge(true);
                SessionCmd::None
            }
            KeyCode::PageUp => {
                self.focus_edge(false);
                SessionCmd::None
            }
            KeyCode::Left => {
                self.step_focused(-1);
                SessionCmd::None
            }
            KeyCode::Right => {
                self.step_focused(1);
                SessionCmd::None
            }
            KeyCode::Home => {
                if let Some(f) = self.focused_field_mut() {
                    if f.is_text() {
                        f.buf.home();
                    }
                }
                SessionCmd::None
            }
            KeyCode::End => {
                if let Some(f) = self.focused_field_mut() {
                    if f.is_text() {
                        f.buf.end();
                    }
                }
                SessionCmd::None
            }
            KeyCode::Backspace => {
                if let Some(f) = self.focused_field_mut() {
                    if f.is_text() {
                        f.buf.backspace();
                    }
                }
                SessionCmd::None
            }
            KeyCode::Delete => {
                if let Some(f) = self.focused_field_mut() {
                    if f.is_text() {
                        f.buf.delete();
                    }
                }
                SessionCmd::None
            }
            KeyCode::Char(c) => {
                self.edit_char(c);
                SessionCmd::None
            }
            _ => SessionCmd::None,
        }
    }

    /// Whether the focused field is a discrete choice (`select` /
    /// `bool`) — printable chars there are commands, not text.
    fn is_choice_focused(&self) -> bool {
        self.focused_field().map(|f| !f.is_text()).unwrap_or(false)
    }

    /// Left/Right on the focused field.
    fn step_focused(&mut self, dir: isize) {
        if let Some(f) = self.focused_field_mut() {
            f.step(dir);
        }
    }

    /// A printable char: inserts into text fields; on `select`/`bool`
    /// only the cycle keys act (`h`/`l`/space) — everything else is
    /// ignored so stray letters can't corrupt a choice. (`j`/`k` are
    /// intercepted earlier as focus movement.)
    fn edit_char(&mut self, c: char) {
        let Some(f) = self.focused_field_mut() else {
            return;
        };
        if f.is_text() {
            f.buf.insert(c);
            return;
        }
        match f.spec.kind.as_str() {
            "select" => match c {
                'h' => f.cycle_select(-1),
                'l' | ' ' => f.cycle_select(1),
                _ => {}
            },
            "bool" => match c {
                ' ' => f.toggle_bool(),
                'h' => f.step(-1),
                'l' => f.step(1),
                _ => {}
            },
            _ => {}
        }
    }

    /// Build the `values` map for the `submit` op: editable fields
    /// only, coerced per kind (`bool` → JSON bool, `number` → JSON
    /// number when parseable, everything else → string).
    #[must_use]
    pub fn values(&self) -> Params {
        let mut out = Map::new();
        for f in self.fields.iter().filter(|f| f.spec.editable) {
            if f.spec.key.is_empty() {
                continue;
            }
            let raw = f.buf.text.trim();
            let value = match f.spec.kind.as_str() {
                "bool" => Value::Bool(matches!(raw, "true" | "1" | "yes" | "on")),
                "number" => number_value(raw),
                _ => Value::String(f.buf.text.clone()),
            };
            out.insert(f.spec.key.clone(), value);
        }
        out
    }

    /// Client-side required check — returns per-field errors for
    /// empty required buffers (the driver would reject anyway, but
    /// this saves a round-trip and keeps the message local).
    #[must_use]
    pub fn required_errors(&self) -> HashMap<String, String> {
        self.fields
            .iter()
            .filter(|f| f.spec.editable && f.spec.required)
            .filter(|f| f.buf.text.trim().is_empty())
            .map(|f| (f.spec.key.clone(), "required".to_string()))
            .collect()
    }
}

/// What a session keypress asks the app to do.
#[derive(Clone, Debug)]
pub enum SessionCmd {
    /// Key consumed, nothing further.
    None,
    /// `Esc` — close the session, discard edits.
    Close,
    /// `Enter` — submit these values (the app sends `Cmd::Submit`).
    Submit(Params),
}

/// Coerce a `number` field's buffer: JSON number when it parses,
/// else the raw string (the driver validates and can reject).
fn number_value(raw: &str) -> Value {
    if raw.is_empty() {
        return Value::String(String::new());
    }
    if let Ok(n) = raw.parse::<i64>() {
        return Value::from(n);
    }
    if let Ok(n) = raw.parse::<f64>() {
        if let Some(v) = serde_json::Number::from_f64(n) {
            return Value::Number(v);
        }
    }
    Value::String(raw.to_string())
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use serde_json::json;

    fn spec() -> FormSpec {
        FormSpec {
            id: Some("f1".to_string()),
            title: "Edit".to_string(),
            submit_op: "set".to_string(),
            submit_label: "Save".to_string(),
            fields: vec![
                FormFieldSpec {
                    key: "name".to_string(),
                    label: "Name".to_string(),
                    kind: "text".to_string(),
                    required: true,
                    editable: true,
                    options: vec![],
                    default: Some(json!("anon")),
                    value: None,
                    hint: None,
                    placeholder: None,
                },
                FormFieldSpec {
                    key: "flag".to_string(),
                    label: "Flag".to_string(),
                    kind: "bool".to_string(),
                    required: false,
                    editable: true,
                    options: vec![],
                    default: Some(json!(false)),
                    value: None,
                    hint: None,
                    placeholder: None,
                },
                FormFieldSpec {
                    key: "level".to_string(),
                    label: "Level".to_string(),
                    kind: "select".to_string(),
                    required: false,
                    editable: true,
                    options: vec!["low".to_string(), "high".to_string()],
                    default: Some(json!("low")),
                    value: None,
                    hint: None,
                    placeholder: None,
                },
                FormFieldSpec {
                    key: "ro".to_string(),
                    label: "RO".to_string(),
                    kind: "text".to_string(),
                    required: false,
                    editable: false,
                    options: vec![],
                    default: None,
                    value: Some(json!("fixed")),
                    hint: None,
                    placeholder: None,
                },
            ],
        }
    }

    #[test]
    fn line_edit_insert_delete_cursor() {
        let mut e = LineEdit::default();
        for c in "abc".chars() {
            e.insert(c);
        }
        assert_eq!(e.text, "abc");
        assert_eq!(e.cursor, 3);
        e.move_cursor(-1);
        e.insert('X');
        assert_eq!(e.text, "abXc");
        e.backspace();
        assert_eq!(e.text, "abc");
        e.delete();
        assert_eq!(e.text, "ab");
        e.home();
        e.delete();
        assert_eq!(e.text, "b");
        e.end();
        e.set("zz".to_string());
        assert_eq!(e.cursor, 2);
        // cursor clamps
        e.move_cursor(99);
        assert_eq!(e.cursor, 2);
        e.move_cursor(-99);
        assert_eq!(e.cursor, 0);
    }

    #[test]
    fn session_seeds_defaults_and_skips_readonly() {
        let s = FormSession::open(spec(), "demo:edit".to_string(), None);
        assert_eq!(s.order, vec![0, 1, 2]); // 'ro' is editable:false
        assert_eq!(s.fields[0].buf.text, "anon");
        assert_eq!(s.fields[3].buf.text, "fixed");
        let v = s.values();
        assert_eq!(v["name"], json!("anon"));
        assert_eq!(v["flag"], json!(false));
        assert_eq!(v["level"], json!("low"));
        assert!(v.get("ro").is_none(), "read-only never submits");
    }

    #[test]
    fn session_keys_edit_choices_and_submit() {
        let mut s = FormSession::open(spec(), "demo:edit".to_string(), None);
        // Focus starts on 'name' (text): chars insert, j/k are text.
        s.key(KeyCode::Char('j'));
        assert_eq!(s.fields[0].buf.text, "anonj");
        // Down moves to 'flag' (bool); space toggles.
        s.key(KeyCode::Down);
        assert_eq!(s.focused(), Some(1));
        s.key(KeyCode::Char(' '));
        assert_eq!(s.fields[1].buf.text, "true");
        // Down moves focus to 'level' (select); Right/h/l cycle.
        s.key(KeyCode::Down);
        assert_eq!(s.focused(), Some(2));
        s.key(KeyCode::Right);
        assert_eq!(s.fields[2].buf.text, "high");
        s.key(KeyCode::Char('h'));
        assert_eq!(s.fields[2].buf.text, "low");
        // j on a choice field moves focus (wraps to 'name').
        s.key(KeyCode::Char('j'));
        assert_eq!(s.focused(), Some(0));
        s.key(KeyCode::BackTab);
        assert_eq!(s.focused(), Some(2));
        // Cursor keys + editing on the text field.
        s.key(KeyCode::Tab); // wraps? no — order len 3, focus pos 2 → 0
        assert_eq!(s.focused(), Some(0));
        s.key(KeyCode::Home);
        s.key(KeyCode::Delete);
        assert_eq!(s.fields[0].buf.text, "nonj");
        // Enter submits editable values only.
        match s.key(KeyCode::Enter) {
            SessionCmd::Submit(v) => {
                assert_eq!(v["name"], json!("nonj"));
                assert_eq!(v["flag"], json!(true));
                assert_eq!(v["level"], json!("low"));
                assert!(v.get("ro").is_none());
            }
            _ => panic!("expected Submit"),
        }
        assert!(matches!(s.key(KeyCode::Esc), SessionCmd::Close));
    }

    #[test]
    fn required_errors_and_number_coercion() {
        let mut s = FormSession::open(spec(), "t:v".to_string(), None);
        s.fields[0].buf.set(String::new());
        assert!(s.required_errors().contains_key("name"));
        s.fields[0].buf.set("x".to_string());
        assert!(s.required_errors().is_empty());

        // number kind coercion
        let mut sp2 = spec();
        sp2.fields.push(FormFieldSpec {
            key: "n".to_string(),
            label: "N".to_string(),
            kind: "number".to_string(),
            required: false,
            editable: true,
            options: vec![],
            default: None,
            value: None,
            hint: None,
            placeholder: None,
        });
        let mut s2 = FormSession::open(sp2, "t:v".to_string(), None);
        s2.fields[4].buf.set("42".to_string());
        assert_eq!(s2.values()["n"], json!(42));
        s2.fields[4].buf.set("4.5".to_string());
        assert_eq!(s2.values()["n"], json!(4.5));
        s2.fields[4].buf.set("abc".to_string());
        assert_eq!(s2.values()["n"], json!("abc"));
    }
}
