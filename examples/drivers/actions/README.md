# tamias-driver-actions

The interaction-actions demo driver — a self-contained Python 3 script
(stdlib only) that exercises the `invoke` wire op and the declarative
action layer end-to-end:

- **`DriverManifest.actions[]`** — global verbs covering all three
  `danger` levels (`none`, `confirm`, `type-target`) and every
  `ParamKind` (`string`, `text`, `number`, `integer`, `bool`,
  `enum`, `date`, `ref`, `string[]`) in one auto-form (`task.new`).
- **`NodeBase.actions[]`** — `board.export` scoped to the tasks table.
- **`TreeItem.actions[]`** — per-row `task.done` / `task.archive` /
  `task.delete` on every backlog tree item.
- **`contributes.menus[]`** — command-palette entries mixing `action`
  and `route` kinds, with `section`, `keywords`, and `priority`.
- **`{"op":"invoke",…}`** — mutations are in-memory for the life of
  the `serve` process and answered with a fresh `view` (the
  refresh-after-mutation contract); per-param validation failures
  come back in `ActionResult.errors`.

## Install

The executable must be named `tamias-driver-<type>` and discoverable
either on `PATH` or in `$TAMIAS_HOME/drivers/` (`$TAMIAS_HOME` or
`~/.local/share/tamias/drivers`):

```bash
# on PATH
export PATH="$PWD/examples/drivers/actions:$PATH"

# or into a tamias drivers dir
mkdir -p "${TAMIAS_HOME:-~/.local/share/tamias}/drivers"
ln -s "$PWD/examples/drivers/actions/tamias-driver-actions" \
      "${TAMIAS_HOME:-~/.local/share/tamias}/drivers/tamias-driver-actions"
```

Requires `python3` (3.8+; nothing beyond the stdlib).

## Views

| Deep link          | Route             | Shows                                        |
| ------------------ | ----------------- | -------------------------------------------- |
| `actions:board`    | `/actions/board`  | counts (kv) + tasks table (node action)      |
| `actions:backlog`  | `/actions/backlog`| expandable task `tree` with per-row actions  |

## Actions

Manifest-global (`invoke` with no `view`, or any view context):

| id            | label       | danger        | params / keys                     |
| ------------- | ----------- | ------------- | --------------------------------- |
| `refresh`     | Refresh     | none          | `r`                               |
| `task.new`    | New task    | none          | full ParamKind auto-form, `n`     |
| `task.done`   | Mark done   | none          | `target:"item"`, `d`              |
| `task.archive`| Archive     | confirm       | `target:"item"`, `a`              |
| `task.delete` | Delete      | type-target   | `target:"item"`, `D`, `reason`    |

Plus `board.export` on the board's table node (`format` enum param).

## Wire

```
{"op":"invoke","view":"backlog","action":"task.done",
 "target":{"id":"A-003"}}
→ {"ok":true,"result":{"message":"marked A-003 done",
    "data":{…},"view":{…fresh ViewResult…}}}

{"op":"invoke","action":"task.new","params":{"title":"ship it"}}
→ {"ok":true,"result":{"errors":{"assignee":"required"}}}
```

`target` is opaque JSON — the driver accepts a bare id string,
`{"id": …}`, or a row object carrying `id`. `task.archive` /
`task.delete` require the host's confirm/type-target UX; the wire
itself carries no confirmation — danger levels are host hints.

## Chrome

Contributes `navbar` (board + backlog), `statusbar`, and `menus`
(palette entries for the task actions plus navigate routes, grouped
under `Tasks`/`Navigate` sections).

## Queries

`tasks`, `members` (picker data for `ref` params), `items` (tree
shape), `counts`.
