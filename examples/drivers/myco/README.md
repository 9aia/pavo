# tamias-driver-myco

A **thin reference driver** — the pattern every real product driver
follows. The driver owns its pavo manifest and ViewNode trees
(discovery never needs the product running) and translates `view` /
`query` ops onto the product's own `serve` wire.

> The production version of this driver lives in the **myco** repo.
> This example exists to prove the proxy pattern end-to-end; keep it
> small and honest rather than feature-complete.

## Install

```bash
export PATH="$PWD/examples/drivers/myco:$PATH"
# or:
ln -s "$PWD/examples/drivers/myco/tamias-driver-myco" \
      "${TAMIAS_HOME:-~/.local/share/tamias}/drivers/tamias-driver-myco"
```

Requires `python3` (stdlib only) and, for live data, `myco` on `PATH`.

## How the proxy works

- `manifest`, `views`, and the `manifest` capability probe are answered
  **locally** — pavo can discover the driver with myco absent.
- `view`/`query` ops resolve each declared query binding through one
  persistent `myco serve` child speaking **JSON-RPC 2.0** (myco's wire
  is *not* pavo's `{"op":…}` envelope — translation is the driver's
  job):

  | pavo query     | myco method                          |
  | --------------- | ------------------------------------ |
  | `items`         | `todo.list`                          |
  | `board`         | `view` `{"name":"board"}`            |
  | `adrs`          | `adr.list`                           |
  | `status`        | `status`                             |

- myco discovers the repo from the cwd it inherits — **run `pavo`
  inside a myco-managed repo** and the views show that repo's ledger.
- Degraded mode: with no `myco` on `PATH`, `manifest`/`views` still
  answer and `view`/`query` return data payloads of
  `{"error": "myco not installed"}` — the ViewNode contract's partial
  degradation path, so views still render with error markers.

## Views

| Deep link      | Route        | Bound queries            |
| -------------- | ------------ | ------------------------ |
| `myco:board`   | `/myco/board`| `items` (table), `status` (kv) |
| `myco:items`   | `/myco/items`| `items`                  |
| `myco:adrs`    | `/myco/adrs` | `adrs`                   |

```bash
cd ~/GitHub/9aia/kuda        # a myco-managed repo
pavo view myco:items         # live ledger, rendered
pavo query myco status       # raw passthrough → myco `status`
```
