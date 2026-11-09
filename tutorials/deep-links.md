---
url: /pavo/tutorials/deep-links.md
---
# Deep links

Every view has a stable `<driver>:<view>` ref. On the CLI it is an
argument; on the web it is a URL — the same link agents and humans
can drop into PRs, issues, and handoffs.

## The ref

```
pavo view demo:board
demo:board  →  route /demo/board  →  ViewSpec { id: "board" }
```

`pavo views` lists every ref; `pavo view --help` covers the CLI
flags (`--param`, `--format`, `--json`).

## Params in URLs

Views take params (`--param k=v` on the CLI). In URLs they can be
carried three ways — all normalize to the same `param.<k>=<v>`
pairs the bridge API understands:

```
/view/myco:item/id=T-001/verbose        path segments: one k=v each
/view/myco:item?id=T-001&verbose        bare query keys (no prefix)
/view/myco:item?param.id=T-001          legacy param.* form (still works)
```

Grammar details (parsed by `web/app/utils/view-link.ts`; the
no-build `/view/` stub in `pavo serve` speaks the same rules):

* The **first** path segment after `/view/` is the `<driver>:<view>`
  ref (the colon may be percent-encoded). Every later segment is one
  param: `key=value`, split at the **first** `=` (values may contain
  `=`); a bare `key` means an empty-string value; empty keys are
  dropped. So `/view/myco:item/T-001` sets param `T-001` to `""` —
  write `/view/myco:item/id=T-001` for a real param.
* Query keys are params too: bare `?k=v`, plus the legacy
  `?param.k=v` (prefix stripped).
* Precedence, lowest → highest: **path segments < bare query keys <
  `param.*` keys** — the bridge merges last-wins, so later sources
  override earlier ones on a key collision.
* Values parse as JSON when possible (`n=42`, `ok=true`, `j={…}`),
  else strings — identical to `--param` / `/api/view?param.*`.

## Serving the links

`pavo serve` prints `listening http://127.0.0.1:<port>`; open the
printed URL for the dashboard, or append a deep link directly:

```
http://127.0.0.1:7420/view/demo%3Aboard
http://127.0.0.1:7420/view/demo:board/status=open
```

With `cd web && npm run build` the bridge serves the Nuxt app;
without a build it serves a built-in landing page that still
understands `/view/` links.

## CLI equivalents

```sh
pavo view demo:board --param status=open
pavo open demo:deps          # TUI deep link (falls back to text off-TTY)
curl 'localhost:7420/api/view?ref=demo:board&param.status=open'
```

The `param.*` query convention is what the `/api/view` endpoint
already speaks — URL path segments and bare query keys are
normalized into it before the bridge sees them.
