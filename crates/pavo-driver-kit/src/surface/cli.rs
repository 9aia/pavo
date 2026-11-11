//! Ops manifest → a generated clap CLI (feature `cli`).
//!
//! [`command`] builds the parser: one subcommand per op (params →
//! args per [`ParamKind`]) plus one per entry in `surfaces` (trailing
//! argv passthrough — unbounded surfaces stream). [`run`] parses an
//! argv, builds the wire params map, dispatches through a [`Surface`],
//! prints `stdout`/`stderr`, and returns the engine's exit code.
//!
//! Every subcommand also accepts `--root <dir>` — the repo/dir hint
//! wire requests already carry (`params.root`); drivers that resolve
//! roots from params honor it, others ignore it.
//!
//! Every op subcommand also accepts `--params-stdin` — read the op's
//! params as one JSON object on stdin and merge them **over** the
//! argv params (stdin wins per key). This is the channel for
//! secret-bearing params ([`ParamSpec::secret`]): values never touch
//! argv, shell history, or `/proc`. Required (`arg`) params may come
//! from either side; a missing one is a usage error either way.

use std::io::{self, Read, Write};

use clap::{Arg, ArgAction, ArgMatches, Command};
use serde_json::{Map, Value};

use super::{OpOutcome, OpsManifest, ParamKind, Surface};

/// clap's `Str`/`Id` want `'static` — op/param names are runtime
/// data, and a generated CLI parses once per process, so leaking
/// is the honest cost.
fn s(v: String) -> &'static str {
    Box::leak(v.into_boxed_str())
}

/// Build the generated CLI's clap parser for `manifest`. `bin` is the
/// displayed program name (`kuda`, `tamias-driver-cli`, …).
#[must_use]
pub fn command(bin: &str, manifest: &OpsManifest) -> Command {
    let mut cmd = Command::new(s(bin.to_string()))
        .about(format!(
            "{} ops — generated CLI over its tamias driver",
            manifest.engine
        ))
        .subcommand_required(true)
        .arg_required_else_help(true)
        .arg(
            Arg::new("root")
                .long("root")
                .value_name("DIR")
                .global(true)
                .help("repo/dir override — passed through as the `root` wire param"),
        );
    for op in &manifest.ops {
        let mut sub = Command::new(s(op.name.clone())).about(s(op.doc.clone()));
        if op.mutates {
            sub = sub.after_help("mutates engine state");
        }
        let mut positional = 0usize;
        for param in &op.params {
            let arg = match param.kind {
                // Positional but not clap-required: `--params-stdin`
                // may supply it — presence is validated after the
                // stdin merge in `run_with_input`.
                ParamKind::Arg => {
                    positional += 1;
                    Arg::new(s(param.name.clone()))
                        .index(positional)
                        .value_name("VALUE")
                }
                // `--name`, `--name v`, `--name=v` — bare flag spells
                // `true`, a present value rides as a string.
                ParamKind::Opt => Arg::new(s(param.name.clone()))
                    .long(s(param.name.clone()))
                    .value_name("VALUE")
                    .num_args(0..=1)
                    .default_missing_value("true"),
                ParamKind::Flag => Arg::new(s(param.name.clone()))
                    .long(s(param.name.clone()))
                    .action(ArgAction::SetTrue),
            };
            let arg = if param.secret {
                // The marker steers callers to stdin; the value itself
                // is never echoed by the surface.
                let help = match &param.doc {
                    Some(doc) => format!("{doc} (secret — prefer --params-stdin)"),
                    None => "secret — prefer --params-stdin".to_string(),
                };
                arg.help(s(help))
            } else {
                match &param.doc {
                    Some(doc) => arg.help(s(doc.clone())),
                    None => arg,
                }
            };
            sub = sub.arg(arg);
        }
        sub = sub.arg(
            Arg::new("params-stdin")
                .long("params-stdin")
                .action(ArgAction::SetTrue)
                .help("read op params as a JSON object on stdin — stdin keys win over argv"),
        );
        cmd = cmd.subcommand(sub);
    }
    for surface in &manifest.surfaces {
        cmd = cmd.subcommand(
            Command::new(s(surface.clone()))
                .about(format!(
                    "{surface} — engine surface (argv passthrough, streams)"
                ))
                .arg(
                    Arg::new("args")
                        .num_args(0..)
                        .trailing_var_arg(true)
                        .allow_hyphen_values(true),
                ),
        );
    }
    cmd
}

/// Parse `argv` (argv[0] included) against [`command`], dispatch the
/// matched op/surface through `surface`, print captured output, and
/// return the exit code. clap errors/help print and exit `2`/`0`
/// through clap's own rendering. `--params-stdin` reads real stdin.
#[must_use]
pub fn run(bin: &str, manifest: &OpsManifest, argv: &[String], surface: &mut dyn Surface) -> i32 {
    let stdin = io::stdin();
    run_with_input(bin, manifest, argv, surface, &mut stdin.lock())
}

/// [`run`] with the stdin channel injectable — tests and embedders
/// pass a buffer; `tamias-driver-cli` uses [`run`].
#[must_use]
pub fn run_with_input(
    bin: &str,
    manifest: &OpsManifest,
    argv: &[String],
    surface: &mut dyn Surface,
    input: &mut dyn Read,
) -> i32 {
    let matches = match command(bin, manifest).try_get_matches_from(argv) {
        Ok(m) => m,
        Err(e) => {
            let code = e.exit_code();
            let _ = e.print();
            return code;
        }
    };
    let Some((name, sub)) = matches.subcommand() else {
        return 2; // arg_required_else_help makes this unreachable
    };
    if manifest.surfaces.iter().any(|s| s == name) {
        let args: Vec<String> = sub
            .get_many::<String>("args")
            .map(|v| v.cloned().collect())
            .unwrap_or_default();
        return surface.surface(name, &args);
    }
    let Some(op) = manifest.op(name) else {
        eprintln!("unknown op: {name}");
        return 2;
    };
    let mut params = params_of(sub, op, &matches);
    if sub.get_flag("params-stdin") {
        match stdin_params(input) {
            Ok(stdin_map) => params.extend(stdin_map), // stdin wins per key
            Err(e) => {
                eprintln!("{bin} {name}: {e}");
                return 2;
            }
        }
    }
    // `arg` params are clap-optional so stdin can satisfy them;
    // presence is enforced here, after the merge.
    for param in &op.params {
        if param.kind == ParamKind::Arg && !params.contains_key(&param.name) {
            eprintln!(
                "{bin} {name}: missing required param `{}` (positional or --params-stdin)",
                param.name
            );
            return 2;
        }
    }
    let outcome = surface.op(op, &params);
    print_outcome(&outcome)
}

/// `--params-stdin` payload → params object. Empty/whitespace stdin
/// is an empty map; anything else must be a JSON object — values may
/// be any JSON (bools, numbers, nested), not just argv-shaped
/// strings.
fn stdin_params(input: &mut dyn Read) -> Result<Map<String, Value>, String> {
    let mut buf = String::new();
    input
        .read_to_string(&mut buf)
        .map_err(|e| format!("--params-stdin: read: {e}"))?;
    if buf.trim().is_empty() {
        return Ok(Map::new());
    }
    let value: Value =
        serde_json::from_str(&buf).map_err(|e| format!("--params-stdin: bad JSON: {e}"))?;
    value
        .as_object()
        .cloned()
        .ok_or_else(|| "--params-stdin: expected a JSON object".to_string())
}

/// Subcommand matches → the `cli.<op>` wire params map.
fn params_of(sub: &ArgMatches, op: &super::OpSpec, top: &ArgMatches) -> Map<String, Value> {
    let mut params = Map::new();
    if let Some(root) = top
        .get_one::<String>("root")
        .or_else(|| sub.get_one("root"))
    {
        params.insert("root".to_string(), Value::from(root.clone()));
    }
    for param in &op.params {
        match param.kind {
            ParamKind::Arg | ParamKind::Opt => {
                if let Some(v) = sub.get_one::<String>(&param.name) {
                    // bare `--name` lands as "true" — bools read as
                    // bools so `bool_param` consumers still work.
                    let value = match v.as_str() {
                        "true" => Value::Bool(true),
                        "false" => Value::Bool(false),
                        _ => Value::from(v.clone()),
                    };
                    if param.secret {
                        // Name only — the value stays out of stderr too.
                        eprintln!(
                            "warning: `{}` is secret-bearing — prefer `--params-stdin` to keep it out of argv/shell history",
                            param.name
                        );
                    }
                    params.insert(param.name.clone(), value);
                }
            }
            ParamKind::Flag => {
                if sub.get_flag(&param.name) {
                    if param.secret {
                        eprintln!(
                            "warning: `{}` is secret-bearing — prefer `--params-stdin`",
                            param.name
                        );
                    }
                    params.insert(param.name.clone(), Value::Bool(true));
                }
            }
        }
    }
    params
}

/// Outcome → stdout/stderr + the engine's own exit code.
fn print_outcome(outcome: &OpOutcome) -> i32 {
    let stdout = std::io::stdout();
    let stderr = std::io::stderr();
    let _ = stdout.lock().write_all(outcome.stdout.as_bytes());
    let _ = stdout.lock().flush();
    let _ = stderr.lock().write_all(outcome.stderr.as_bytes());
    let _ = stderr.lock().flush();
    outcome.code
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::surface::{OpSpec, ParamSpec};
    use serde_json::json;

    fn manifest() -> OpsManifest {
        OpsManifest::from_value(&json!({
            "engine": "fx",
            "ops": [
                {"query": "cli.find", "doc": "sym -> loc",
                 "params": [{"name": "symbol", "kind": "arg"},
                            {"name": "workspace", "kind": "flag"},
                            {"name": "config", "kind": "opt"},
                            {"name": "token", "kind": "opt", "secret": true}]},
                {"query": "cli.status", "doc": "drift"},
            ],
            "surfaces": ["watch"],
        }))
        .unwrap()
    }

    struct Spy {
        last: Option<(String, Map<String, Value>)>,
        surfaced: Option<(String, Vec<String>)>,
    }

    impl Surface for Spy {
        fn op(&mut self, op: &OpSpec, params: &Map<String, Value>) -> OpOutcome {
            self.last = Some((op.name.clone(), params.clone()));
            OpOutcome {
                code: 0,
                stdout: String::new(),
                stderr: String::new(),
            }
        }
        fn surface(&mut self, name: &str, args: &[String]) -> i32 {
            self.surfaced = Some((name.to_string(), args.to_vec()));
            0
        }
    }

    #[test]
    fn ops_parse_into_wire_params() {
        let m = manifest();
        let mut spy = Spy {
            last: None,
            surfaced: None,
        };
        let argv: Vec<String> = ["fx", "find", "main", "--workspace", "--root", "/tmp/r"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(run("fx", &m, &argv, &mut spy), 0);
        let (name, params) = spy.last.unwrap();
        assert_eq!(name, "find");
        assert_eq!(params["symbol"], json!("main"));
        assert_eq!(params["workspace"], json!(true));
        assert_eq!(params["root"], json!("/tmp/r"));
        assert!(params.get("config").is_none());
    }

    #[test]
    fn opt_accepts_value_and_bare_true() {
        let m = manifest();
        let mut spy = Spy {
            last: None,
            surfaced: None,
        };
        let argv: Vec<String> = ["fx", "find", "x", "--config", "fx.toml"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(run("fx", &m, &argv, &mut spy), 0);
        assert_eq!(spy.last.unwrap().1["config"], json!("fx.toml"));

        let mut spy = Spy {
            last: None,
            surfaced: None,
        };
        let argv: Vec<String> = ["fx", "find", "x", "--config"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(run("fx", &m, &argv, &mut spy), 0);
        assert_eq!(spy.last.unwrap().1["config"], json!(true));
    }

    #[test]
    fn surfaces_stream_trailing_argv() {
        let m = manifest();
        let mut spy = Spy {
            last: None,
            surfaced: None,
        };
        // hyphen-leading trailing args pass through verbatim
        // (`--` itself is clap's escape token, not data).
        let argv: Vec<String> = ["fx", "watch", "--fast"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(run("fx", &m, &argv, &mut spy), 0);
        assert_eq!(
            spy.surfaced.unwrap(),
            ("watch".to_string(), vec!["--fast".to_string()])
        );
    }

    fn spy() -> Spy {
        Spy {
            last: None,
            surfaced: None,
        }
    }

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn params_stdin_merges_and_wins_per_key() {
        let m = manifest();
        let mut s = spy();
        // argv supplies symbol/config; stdin overrides both and adds
        // a non-argv-shaped value (a number) plus the secret param.
        let mut input: &[u8] =
            br#"{"symbol":"stdin-sym","config":"stdin.toml","token":"s3cr3t","retries":3}"#;
        let code = run_with_input(
            "fx",
            &m,
            &argv(&[
                "fx",
                "find",
                "argv-sym",
                "--config",
                "argv.toml",
                "--params-stdin",
            ]),
            &mut s,
            &mut input,
        );
        assert_eq!(code, 0);
        let params = s.last.unwrap().1;
        assert_eq!(params["symbol"], json!("stdin-sym")); // stdin wins over argv positional
        assert_eq!(params["config"], json!("stdin.toml")); // stdin wins over argv flag
        assert_eq!(params["token"], json!("s3cr3t"));
        assert_eq!(params["retries"], json!(3)); // any JSON value, not just strings
    }

    #[test]
    fn params_stdin_satisfies_required_arg() {
        let m = manifest();
        let mut s = spy();
        let mut input: &[u8] = br#"{"symbol":"only-stdin"}"#;
        let code = run_with_input(
            "fx",
            &m,
            &argv(&["fx", "find", "--params-stdin"]),
            &mut s,
            &mut input,
        );
        assert_eq!(code, 0);
        assert_eq!(s.last.unwrap().1["symbol"], json!("only-stdin"));
    }

    #[test]
    fn missing_required_arg_is_usage_error() {
        let m = manifest();
        let mut s = spy();
        // no positional, no --params-stdin → `symbol` never arrives
        let mut input: &[u8] = b"";
        let code = run_with_input("fx", &m, &argv(&["fx", "find"]), &mut s, &mut input);
        assert_eq!(code, 2);
        assert!(s.last.is_none());
    }

    #[test]
    fn params_stdin_rejects_bad_payloads() {
        let m = manifest();
        for payload in [&b"not json"[..], b"[1,2]", b"\"str\""] {
            let mut s = spy();
            let mut input: &[u8] = payload;
            let code = run_with_input(
                "fx",
                &m,
                &argv(&["fx", "find", "x", "--params-stdin"]),
                &mut s,
                &mut input,
            );
            assert_eq!(code, 2, "payload {payload:?} must fail");
            assert!(s.last.is_none());
        }
        // empty stdin is an empty map — argv params still dispatch
        let mut s = spy();
        let mut input: &[u8] = b"  \n";
        let code = run_with_input(
            "fx",
            &m,
            &argv(&["fx", "find", "x", "--params-stdin"]),
            &mut s,
            &mut input,
        );
        assert_eq!(code, 0);
        assert_eq!(s.last.unwrap().1["symbol"], json!("x"));
    }

    #[test]
    fn unknown_subcommand_is_clap_error() {
        let m = manifest();
        let mut spy = Spy {
            last: None,
            surfaced: None,
        };
        let argv: Vec<String> = ["fx", "bogus"].iter().map(|s| s.to_string()).collect();
        assert_eq!(run("fx", &m, &argv, &mut spy), 2);
    }

    #[test]
    fn spec_types_stay_constructible() {
        let op = OpSpec {
            name: "x".into(),
            doc: String::new(),
            params: vec![ParamSpec {
                name: "p".into(),
                kind: ParamKind::Flag,
                doc: None,
                secret: true,
            }],
            mutates: true,
        };
        assert!(serde_json::to_value(&op).unwrap()["mutates"]
            .as_bool()
            .unwrap());
    }
}
