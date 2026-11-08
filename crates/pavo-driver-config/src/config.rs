//! Config resolution — the "common config API" half of
//! `tamias-driver-config`.
//!
//! The driver answers *namespaced* queries (`get myco.theme`,
//! `resolve`, `sources`) over a layered config tree; callers never
//! read the files themselves. Layers, lowest → highest precedence:
//!
//! 1. **defaults** — built-in (`theme=dark`, `statusbar=true`,
//!    `refresh_ms=5000`).
//! 2. **global** — `$XDG_CONFIG_HOME/pavo/pavo.{toml,json}` or
//!    `~/.config/pavo/pavo.{toml,json}`.
//! 3. **project** — every `pavo.toml`/`pavo.json` found walking up
//!    from the cwd (filesystem root → cwd; nearer files win per-key,
//!    nested tables merge recursively).
//! 4. **explicit** — `--config <path>` (manual `serve` only; pavo's
//!    spawned `serve` can't pass argv) or `$PAVO_CONFIG`. One file,
//!    highest file-layer precedence.
//! 5. **env** — `PAVO_CONFIG_*` overrides for scalar keys:
//!    `PAVO_CONFIG_THEME=light` → `theme`; `__` separates dotted
//!    segments (`PAVO_CONFIG_MYCO__THEME` → `myco.theme`), single
//!    `_` stays literal (`PAVO_CONFIG_REFRESH_MS` → `refresh_ms`).
//!    Values parse as JSON when possible, else strings.
//!
//! `set` writes back to the *project-level* file: the explicit path
//! when `--config`/`PAVO_CONFIG` named one, else the nearest project
//! file found, else a fresh `./pavo.toml` in the cwd. Writes preserve
//! TOML formatting/comments via `toml_edit`; JSON is rewritten
//! pretty-printed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Number, Value};
use toml_edit::{Array, DocumentMut, Item, Table, Value as TomlValue};

/// File formats the driver reads/writes. YAML is deliberately
/// unsupported (no yaml dep in the workspace tree) — `.yaml`/`.yml`
/// candidates surface as errors in `sources`, never silent parses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    /// TOML (`toml_edit` parse — comments/formatting preserved on `set`).
    Toml,
    /// JSON (serde_json).
    Json,
}

impl FileKind {
    /// Stable label for `sources` payloads.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Toml => "toml",
            Self::Json => "json",
        }
    }
}

/// Where a [`Config`] came from — one entry per contributing layer.
#[derive(Clone, Debug)]
pub struct SourceInfo {
    /// Display label — `defaults`, `global`, `project`, `--config`,
    /// `PAVO_CONFIG`, `env:PAVO_CONFIG_*`, or `error`.
    pub label: String,
    /// Filesystem path for file layers (`None` for defaults/env).
    pub path: Option<PathBuf>,
    /// Format / pseudo-kind label for the `sources` view.
    pub kind: String,
    /// Precedence rank — lower is weaker (defaults=0 … env=99).
    pub precedence: u32,
    /// Number of flattened dotted keys this layer contributed.
    pub keys: usize,
    /// Parse/IO error detail when `kind == "error"`.
    pub error: Option<String>,
}

/// A successfully loaded config file layer.
#[derive(Clone, Debug)]
struct FileLayer {
    /// Parsed top-level object.
    value: Map<String, Value>,
}

/// The resolved configuration: merged tree, flat dotted view,
/// provenance, and where `set` writes.
#[derive(Clone, Debug)]
pub struct Config {
    /// Effective config — defaults + all file layers + env, deep-merged.
    pub merged: Value,
    /// `merged` flattened to dotted keys (`myco.theme` → value).
    pub flat: BTreeMap<String, Value>,
    /// Every layer that contributed (or failed), in precedence order.
    pub sources: Vec<SourceInfo>,
    /// File `set` writes to.
    pub write_target: PathBuf,
}

/// Inputs to [`load`] — gathered once so tests don't touch the
/// process environment.
#[derive(Clone, Debug)]
pub struct Options {
    /// Explicit config file (`--config` arg or `$PAVO_CONFIG`).
    pub explicit: Option<PathBuf>,
    /// Directory the project-layer walk starts from.
    pub cwd: PathBuf,
    /// Environment snapshot (for `$PAVO_CONFIG*` and XDG/HOME lookups).
    pub env: BTreeMap<String, String>,
}

impl Options {
    /// Snapshot the live process environment.
    #[must_use]
    pub fn from_process() -> Self {
        Self {
            explicit: None, // main() fills this from argv
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            env: std::env::vars().collect(),
        }
    }

    /// `$PAVO_CONFIG` as the explicit file (when `--config` wasn't
    /// given on argv).
    fn env_explicit(&self) -> Option<PathBuf> {
        self.env
            .get("PAVO_CONFIG")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    }
}

/// Built-in defaults — the weakest layer.
fn defaults() -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("theme".to_string(), Value::from("dark"));
    m.insert("statusbar".to_string(), Value::from(true));
    m.insert("refresh_ms".to_string(), Value::from(5000));
    m
}

/// Load and merge every config layer. Never fails wholesale — a bad
/// file becomes an `error` entry in [`Config::sources`] and its keys
/// are simply absent from the merge (provenance over silence).
#[must_use]
pub fn load(opts: &Options) -> Config {
    let mut sources: Vec<SourceInfo> = Vec::new();
    let mut layers: Vec<FileLayer> = Vec::new();

    // -- defaults ------------------------------------------------------
    let dkeys = flatten_count(&defaults());
    sources.push(SourceInfo {
        label: "defaults".to_string(),
        path: None,
        kind: "defaults".to_string(),
        precedence: 0,
        keys: dkeys,
        error: None,
    });

    // -- global ----------------------------------------------------------
    for path in global_candidates(&opts.env) {
        push_file(&mut layers, &mut sources, "global", path, 10);
    }

    // -- project: pavo.toml/json in every ancestor of cwd ---------------
    let mut project_files: Vec<PathBuf> = Vec::new();
    for dir in ancestors(&opts.cwd) {
        for name in ["pavo.toml", "pavo.json"] {
            let path = dir.join(name);
            if path.is_file() {
                project_files.push(path);
            }
        }
    }
    // ancestors() yields cwd → root; reverse so the root-most file is
    // weakest and the cwd-nearest is strongest.
    project_files.reverse();
    for (i, path) in project_files.iter().enumerate() {
        push_file(
            &mut layers,
            &mut sources,
            "project",
            path.clone(),
            20 + i as u32,
        );
    }

    // -- explicit (--config arg wins over $PAVO_CONFIG) ------------------
    let explicit = opts
        .explicit
        .clone()
        .or_else(|| opts.env_explicit())
        .map(expand_tilde);
    if let Some(path) = &explicit {
        push_file(
            &mut layers,
            &mut sources,
            if opts.explicit.is_some() {
                "--config"
            } else {
                "PAVO_CONFIG"
            },
            path.clone(),
            90,
        );
    }

    // -- env overrides ----------------------------------------------------
    let env_map = env_overrides(&opts.env);
    let ekeys = flatten_count(&env_map);
    sources.push(SourceInfo {
        label: "env:PAVO_CONFIG_*".to_string(),
        path: None,
        kind: "env".to_string(),
        precedence: 99,
        keys: ekeys,
        error: None,
    });

    // -- merge ------------------------------------------------------------
    let mut merged = Value::Object(defaults());
    for layer in &layers {
        merge_into(&mut merged, &Value::Object(layer.value.clone()));
    }
    merge_into(&mut merged, &Value::Object(env_map));
    let flat = flatten(&merged);

    // -- write target -------------------------------------------------------
    let write_target = explicit.clone().unwrap_or_else(|| {
        project_files
            .last()
            .cloned()
            .unwrap_or_else(|| opts.cwd.join("pavo.toml"))
    });

    Config {
        merged,
        flat,
        sources,
        write_target,
    }
}

/// Dotted-key lookup over the merged config (`myco.theme`).
/// Returns `None` for missing keys *and* present-but-null — config
/// has no nulls, so the distinction doesn't exist here.
#[must_use]
pub fn get<'a>(cfg: &'a Config, key: &str) -> Option<&'a Value> {
    cfg.flat.get(key)
}

/// Write `key=value` into the resolved file. `key` may be dotted
/// (`server.port`) — TOML tables/JSON objects are created as needed.
///
/// # Errors
/// Invalid key, unrepresentable value (null/objects in TOML),
/// unreadable target, or I/O failure — returned as a message safe to
/// surface on the wire.
pub fn set(cfg: &Config, key: &str, value: &Value) -> Result<SetOutcome, String> {
    let segments = key.split('.').map(str::trim).collect::<Vec<_>>();
    if segments.is_empty()
        || segments.iter().any(|s| s.is_empty())
        || !segments.iter().all(|s| {
            s.chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        })
    {
        return Err(format!("invalid config key: {key:?}"));
    }

    let target = &cfg.write_target;
    let kind = file_kind(target)?;
    match kind {
        FileKind::Json => set_json(target, &segments, value),
        FileKind::Toml => set_toml(target, &segments, value),
    }
    .map(|()| SetOutcome {
        key: key.to_string(),
        value: value.clone(),
        path: target.clone(),
    })
}

/// What `set` did — echoed back in the query payload.
#[derive(Clone, Debug)]
pub struct SetOutcome {
    /// The key that was written.
    pub key: String,
    /// The value that was written.
    pub value: Value,
    /// The file it landed in.
    pub path: PathBuf,
}

// ------------------------------------------------------------------
// layer discovery

/// `~/.config/pavo/` honoring `$XDG_CONFIG_HOME` — both `pavo.toml`
/// and `pavo.json` are candidates when present.
fn global_candidates(env: &BTreeMap<String, String>) -> Vec<PathBuf> {
    let base = env
        .get("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env.get("HOME")
                .filter(|v| !v.is_empty())
                .map(|h| PathBuf::from(h).join(".config"))
        });
    let Some(base) = base else { return Vec::new() };
    let dir = base.join("pavo");
    ["pavo.toml", "pavo.json"]
        .iter()
        .map(|n| dir.join(n))
        .filter(|p| p.is_file())
        .collect()
}

/// cwd and every ancestor, nearest first.
fn ancestors(cwd: &Path) -> Vec<PathBuf> {
    cwd.ancestors().map(Path::to_path_buf).collect()
}

/// `~/…` expansion for explicit paths (pavo never sets HOME-relative
/// semantics itself, but users type `~/pavo.toml`).
fn expand_tilde(path: PathBuf) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    path
}

/// Load one candidate file into a layer, or record why it couldn't
/// be read. Missing files are skipped silently *unless* the caller
/// wants provenance for an explicit path — `push_file` records a
/// `missing` entry only for non-existent files when they were
/// explicitly requested... simpler: missing files never produce a
/// layer; explicit-missing is still reported via `sources` as a
/// `missing` entry so `set` has somewhere honest to write.
fn push_file(
    layers: &mut Vec<FileLayer>,
    sources: &mut Vec<SourceInfo>,
    label: &str,
    path: PathBuf,
    precedence: u32,
) {
    if !path.is_file() {
        // Only surface missing files that were *explicitly* pointed
        // at (--config / PAVO_CONFIG) — global/project misses are the
        // normal case and would just be noise.
        if precedence >= 90 {
            sources.push(SourceInfo {
                label: label.to_string(),
                path: Some(path),
                kind: "missing".to_string(),
                precedence,
                keys: 0,
                error: None,
            });
        }
        return;
    }
    match load_file(&path) {
        Ok((kind, value)) => {
            let flat_keys = flatten_count(&value);
            sources.push(SourceInfo {
                label: label.to_string(),
                path: Some(path.clone()),
                kind: kind.as_str().to_string(),
                precedence,
                keys: flat_keys,
                error: None,
            });
            layers.push(FileLayer { value });
        }
        Err(e) => sources.push(SourceInfo {
            label: label.to_string(),
            path: Some(path),
            kind: "error".to_string(),
            precedence,
            keys: 0,
            error: Some(e),
        }),
    }
}

/// Read + parse one file into a JSON object.
fn load_file(path: &Path) -> Result<(FileKind, Map<String, Value>), String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let kind = sniff_kind(path, &text)?;
    let value = match kind {
        FileKind::Json => serde_json::from_str::<Value>(&text)
            .map_err(|e| format!("invalid JSON in {}: {e}", path.display()))?,
        FileKind::Toml => toml_to_json(
            &text
                .parse::<DocumentMut>()
                .map_err(|e| format!("invalid TOML in {}: {e}", path.display()))?,
        ),
    };
    match value {
        Value::Object(map) => Ok((kind, map)),
        _ => Err(format!("{}: top level must be an object", path.display())),
    }
}

/// Pick the parser: extension first, content sniff for extensionless
/// files, explicit error for YAML.
fn sniff_kind(path: &Path, text: &str) -> Result<FileKind, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("json") => return Ok(FileKind::Json),
        Some("toml") => return Ok(FileKind::Toml),
        Some("yaml" | "yml") => {
            return Err(format!(
                "{}: YAML is not supported (no yaml dep in the workspace) — use TOML or JSON",
                path.display()
            ));
        }
        _ => {}
    }
    let trimmed = text.trim_start();
    if trimmed.starts_with('{') {
        Ok(FileKind::Json)
    } else {
        Ok(FileKind::Toml)
    }
}

/// Kind for `set`'s target — same sniffing, but a *missing* file
/// falls back to extension (`.json` → JSON, everything else TOML).
fn file_kind(path: &Path) -> Result<FileKind, String> {
    if path.is_file() {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        sniff_kind(path, &text)
    } else {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref()
        {
            Some("json") => Ok(FileKind::Json),
            Some("yaml" | "yml") => Err(format!(
                "{}: YAML is not supported — use TOML or JSON",
                path.display()
            )),
            _ => Ok(FileKind::Toml),
        }
    }
}

// ------------------------------------------------------------------
// TOML ↔ JSON

/// `toml_edit` document → `serde_json` object (datetimes stringify).
fn toml_to_json(doc: &DocumentMut) -> Value {
    table_to_json(doc.as_table())
}

fn table_to_json(table: &Table) -> Value {
    let mut map = Map::new();
    for (key, item) in table.iter() {
        map.insert(key.to_string(), item_to_json(item));
    }
    Value::Object(map)
}

fn item_to_json(item: &Item) -> Value {
    match item {
        Item::None => Value::Null,
        Item::Value(v) => toml_value_to_json(v),
        Item::Table(t) => table_to_json(t),
        Item::ArrayOfTables(a) => Value::Array(a.iter().map(table_to_json).collect()),
    }
}

fn toml_value_to_json(v: &TomlValue) -> Value {
    match v {
        TomlValue::String(s) => Value::from(s.value().as_str()),
        TomlValue::Integer(i) => Value::from(*i.value()),
        TomlValue::Float(f) => Number::from_f64(*f.value())
            .map(Value::Number)
            .unwrap_or(Value::Null),
        TomlValue::Boolean(b) => Value::from(*b.value()),
        TomlValue::Datetime(d) => Value::from(d.value().to_string()),
        TomlValue::Array(a) => Value::Array(a.iter().map(toml_value_to_json).collect()),
        // Inline tables arrive as Value::InlineTable in toml_edit 0.25.
        TomlValue::InlineTable(t) => {
            let mut map = Map::new();
            for (k, v) in t.iter() {
                map.insert(k.to_string(), toml_value_to_json(v));
            }
            Value::Object(map)
        }
    }
}

/// `serde_json` value → `toml_edit` value. TOML has no null and no
/// arbitrary nested objects at a leaf (use dotted keys for tables);
/// arrays must be scalar.
fn json_to_toml(v: &Value) -> Result<TomlValue, String> {
    match v {
        Value::Null => Err("cannot store null in TOML".to_string()),
        Value::Bool(b) => Ok(TomlValue::from(*b)),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(TomlValue::from(i))
            } else if let Some(u) = n.as_u64() {
                match i64::try_from(u) {
                    Ok(i) => Ok(TomlValue::from(i)),
                    // u64 beyond i64::MAX — f64 keeps the magnitude.
                    Err(_) => n
                        .as_f64()
                        .map(TomlValue::from)
                        .ok_or_else(|| "number out of TOML range".to_string()),
                }
            } else {
                // serde_json numbers are always finite, so as_f64
                // only fails for integers already handled above.
                n.as_f64()
                    .map(TomlValue::from)
                    .ok_or_else(|| "number out of TOML range".to_string())
            }
        }
        Value::String(s) => Ok(TomlValue::from(s.clone())),
        Value::Array(items) => {
            let mut arr = Array::new();
            for item in items {
                arr.push(json_to_toml(item)?);
            }
            Ok(TomlValue::Array(arr))
        }
        Value::Object(_) => Err(
            "nested objects need dotted keys in TOML (e.g. server.port, not server={…})"
                .to_string(),
        ),
    }
}

// ------------------------------------------------------------------
// merge / flatten / env

/// Deep-merge `over` into `base` — objects recurse, everything else
/// replaces. Both must be objects at the top level.
fn merge_into(base: &mut Value, over: &Value) {
    if let (Value::Object(b), Value::Object(o)) = (base, over) {
        merge_maps(b, o);
    }
}

fn merge_maps(base: &mut Map<String, Value>, over: &Map<String, Value>) {
    for (k, v) in over {
        match (base.get_mut(k), v) {
            (Some(Value::Object(bm)), Value::Object(om)) => merge_maps(bm, om),
            _ => {
                base.insert(k.clone(), v.clone());
            }
        }
    }
}

/// Flatten an object tree to dotted keys; arrays/scalars stay atomic.
fn flatten(value: &Value) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    if let Value::Object(map) = value {
        flatten_into(map, "", &mut out);
    }
    out
}

fn flatten_into(obj: &Map<String, Value>, prefix: &str, out: &mut BTreeMap<String, Value>) {
    for (k, v) in obj {
        let key = if prefix.is_empty() {
            k.clone()
        } else {
            format!("{prefix}.{k}")
        };
        match v {
            Value::Object(inner) => flatten_into(inner, &key, out),
            _ => {
                out.insert(key, v.clone());
            }
        }
    }
}

fn flatten_count(map: &Map<String, Value>) -> usize {
    let mut out = BTreeMap::new();
    flatten_into(map, "", &mut out);
    out.len()
}

/// `PAVO_CONFIG_*` env vars → nested override object.
/// `__` separates dotted segments; `_` stays literal; suffix
/// lowercases. `PAVO_CONFIG` itself is the explicit-path var and is
/// excluded.
fn env_overrides(env: &BTreeMap<String, String>) -> Map<String, Value> {
    let mut root = Map::new();
    for (name, raw) in env {
        let Some(suffix) = name.strip_prefix("PAVO_CONFIG_") else {
            continue;
        };
        if suffix.is_empty() {
            continue;
        }
        let key = suffix
            .split("__")
            .map(str::to_lowercase)
            .collect::<Vec<_>>()
            .join(".");
        let parsed =
            serde_json::from_str::<Value>(raw).unwrap_or_else(|_| Value::from(raw.clone()));
        insert_dotted(&mut root, &key, parsed);
    }
    root
}

/// Insert `value` at a dotted path inside a JSON object, creating
/// intermediate objects.
fn insert_dotted(map: &mut Map<String, Value>, key: &str, value: Value) {
    let mut segments = key.split('.');
    insert_at(map, &mut segments, value);
}

fn insert_at<'k, 'v>(
    map: &'v mut Map<String, Value>,
    segments: &mut std::str::Split<'k, char>,
    value: Value,
) {
    let Some(seg) = segments.next() else { return };
    if segments.clone().next().is_none() {
        map.insert(seg.to_string(), value);
        return;
    }
    let entry = map
        .entry(seg.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    if let Value::Object(inner) = entry {
        insert_at(inner, segments, value);
    }
}

// ------------------------------------------------------------------
// set — write back

fn set_json(path: &Path, segments: &[&str], value: &Value) -> Result<(), String> {
    let mut doc: Map<String, Value> = if path.is_file() {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(m)) => m,
            Ok(_) => return Err(format!("{}: top level must be an object", path.display())),
            Err(e) => return Err(format!("invalid JSON in {}: {e}", path.display())),
        }
    } else {
        Map::new()
    };
    insert_json_dotted(&mut doc, segments, value.clone());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let mut out = serde_json::to_string_pretty(&Value::Object(doc))
        .map_err(|e| format!("cannot serialize {}: {e}", path.display()))?;
    out.push('\n');
    std::fs::write(path, out).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

fn insert_json_dotted(map: &mut Map<String, Value>, segments: &[&str], value: Value) {
    let Some((first, rest)) = segments.split_first() else {
        return;
    };
    if rest.is_empty() {
        map.insert((*first).to_string(), value);
        return;
    }
    let entry = map
        .entry((*first).to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    if let Value::Object(inner) = entry {
        insert_json_dotted(inner, rest, value);
    }
}

fn set_toml(path: &Path, segments: &[&str], value: &Value) -> Result<(), String> {
    let mut doc: DocumentMut = if path.is_file() {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        text.parse::<DocumentMut>()
            .map_err(|e| format!("invalid TOML in {}: {e}", path.display()))?
    } else {
        DocumentMut::new()
    };
    let leaf = json_to_toml(value)?;
    let (last, parents) = segments
        .split_last()
        .ok_or_else(|| "empty key".to_string())?;
    let mut table = doc.as_table_mut();
    for seg in parents {
        let item = table
            .entry(seg)
            .or_insert_with(|| Item::Table(Table::new()));
        table = item
            .as_table_mut()
            .ok_or_else(|| format!("config key {seg:?} is not a table — cannot descend"))?;
    }
    let slot = table.entry(last).or_insert(Item::None);
    *slot = Item::Value(leaf);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    std::fs::write(path, doc.to_string())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

// ------------------------------------------------------------------
// tests

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use serde_json::json;

    fn opts(cwd: &Path, env: &[(&str, &str)]) -> Options {
        Options {
            explicit: None,
            cwd: cwd.to_path_buf(),
            env: env
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
        }
    }

    #[test]
    fn defaults_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = load(&opts(tmp.path(), &[("HOME", "/nonexistent")]));
        assert_eq!(get(&cfg, "theme"), Some(&Value::from("dark")));
        assert_eq!(get(&cfg, "refresh_ms"), Some(&Value::from(5000)));
        assert_eq!(cfg.sources.len(), 2); // defaults + env
    }

    #[test]
    fn project_beats_global_beats_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let global = home.join(".config/pavo");
        std::fs::create_dir_all(&global).unwrap();
        std::fs::write(
            global.join("pavo.toml"),
            "theme = \"light\"\n[ui]\nscale = 2\n",
        )
        .unwrap();
        let proj = tmp.path().join("proj");
        let nested = proj.join("a/b");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(proj.join("pavo.toml"), "theme = \"solarized\"\n").unwrap();
        std::fs::write(nested.join("pavo.json"), "{\"ui\":{\"scale\":3}}").unwrap();

        let cfg = load(&opts(
            &nested,
            &[("HOME", home.to_str().unwrap()), ("XDG_CONFIG_HOME", "")],
        ));
        // nearest project file wins over global + root project file
        assert_eq!(get(&cfg, "theme"), Some(&Value::from("solarized")));
        // nested merge: ui.scale from cwd-nearest json beats global toml
        assert_eq!(get(&cfg, "ui.scale"), Some(&Value::from(3)));
        assert_eq!(get(&cfg, "statusbar"), Some(&Value::from(true)));
        // sources: defaults, global, 2 project files, env
        let labels: Vec<&str> = cfg.sources.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "defaults",
                "global",
                "project",
                "project",
                "env:PAVO_CONFIG_*"
            ]
        );
    }

    #[test]
    fn env_overrides_win_and_decode_dotted_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = load(&opts(
            tmp.path(),
            &[
                ("HOME", "/nonexistent"),
                ("PAVO_CONFIG_THEME", "\"neon\""),
                ("PAVO_CONFIG_MYCO__EDITOR", "vim"),
                ("PAVO_CONFIG_REFRESH_MS", "250"),
            ],
        ));
        assert_eq!(get(&cfg, "theme"), Some(&Value::from("neon")));
        assert_eq!(get(&cfg, "myco.editor"), Some(&Value::from("vim")));
        assert_eq!(get(&cfg, "refresh_ms"), Some(&Value::from(250)));
    }

    #[test]
    fn explicit_config_is_top_file_layer() {
        let tmp = tempfile::tempdir().unwrap();
        let explicit = tmp.path().join("custom.toml");
        std::fs::write(&explicit, "theme = \"explicit\"\n").unwrap();
        let mut o = opts(tmp.path(), &[("HOME", "/nonexistent")]);
        o.explicit = Some(explicit.clone());
        let cfg = load(&o);
        assert_eq!(get(&cfg, "theme"), Some(&Value::from("explicit")));
        assert_eq!(cfg.write_target, explicit);
    }

    #[test]
    fn yaml_reports_error_not_silence() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("pavo.yaml"), "theme: light\n").unwrap();
        // .yaml isn't a candidate name — but an explicit .yaml must error.
        let mut o = opts(tmp.path(), &[("HOME", "/nonexistent")]);
        o.explicit = Some(tmp.path().join("pavo.yaml"));
        let cfg = load(&o);
        let src = cfg.sources.iter().find(|s| s.kind == "error").unwrap();
        assert!(src.error.as_deref().unwrap().contains("YAML"));
        assert_eq!(get(&cfg, "theme"), Some(&Value::from("dark")));
    }

    #[test]
    fn set_writes_dotted_toml_and_reads_back() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("pavo.toml"), "theme = \"dark\"\n").unwrap();
        let cfg = load(&opts(tmp.path(), &[("HOME", "/nonexistent")]));
        set(&cfg, "server.port", &json!(8080)).unwrap();
        set(&cfg, "server.host", &json!("localhost")).unwrap();
        let cfg2 = load(&opts(tmp.path(), &[("HOME", "/nonexistent")]));
        assert_eq!(get(&cfg2, "server.port"), Some(&Value::from(8080)));
        assert_eq!(get(&cfg2, "server.host"), Some(&Value::from("localhost")));
        assert_eq!(get(&cfg2, "theme"), Some(&Value::from("dark")));
        // comments/formatting preserved
        let text = std::fs::read_to_string(tmp.path().join("pavo.toml")).unwrap();
        assert!(text.contains("theme = \"dark\""));
    }

    #[test]
    fn set_creates_pavo_toml_when_no_project_file() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = load(&opts(tmp.path(), &[("HOME", "/nonexistent")]));
        assert_eq!(cfg.write_target, tmp.path().join("pavo.toml"));
        set(&cfg, "accent", &json!("blue")).unwrap();
        let cfg2 = load(&opts(tmp.path(), &[("HOME", "/nonexistent")]));
        assert_eq!(get(&cfg2, "accent"), Some(&Value::from("blue")));
    }

    #[test]
    fn set_json_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("pavo.json"),
            "{\"theme\": \"dark\", \"nested\": {\"a\": 1}}\n",
        )
        .unwrap();
        let cfg = load(&opts(tmp.path(), &[("HOME", "/nonexistent")]));
        set(&cfg, "nested.b", &json!(true)).unwrap();
        let cfg2 = load(&opts(tmp.path(), &[("HOME", "/nonexistent")]));
        assert_eq!(get(&cfg2, "nested.b"), Some(&Value::from(true)));
        assert_eq!(get(&cfg2, "nested.a"), Some(&Value::from(1)));
    }

    #[test]
    fn set_rejects_bad_keys_and_toml_null() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = load(&opts(tmp.path(), &[("HOME", "/nonexistent")]));
        assert!(set(&cfg, "", &json!(1)).is_err());
        assert!(set(&cfg, "a..b", &json!(1)).is_err());
        assert!(set(&cfg, "a.b", &Value::Null).is_err());
        assert!(set(&cfg, "a.b", &json!({"x": 1})).is_err());
    }

    #[test]
    fn toml_comments_survive_set() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("pavo.toml"),
            "# user theme\ntheme = \"dark\" # trailing\n",
        )
        .unwrap();
        let cfg = load(&opts(tmp.path(), &[("HOME", "/nonexistent")]));
        set(&cfg, "theme", &json!("light")).unwrap();
        let text = std::fs::read_to_string(tmp.path().join("pavo.toml")).unwrap();
        assert!(text.contains("# user theme"));
    }
}
