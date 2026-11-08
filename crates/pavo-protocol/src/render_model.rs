//! RenderModel — the canonical intermediate render output a
//! view-kind renderer plugin emits for the `render` wire op.
//!
//! Renderer plugins (drivers declaring `renderers` in their manifest)
//! don't ship surface code; they answer `{"op":"render", …}` with one
//! of these tagged models and each surface maps it to native widgets:
//!
//! | variant      | CLI            | TUI             | web               |
//! |--------------|----------------|-----------------|-------------------|
//! | `rows`       | aligned table  | table rows      | `<table>`         |
//! | `svg`        | raw markup     | `[svg]` hint    | `v-html` island   |
//! | `text`       | lines          | lines           | `<pre>`           |
//! | `placeholder`| `[component]`  | box + detail    | named component   |
//!
//! Wire shape (internally tagged on `"type"`):
//!
//! ```json
//! {"type":"rows","headers":["id","label"],"aligns":["left","left"],
//!  "rows":[["a","Alpha"]]}
//! ```
//!
//! The set is deliberately small — plugins needing richer output use
//! `placeholder` (web components) or `svg` (self-contained markup).
//! Unknown `type` values must degrade gracefully on every surface
//! (hosts treat them like `placeholder`).

use serde::{Deserialize, Serialize};

use crate::view_node::Align;

/// One rendered node's canonical output — see the module docs for the
/// per-surface mapping contract.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum RenderModel {
    /// Tabular output: headers, per-column alignment hints, and
    /// stringified cell rows. The most portable model — every surface
    /// has a table story.
    Rows {
        /// Column headings, in display order.
        headers: Vec<String>,
        /// Per-column alignment hints (parallel to `headers`); renderers
        /// default missing/extra entries to left.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        aligns: Vec<Align>,
        /// Body rows — each a list of stringified cells aligned to
        /// `headers` by index.
        #[serde(default)]
        rows: Vec<Vec<String>>,
    },
    /// Self-contained SVG markup — the plugin owns layout; surfaces
    /// embed it verbatim (web) or degrade to a hint (TUI).
    Svg {
        /// The `<svg>…</svg>` document fragment.
        markup: String,
    },
    /// Plain-text lines — the universal degradation target.
    Text {
        /// Lines to render top-to-bottom (no trailing newline needed).
        lines: Vec<String>,
    },
    /// A named rich component the surface may resolve itself — the
    /// plugin's escape hatch for visuals no built-in model covers.
    /// Surfaces that can't resolve `component` show `props` as KV.
    Placeholder {
        /// Component/renderer name (web component tag, island id, …).
        component: String,
        /// Props passed verbatim to the component.
        #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
        props: serde_json::Map<String, serde_json::Value>,
    },
}

impl RenderModel {
    /// Convenience constructor for [`RenderModel::Text`].
    #[must_use]
    pub fn text(lines: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self::Text {
            lines: lines.into_iter().map(Into::into).collect(),
        }
    }

    /// Convenience constructor for [`RenderModel::Rows`].
    #[must_use]
    pub fn rows(headers: Vec<String>, rows: Vec<Vec<String>>) -> Self {
        Self::Rows {
            headers,
            aligns: Vec::new(),
            rows,
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use serde_json::json;

    #[test]
    fn rows_roundtrip() {
        let m = RenderModel::Rows {
            headers: vec!["id".into(), "label".into()],
            aligns: vec![Align::Left, Align::Right],
            rows: vec![vec!["a".into(), "Alpha".into()]],
        };
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["type"], "rows");
        assert_eq!(v["headers"], json!(["id", "label"]));
        assert_eq!(v["aligns"], json!(["left", "right"]));
        let back: RenderModel = serde_json::from_value(v).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn all_variants_tag_and_roundtrip() {
        for m in [
            RenderModel::Svg {
                markup: "<svg/>".into(),
            },
            RenderModel::text(["a", "b"]),
            RenderModel::Placeholder {
                component: "viz-gantt".into(),
                props: Default::default(),
            },
        ] {
            let v = serde_json::to_value(&m).unwrap();
            let ty = v["type"].as_str().unwrap().to_string();
            let back: RenderModel = serde_json::from_value(v).unwrap();
            assert_eq!(back, m, "{ty}");
        }
        assert_eq!(
            serde_json::to_value(RenderModel::text(["x"])).unwrap()["type"],
            "text"
        );
        assert_eq!(
            serde_json::to_value(RenderModel::Placeholder {
                component: "c".into(),
                props: Default::default()
            })
            .unwrap()["type"],
            "placeholder"
        );
    }

    #[test]
    fn rows_defaults_for_absent_fields() {
        // Forward-compat: a plugin omitting aligns/rows still parses.
        let m: RenderModel =
            serde_json::from_value(json!({"type":"rows","headers":["h"]})).unwrap();
        match m {
            RenderModel::Rows { headers, rows, .. } => {
                assert_eq!(headers, vec!["h"]);
                assert!(rows.is_empty());
            }
            other => panic!("expected rows, got {other:?}"),
        }
    }
}
