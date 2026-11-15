# Command-Line Help for `pavo`

This document contains the help content for the `pavo` command-line program.

**Command Overview:**

* [`pavo`↴](#pavo)
* [`pavo drivers`↴](#pavo-drivers)
* [`pavo views`↴](#pavo-views)
* [`pavo chrome`↴](#pavo-chrome)
* [`pavo view`↴](#pavo-view)
* [`pavo query`↴](#pavo-query)
* [`pavo submit`↴](#pavo-submit)
* [`pavo actions`↴](#pavo-actions)
* [`pavo invoke`↴](#pavo-invoke)
* [`pavo jobs`↴](#pavo-jobs)
* [`pavo notifications`↴](#pavo-notifications)
* [`pavo serve`↴](#pavo-serve)
* [`pavo open`↴](#pavo-open)
* [`pavo manifest`↴](#pavo-manifest)
* [`pavo schema`↴](#pavo-schema)
* [`pavo gen-cli-docs`↴](#pavo-gen-cli-docs)

## `pavo`

Visualization host for the 9aia tool suite

**Usage:** `pavo [COMMAND]`

###### **Subcommands:**

* `drivers` — List discovered pavo-capable drivers
* `views` — List every view as `<driver>:<view>` refs
* `chrome` — Show merged chrome contributions (navbar/dashboard/statusbar/ routes/sidebar/headerActions/notifications/jobs + extras)
* `view` — Resolve + render a view — `pavo view myco:board` or a `/myco/item?id=T-1` route
* `query` — Raw `query` op passthrough against a driver's serve
* `submit` — Submit a form's field values — `pavo submit config:edit --param key=myco.theme --param value=dark`
* `actions` — List the actions a target offers — declared `actions[]` (manifest, view-node, tree-item scopes) plus ops-manifest derived actions, with danger level + param schemas
* `invoke` — Invoke an action — `pavo invoke myco:board refresh`. Dispatches the `invoke` wire op; `--json` dumps the full ActionResult
* `jobs` — List the aggregated job feed — every `contributes.jobs` provider driver's `jobs` wire op, tagged with its driver
* `notifications` — List the aggregated notification feed — every `contributes.notifications` provider driver's `notifications` wire op, tagged with its driver
* `serve` — Serve the web bridge — localhost HTTP API + static Nuxt host
* `open` — Open a view in the TUI (`pavo-tui`) — the interactive surface. Falls back to `view` text output when stdout is not a TTY
* `manifest` — Print the pavo capability manifest (JSON)
* `schema` — Print the CLI verb schema (JSON)
* `gen-cli-docs` — Print the CLI reference as markdown — the docs pipeline (`docs:cli`) renders `docs/reference/cli.md` from this



## `pavo drivers`

List discovered pavo-capable drivers

**Usage:** `pavo drivers [OPTIONS]`

###### **Options:**

* `--json` — JSON output



## `pavo views`

List every view as `<driver>:<view>` refs

**Usage:** `pavo views [OPTIONS]`

###### **Options:**

* `--driver <DRIVER>` — Only views of this driver
* `--json` — JSON output



## `pavo chrome`

Show merged chrome contributions (navbar/dashboard/statusbar/ routes/sidebar/headerActions/notifications/jobs + extras)

**Usage:** `pavo chrome [OPTIONS]`

###### **Options:**

* `--json` — JSON output



## `pavo view`

Resolve + render a view — `pavo view myco:board` or a `/myco/item?id=T-1` route

**Usage:** `pavo view [OPTIONS] <TARGET>`

###### **Arguments:**

* `<TARGET>` — `<driver>:<view>` deep link (bare `<driver>` = its first view) or a `/route?query` matching a declared view route / `item_route` template

###### **Options:**

* `--param <K=V>` — View param, `key=value` (repeatable). Values parse as JSON when possible, else strings. A view's `filter_param` (ViewSpec, convention `q`) is just another `--param`
* `--format <FORMAT>` — Output format — `text` (default), `json`, `dot`, `mermaid`, or any format a renderer plugin claims (e.g. `svg`, `ascii`; see a driver's manifest `renderers`)

  Default value: `text`



## `pavo query`

Raw `query` op passthrough against a driver's serve

**Usage:** `pavo query [OPTIONS] <DRIVER> <QUERY>`

###### **Arguments:**

* `<DRIVER>` — Driver kind
* `<QUERY>` — Query/binding name (or raw product op for thin drivers)

###### **Options:**

* `--param <K=V>` — Query param, `key=value` (repeatable)
* `--json` — JSON output



## `pavo submit`

Submit a form's field values — `pavo submit config:edit --param key=myco.theme --param value=dark`

**Usage:** `pavo submit [OPTIONS] <TARGET>`

###### **Arguments:**

* `<TARGET>` — `<driver>:<view>` deep link the form lives in (bare `<driver>` = its first view)

###### **Options:**

* `--form <FORM>` — Form node id when the view carries several forms (the `id` of the `form` ViewNode)
* `--param <K=V>` — Field value, `key=value` (repeatable) — the form's field keys. Values parse as JSON when possible, else strings
* `--json` — JSON output — dumps the full SubmitResult



## `pavo actions`

List the actions a target offers — declared `actions[]` (manifest, view-node, tree-item scopes) plus ops-manifest derived actions, with danger level + param schemas

**Usage:** `pavo actions [OPTIONS] <TARGET>`

###### **Arguments:**

* `<TARGET>` — `<driver>:<view>` deep link, or bare `<driver>` for manifest-scope actions only

###### **Options:**

* `--param <K=V>` — View param, `key=value` (repeatable) — used when resolving the view to collect node/item actions
* `--json` — JSON output



## `pavo invoke`

Invoke an action — `pavo invoke myco:board refresh`. Dispatches the `invoke` wire op; `--json` dumps the full ActionResult

**Usage:** `pavo invoke [OPTIONS] <TARGET> <ACTION>`

###### **Arguments:**

* `<TARGET>` — `<driver>:<view>` deep link the action lives in (bare `<driver>` = its first view)
* `<ACTION>` — Action id — see `pavo actions <driver>:<view>`

###### **Options:**

* `--param <K=V>` — Action param, `key=value` (repeatable) — the action's `ParamSchema` names. Values parse as JSON when possible, else strings
* `--target <JSON-OR-STRING>` — Opaque action target — JSON (`{"id":"T-1"}`) or a bare string. What the action's `target` context calls for (selected row, item id, …); the driver interprets it
* `--target-id <ID>` — Shorthand for `--target <id>` as a plain string — also the `type-target` confirmation: the id *is* the typed confirmation, so `--yes --target-id T-1` is the non-interactive spelling
* `--yes` — Skip confirmation prompts. `confirm` actions require it off-TTY; `type-target` actions still need `--target-id` (the id is the confirmation — `--yes` only skips the interactive "type it" prompt)
* `--interactive` — Prompt on a TTY for each action param not given via `--param` (uses the ParamSchema: enum lists options, bool y/n, defaults on empty input). Errors off-TTY
* `--json` — JSON output — dumps the full ActionResult



## `pavo jobs`

List the aggregated job feed — every `contributes.jobs` provider driver's `jobs` wire op, tagged with its driver

**Usage:** `pavo jobs [OPTIONS]`

###### **Options:**

* `--json` — JSON output



## `pavo notifications`

List the aggregated notification feed — every `contributes.notifications` provider driver's `notifications` wire op, tagged with its driver

**Usage:** `pavo notifications [OPTIONS]`

###### **Options:**

* `--json` — JSON output



## `pavo serve`

Serve the web bridge — localhost HTTP API + static Nuxt host

**Usage:** `pavo serve [OPTIONS]`

###### **Options:**

* `--port <PORT>` — Port to bind on 127.0.0.1 (0 = pick a free port; the actual port is printed as the first stdout line)

  Default value: `0`



## `pavo open`

Open a view in the TUI (`pavo-tui`) — the interactive surface. Falls back to `view` text output when stdout is not a TTY

**Usage:** `pavo open [TARGET]`

###### **Arguments:**

* `<TARGET>` — `<driver>:<view>` deep link (bare `<driver>` = its first view) or a `/route?query`. Omit to open the TUI dashboard



## `pavo manifest`

Print the pavo capability manifest (JSON)

**Usage:** `pavo manifest`



## `pavo schema`

Print the CLI verb schema (JSON)

**Usage:** `pavo schema`



## `pavo gen-cli-docs`

Print the CLI reference as markdown — the docs pipeline (`docs:cli`) renders `docs/reference/cli.md` from this

**Usage:** `pavo gen-cli-docs`



<hr/>

<small><i>
    This document was generated automatically by
    <a href="https://crates.io/crates/clap-markdown"><code>clap-markdown</code></a>.
</i></small>
