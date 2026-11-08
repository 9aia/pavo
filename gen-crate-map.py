#!/usr/bin/env python3
"""Generate the crate-map table (into explanation/architecture.md)
and reference/crate-api.md from workspace Cargo.tomls — never
hand-edit either."""
import os, pathlib, re

# Sources live in the main checkout — normally docs/ is a worktree of
# it (so `..` works); PAVO_ROOT overrides for detached runs.
ROOT = pathlib.Path(
    os.environ.get("PAVO_ROOT") or pathlib.Path(__file__).parent / "..")
WST = (ROOT / "Cargo.toml").read_text()

LAYER = {
    "pavo": "binary",
    "pavo-protocol": "domain",
    "pavo-core": "host",
    "pavo-testkit": "adapter",
    "pavo-driver-config": "adapter",
}

# workspace members — match `"crates/…"` paths only (not dep paths)
members = re.findall(r'"(crates/[^"]+)"', WST)
seen, order_dedup = set(), []
for m in members:
    if m not in seen:
        seen.add(m)
        order_dedup.append(m)

rows = []
for rel in order_dedup:
    t = (ROOT / rel / "Cargo.toml").read_text()
    name = re.search(r'^name = "([^"]+)"', t, re.M).group(1)
    m = re.search(r'^description = "([^"]+)"', t, re.M)
    desc = (m.group(1) if m else "").replace("<", "&lt;").replace(">", "&gt;")
    rows.append((name, LAYER.get(name, "adapter"), desc))

order = {"domain": 0, "host": 1, "adapter": 2, "binary": 3}
rows.sort(key=lambda r: (order[r[1]], r[0]))
print(f"crate map: {len(rows)} crates")

# 1. crate map table inside explanation/architecture.md
arch = pathlib.Path("explanation/architecture.md")
s = arch.read_text()
table = "| crate | layer | purpose |\n|---|---|---|\n" + "\n".join(
    f"| `{n}` | {l} | {d} |" for n, l, d in rows)
s = re.sub(
    r"<!-- crate-map -->\n(?:\|[^\n]*\n?)*",
    "<!-- crate-map -->\n" + table + "\n", s, count=1)
arch.write_text(s)

# 2. crate-api page — one rustdoc link per crate
out = ["# Crate API", "",
       "Rustdoc for every workspace crate — public APIs are",
       "documented at source (doc comments).", ""]
for n, l, _ in rows:
    und = n.replace("-", "_")
    out.append(f"- [{n}](/pavo/api/{und}/index.html) — {l}")
pathlib.Path("reference/crate-api.md").write_text("\n".join(out) + "\n")
print("crate-api regenerated")
