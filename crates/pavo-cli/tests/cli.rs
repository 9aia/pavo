//! Integration tests — spawn the real `pavo` binary against a fixture
//! `tamias-driver-demo` shell script. Discovery is env-driven
//! (`PATH` + `TAMIAS_HOME`), same mechanism pavo-core's tests inject
//! via `DiscoveryOptions`.

#![allow(clippy::unwrap_used)]

use std::process::{Command, Output};

/// Fixture driver `demo`: eager `manifest` + `serve` wire with two
/// views — `board` (kv + table + markdown, query-fed) and `deps`
/// (graph). Pure sh builtins so it runs with a bare PATH.
const DEMO: &str = r#"#!/bin/sh
if [ "$1" = manifest ]; then
  printf '%s\n' '{"name":"demo","title":"Demo Driver","version":"1.2.3","verbs":["manifest","serve"],"capabilities":["pavo"],"contributes":{"navbar":[{"id":"nav","title":"Demo Nav","route":"/demo/board","priority":5}],"statusbar":[{"id":"st","title":"Demo status","route":"/demo/board"}]},"views":[{"id":"board","title":"Board","route":"/demo/board","description":"The board","queries":[{"name":"items","op":"query"},{"name":"counts","op":"query"}],"root":{"kind":"column","title":"Board","children":[{"kind":"kv","query":"counts","pairs":[]},{"kind":"table","title":"Items","query":"items","columns":[{"key":"0","title":"ID"},{"key":"1","title":"Title"},{"key":"2","title":"Status"}],"rows":[]},{"kind":"markdown","text":"footer note"}]}},{"id":"deps","title":"Deps","route":"/demo/deps","queries":[],"root":{"kind":"graph","nodes":[{"id":"a","label":"Alpha"},{"id":"b","label":"Beta"},{"id":"c","label":"Gamma"}],"edges":[{"from":"a","to":"b","label":"uses"},{"from":"b","to":"c"}]}}]}'
  exit 0
fi
if [ "$1" = serve ]; then
  while IFS= read -r line; do
    case "$line" in
      *'"op":"views"'*)
        printf '%s\n' '{"ok":true,"result":[{"id":"board","title":"Board","route":"/demo/board","queries":["items","counts"]},{"id":"deps","title":"Deps","route":"/demo/deps","queries":[]}]}';;
      *'"op":"view","view":"board"'*)
        printf '%s\n' '{"ok":true,"result":{"view":{"id":"board","title":"Board","route":"/demo/board","description":"The board","queries":[{"name":"items","op":"query"},{"name":"counts","op":"query"}],"root":{"kind":"column","title":"Board","children":[{"kind":"kv","query":"counts","pairs":[]},{"kind":"table","title":"Items","query":"items","columns":[{"key":"0","title":"ID"},{"key":"1","title":"Title"},{"key":"2","title":"Status"}],"rows":[]},{"kind":"markdown","text":"footer note"}]}},"data":{"items":[["T-1","Alpha","done"],["T-2","Beta","wip"]],"counts":{"open":1,"done":1}}}}';;
      *'"op":"view","view":"deps"'*)
        printf '%s\n' '{"ok":true,"result":{"view":{"id":"deps","title":"Deps","route":"/demo/deps","queries":[],"root":{"kind":"graph","nodes":[{"id":"a","label":"Alpha"},{"id":"b","label":"Beta"},{"id":"c","label":"Gamma"}],"edges":[{"from":"a","to":"b","label":"uses"},{"from":"b","to":"c"}]}},"data":{}}}';;
      *'"op":"view"'*)
        printf '%s\n' '{"ok":false,"error":"unknown view"}';;
      *'"op":"query"'*)
        printf '%s\n' '{"ok":true,"result":{"query":"items","data":[["T-1","Alpha","done"],["T-2","Beta","wip"]]}}';;
      *'"op":"manifest"'*)
        printf '%s\n' '{"ok":true,"result":{"name":"demo","title":"Demo Driver","version":"1.2.3","verbs":["serve"],"capabilities":["pavo"],"views":[{"id":"board","title":"Board","route":"/demo/board","root":{"kind":"kv","pairs":[]}}]}}';;
    esac
  done
  exit 0
fi
exit 1
"#;

/// Renderer-plugin driver `viz`: declares a `graph` renderer for cli
/// formats `svg`/`ascii` (+tui/web) and answers `render` with canned
/// RenderModels. Zero views — renderer-only.
const VIZ: &str = r#"#!/bin/sh
if [ "$1" = manifest ]; then
  printf '%s\n' '{"name":"viz","title":"Viz Plugin","version":"0.1.0","verbs":["manifest","serve"],"capabilities":["pavo"],"contributes":{},"views":[],"renderers":[{"id":"graph","kinds":["graph"],"priority":50,"surfaces":{"cli":{"formats":["svg","ascii"]},"tui":true,"web":{"component":"viz-graph","island":true}}}]}'
  exit 0
fi
if [ "$1" = serve ]; then
  while IFS= read -r line; do
    case "$line" in
      *'"op":"render"'*'"format":"svg"'*)
        printf '%s\n' '{"ok":true,"result":{"type":"svg","markup":"<svg>PLUGIN-SVG</svg>"}}';;
      *'"op":"render"'*'"format":"ascii"'*)
        printf '%s\n' '{"ok":true,"result":{"type":"text","lines":["PLUGIN-ASCII","a -> b"]}}';;
      *'"op":"render"'*)
        printf '%s\n' '{"ok":true,"result":{"type":"rows","headers":["id","label"],"rows":[["a","Alpha"]]}}';;
      *'"op":"views"'*)
        printf '%s\n' '{"ok":true,"result":[]}';;
    esac
  done
  exit 0
fi
exit 1
"#;

struct Fixture {
    _tmp: tempfile::TempDir,
    bin: std::path::PathBuf,
}

fn write_driver(bin: &std::path::Path, name: &str, script: &str) {
    let driver = bin.join(name);
    std::fs::write(&driver, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&driver, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    write_driver(&bin, "tamias-driver-demo", DEMO);
    Fixture { _tmp: tmp, bin }
}

/// Same fixture plus the `viz` renderer plugin claiming `graph`.
fn fixture_with_plugin() -> Fixture {
    let fx = fixture();
    write_driver(&fx.bin, "tamias-driver-viz", VIZ);
    fx
}

/// Run `pavo <args>` with PATH=<fixture bin> and TAMIAS_HOME=<empty>.
fn pavo(fx: &Fixture, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pavo"))
        .args(args)
        .env("PATH", &fx.bin)
        .env("TAMIAS_HOME", fx.bin.parent().unwrap().join("home"))
        .output()
        .unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn drivers_lists_manifest_summary() {
    let fx = fixture();
    let o = pavo(&fx, &["drivers"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let out = stdout(&o);
    assert!(out.contains("demo"), "{out}");
    assert!(out.contains("Demo Driver"), "{out}");
    assert!(out.contains("1.2.3"), "{out}");
    assert!(out.contains("views:2"), "{out}");
    assert!(out.contains("navbar:1"), "{out}");
    assert!(out.contains("statusbar:1"), "{out}");
}

#[test]
fn drivers_json() {
    let fx = fixture();
    let o = pavo(&fx, &["drivers", "--json"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    let row = &v[0];
    assert_eq!(row["driver"], "demo");
    assert_eq!(row["version"], "1.2.3");
    assert_eq!(row["views"], 2);
    assert_eq!(row["contributes"]["navbar"], 1);
}

#[test]
fn views_lists_namespaced_refs() {
    let fx = fixture();
    let o = pavo(&fx, &["views"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let out = stdout(&o);
    assert!(out.contains("demo:board"), "{out}");
    assert!(out.contains("demo:deps"), "{out}");
    assert!(out.contains("/demo/board"), "{out}");

    // --driver filter + json
    let o = pavo(&fx, &["views", "--driver", "demo", "--json"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 2);
    assert_eq!(v[0]["namespaced"], "demo:board");
}

#[test]
fn chrome_merges_contributions() {
    let fx = fixture();
    let o = pavo(&fx, &["chrome"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let out = stdout(&o);
    assert!(out.contains("navbar:"), "{out}");
    assert!(out.contains("demo:nav"), "{out}");
    assert!(out.contains("demo:st"), "{out}");
    assert!(out.contains("/demo/board"), "{out}");

    let o = pavo(&fx, &["chrome", "--json"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["navbar"][0]["id"], "demo:nav");
    assert_eq!(v["statusbar"][0]["id"], "demo:st");
}

#[test]
fn view_text_renders_tree_and_data() {
    let fx = fixture();
    let o = pavo(&fx, &["view", "demo:board"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let out = stdout(&o);
    assert!(out.contains("Board"), "{out}");
    assert!(out.contains("open: 1"), "{out}");
    assert!(out.contains("done: 1"), "{out}");
    assert!(out.contains("ID"), "{out}");
    assert!(out.contains("T-1"), "{out}");
    assert!(out.contains("Alpha"), "{out}");
    assert!(out.contains("footer note"), "{out}");
}

#[test]
fn view_json_dumps_view_result() {
    let fx = fixture();
    let o = pavo(&fx, &["view", "demo:board", "--format", "json"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["view"]["id"], "board");
    assert_eq!(v["data"]["items"][0][0], "T-1");
    assert_eq!(v["data"]["counts"]["open"], 1);
}

#[test]
fn view_dot_and_mermaid_graphs() {
    let fx = fixture();
    let o = pavo(&fx, &["view", "demo:deps", "--format", "dot"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let out = stdout(&o);
    assert!(out.contains("digraph"), "{out}");
    assert!(out.contains("\"a\" -> \"b\" [label=\"uses\"]"), "{out}");
    assert!(out.contains("\"b\" -> \"c\""), "{out}");

    let o = pavo(&fx, &["view", "demo:deps", "--format", "mermaid"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let out = stdout(&o);
    assert!(out.starts_with("graph TD"), "{out}");
    assert!(out.contains("-->"), "{out}");
    assert!(out.contains("-->|uses|"), "{out}");
}

#[test]
fn view_plugin_format_delegates_to_renderer() {
    let fx = fixture_with_plugin();
    // --format svg is claimed by viz → plugin SVG markup verbatim.
    let o = pavo(&fx, &["view", "demo:deps", "--format", "svg"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(
        stdout(&o).contains("<svg>PLUGIN-SVG</svg>"),
        "{}",
        stdout(&o)
    );

    // --format ascii → plugin text model lines.
    let o = pavo(&fx, &["view", "demo:deps", "--format", "ascii"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let out = stdout(&o);
    assert!(out.contains("PLUGIN-ASCII"), "{out}");
    assert!(out.contains("a -> b"), "{out}");

    // Built-in formats stay built-in even with the plugin installed.
    let o = pavo(&fx, &["view", "demo:deps", "--format", "dot"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(stdout(&o).contains("digraph"), "{}", stdout(&o));

    // Plain text: plugin's cli.formats doesn't claim "text" → built-in.
    let o = pavo(&fx, &["view", "demo:deps"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(!stdout(&o).contains("PLUGIN"), "{}", stdout(&o));
}

#[test]
fn view_plugin_text_substitution_and_unknown_format() {
    let fx = fixture_with_plugin();
    // A plugin claiming "text" would substitute — ours doesn't, so
    // text stays built-in (covered above). An unclaimed format errors
    // helpfully instead of silently rendering.
    let o = pavo(&fx, &["view", "demo:deps", "--format", "bogus"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        stderr(&o).contains("no renderer plugin handles --format 'bogus'"),
        "{}",
        stderr(&o)
    );
}

#[test]
fn drivers_lists_renderer_count() {
    let fx = fixture_with_plugin();
    let o = pavo(&fx, &["drivers"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let out = stdout(&o);
    assert!(out.contains("viz"), "{out}");
    assert!(out.contains("renderers:1"), "{out}");

    // viz declares zero views — still discovered as a driver.
    let o = pavo(&fx, &["views", "--driver", "viz", "--json"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 0);
}

#[test]
fn view_params_and_bare_driver() {
    let fx = fixture();
    // --param parses (JSON value + bare string); bare driver = first view
    let o = pavo(
        &fx,
        &[
            "view",
            "demo",
            "--param",
            "limit=5",
            "--param",
            "flag=true",
            "--param",
            "name=alpha",
        ],
    );
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(stdout(&o).contains("Board"));
}

#[test]
fn view_dot_without_graph_is_error() {
    let fx = fixture();
    let o = pavo(&fx, &["view", "demo:board", "--format", "dot"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("no graph nodes"), "{}", stderr(&o));
}

#[test]
fn query_passthrough() {
    let fx = fixture();
    let o = pavo(&fx, &["query", "demo", "items"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let out = stdout(&o);
    assert!(out.contains("T-1"), "{out}");

    let o = pavo(&fx, &["query", "demo", "items", "--json"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["query"], "items");
    assert_eq!(v["data"][0][0], "T-1");
}

#[test]
fn errors_and_usage_codes() {
    let fx = fixture();
    // unknown driver → exit 1
    let o = pavo(&fx, &["view", "ghost:board"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("unknown driver"), "{}", stderr(&o));

    // unknown view → exit 1
    let o = pavo(&fx, &["view", "demo:nope"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("unknown view"), "{}", stderr(&o));

    // bad --param → exit 2
    let o = pavo(&fx, &["view", "demo:board", "--param", "noequals"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(stderr(&o).contains("key=value"), "{}", stderr(&o));

    // malformed view ref → exit 2
    let o = pavo(&fx, &["view", "demo:"]);
    assert_eq!(o.status.code(), Some(2));
}

#[test]
fn manifest_and_schema_self_description() {
    let fx = fixture();
    let o = pavo(&fx, &["manifest"]);
    assert!(o.status.success());
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["name"], "pavo");
    let verbs = v["verbs"].as_array().unwrap();
    for verb in [
        "drivers", "views", "chrome", "view", "query", "manifest", "schema",
    ] {
        assert!(verbs.iter().any(|x| x == verb), "missing verb {verb}");
    }

    let o = pavo(&fx, &["schema"]);
    assert!(o.status.success());
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert!(v["verbs"]["view"].is_object());
    assert!(v["exit_codes"]["2"].is_string());
}

#[test]
fn open_without_tty_and_no_target_is_usage_error() {
    let fx = fixture();
    // Tests run with piped stdio — never a TTY — so bare `open`
    // must fail as usage rather than spawning pavo-tui.
    let o = pavo(&fx, &["open"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(stderr(&o).contains("needs a terminal"), "{}", stderr(&o));
}

#[test]
fn open_non_tty_falls_back_to_text_view() {
    let fx = fixture();
    let o = pavo(&fx, &["open", "demo:board"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(
        stderr(&o).contains("not a TTY"),
        "missing fallback note: {}",
        stderr(&o)
    );
    let out = stdout(&o);
    assert!(out.contains("Board"), "{out}");
    assert!(out.contains("T-1"), "{out}");
}

#[test]
fn open_unknown_target_errors() {
    let fx = fixture();
    let o = pavo(&fx, &["open", "ghost:board"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("unknown driver"), "{}", stderr(&o));
}
