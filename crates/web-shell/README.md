# web-shell

The shared web front end of Polylogue's workbench and Sinex's reader: one
visual language, one key table, and one small TypeScript shell, so that both
products look and behave alike and the operator's habits carry from one to the
other. Each product keeps its own views, routes and accent, so a page always
shows which product it belongs to.

## Rendering model

- **The daemon renders HTML** with maud, from the same typed answers the CLI
  and MCP render. A view is a Rust function over an answer; a live update is
  a fragment rendered by the same function, so a live row and a reloaded row
  cannot differ. The browser never assembles a view from JSON.
- **Pages are documents with controls**: ordinary links and GET forms; every
  write is a POST of an operation-table request (see
  [command-surface](../command-surface/README.md)). There is no web-only endpoint.
- **One TypeScript shell** carries the behaviour the server cannot: the key
  table and palette, the copy menu, the event-stream client and fragment
  swaps, follow-at-end, windowed lists, aborting superseded requests, and the
  pointer handling of timelines and boards. It is checked by `tsc --strict`
  and bundled by `esbuild`. The built `shell.js` is committed, and a check
  rebuilds it and fails on any difference, so neither product needs a
  TypeScript step to build. There is no npm, no framework and no third-party
  script at run time.
- **One content security policy** on every route:
  `default-src 'none'; script-src 'self'; style-src 'self'; font-src 'self'; img-src 'self' data:; connect-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'`.
  No inline script or style attribute, no `eval`, no WebAssembly. Fonts are
  served by the daemon; nothing is fetched from another origin.
- The TypeScript types for the few JSON values the shell reads are generated
  from the Rust types.

The crate provides the maud helpers both products use (the page frame, value-state
cells, copy-target attributes, the unavailable panel, event framing for
fragments), the security and asset headers, and a handler that serves
`tokens.css`, `shell.css`, `shell.js` and the fonts.

## Visual language

`tokens.css` holds the shared tokens: type, spacing, grounds and ink, and
every semantic colour and mark. Each product adds only its accent
(`--accent-polylogue`, `--accent-sinex`), used for links, the focus ring and
primary buttons; an accent never carries a state.

- **Type:** IBM Plex Sans for interface text; IBM Plex Mono for data, IDs,
  commands, tool lines and citations.
- **Grounds:** `--ground` for the page, `--surface` for panels,
  `--surface-sunk` for nested blocks. Light and dark follow
  `prefers-color-scheme`; a theme setting overrides it.
- **The operator's palette:** when a palette file exists (pywal's, or one a
  setting names), grounds and ink derive from it while the semantic colours
  and contrast checks stay; a palette that fails the contrast checks falls
  back to the designed one, and the theme view says why. It is on by
  default.
- **Meaning is never colour alone.** Failure, unknown, partial, inferred and
  not-applicable each have one mark and one word. Missing values in charts use
  the `--missing-pattern` hatch, never a zero-height bar.
- **Value states:** unknown `—` or `unknown (reason)`; `not recorded`; `n/a`;
  partial `≥ X (N unknown)`; inferred `≈ X`; `0`; `pending`; `error`; stale as
  a banner. Unknown is never shown as zero.
- **Density:** comfortable (default), compact, dense.
- **Motion:** none except a live tick and progress, and none under
  `prefers-reduced-motion`.
- **Terminals** use the 16 ANSI colours only, so the terminal's theme applies.

## Page frame

```
┌ header: wordmark · navigation · query field ──────────────────────────────┐
├─ left rail (optional) ─┬─ main ─────────────────────┬─ inspector ─────────┤
│ outline, facets, toc   │ the view                    │ facts, provenance … │
├────────────────────────┴─────────────────────────────┴─────────────────────┤
│ status strip: daemon state · per-source lag · pending work · keys          │
└────────────────────────────────────────────────────────────────────────────┘
```

- The **header** holds the wordmark, the product's navigation and a persistent
  query field with completion.
- The **status strip** is the daemon state indicator (`live`,
  `catching up · N waiting`, `disconnected · retrying`, `stopped` with the
  command that starts it), followed by the product's figures; each item links
  to its place in status.
- The **main column** is at most 1,200 px wide; running text is 75 to 90
  characters. The **inspector** (280 to 320 px) shows the page's facts, or the
  focused target's, with a way back. A **left rail** appears only where it
  earns its width.
- **Narrow widths** (under 900 px): rails become disclosures above the main
  column and tables scroll inside their box. At 390 px every view is usable
  with no horizontal page scroll.
- **Footer:** every page shows the exact CLI command that reproduces it,
  computed from the same request arguments, with *Copy as CLI command*, and
  the result handle where the view is backed by a query.
- **States:** every view renders empty, loading, degraded and failure as a
  persistent line in the view, never a modal; the way out is a command the
  operator can run.
- **Actions** carry their availability from the operation: `enabled`,
  `disabled (reason)`, `partial (reason)`, `loading` or
  `unavailable (reason)`. A disabled action stays visible with its reason. A
  failed write keeps the editor's text.

## Keys

One key table, shown by `?`, with the same letter for the same act everywhere,
in both products and in each product's terminal picker. The shell owns these
keys; a product adds keys for its own acts and never gives a shared key
another meaning.

| Key | Act |
|---|---|
| `/` | focus the query field |
| `:` or `Ctrl-K` | palette |
| `?` | key help |
| `g` then a letter | go to a section (the letters are the product's navigation) |
| `j` / `k`, `↓` / `↑` | next / previous row, hit or item |
| `gg` / `G` | top / bottom |
| `Enter` | open the focused row |
| `x` | toggle selection |
| `*` | select all visible |
| `y` / `Y` | copy menu / copy the default form; `y c` copies the page's CLI command |
| `n` / `N` | next / previous match of the active find |
| `Esc` | close a disclosure, palette or editor; clear the selection |

Single-character keys can be switched off by a setting. Keys are ignored
inside inputs and while text is selected. Focus is real DOM focus, and a live
update never takes it. In a terminal picker, where letters type into the
query, the same acts use `Ctrl` with the same letter (`Ctrl-y` copies), and
arrows move.

## Palette

`:` or `Ctrl-K` lists every action available for the current selection,
generated from the operation table's rows and the view's navigation. Each
entry shows its key, its CLI form and, when disabled, the reason. Typing
filters with `matching::match_score`, also exposed for terminal pickers. The TypeScript
implementation runs the same shared matcher vectors as Rust. The palette runs
operations; it never holds an action the operation table lacks.

## Copy

Every target has a copy menu: a default form and the target's other forms
(the product lists them per target, such as a reference, Markdown, the CLI
command or a URL). Feedback appears beside the target and says what was
copied and how much ("message as Markdown copied · 1,247 characters"). A
denied clipboard shows the text selected in a box, never a false "Copied".
Bytes a browser cannot place on the clipboard (exact bytes, non-text types)
are offered through a product operation that writes them with the desktop's
clipboard service.

## Live updates

- One event stream per visible tab, framed as in
  [command-surface](../command-surface/README.md#server-sent-events). Payloads are
  `append` (a fragment for an open list that asked for it), `replace` (a
  fragment for a target) or `ref` (a changed reference whose fragments the
  shell refetches).
- A page declares what it follows (`data-feed`); the server filters by
  reference and refuses by name a subscription it cannot evaluate.
- **Updates never move the reader.** A reader appends only while following
  and scrolled to the end, and otherwise shows "N new ↓"; a list holds
  reordering while a row has focus and shows "N updates · apply"; selection
  and focus are never removed by an update.
- Hidden tabs close their stream and refresh on return.
- A newer request aborts the older one, and a stale reply never overwrites a
  newer selection.
- Long lists are windowed: the shell keeps a window of rows in the DOM and
  fetches by anchor when scrolling past it, keeping focus and position.

## What a product supplies

Its views and routes, its navigation and `g` letters, keys for its own acts,
its accent, its copy forms per target, its status-strip figures, and what each
page follows.

## Integration

`Frame::for_request(product, title, query_route, table, request)` supplies a
document frame whose CLI footer is generated by the operation table from that
request. `Frame::new` accepts an already generated command for aggregate pages.
Set navigation, optional rails, completion route, typed actions, status markup,
result handle, preferences, and live routes before calling `render(&answer,
&view)`. A `View<A>` is a function from the product's typed answer to maud
`Markup`. Reuse that function in `FragmentUpdate::append` or `replace`.
A replacement replaces the target element's **contents**; keep its stable ID
on the containing element, outside the view function. Captured text enters
maud as text, never `PreEscaped`.

`Action::from_table` derives the registered operation metadata and CLI command
from the table and exact request. `Action::from_operation` accepts an already
resolved command-surface `OperationSpec`, its
`Availability`, and the exact `RunnableRequest`. It rejects mismatched
operation names, foreign routes and attempts to replace shared keys. Read
actions are ordinary GET links whose fields come from request arguments.
Writes are ordinary POST forms with the request envelope in the hidden
`request` field. The shell enhances writes to JSON POSTs of that same
request. For selection-scoped actions, set `actions_route` to the existing
operation view returning action HTML. The shell sends selected row references
as the JSON-encoded `references` GET field before opening the palette. A row
may override that route with `data-actions-route`. Products apply their table's
selection semantics and return `Action::render` output. The original row is
held while the palette is open, and superseded action lists cannot replace
the current selection. Authentication, origin checks, grants and idempotent write identity
stay with command-surface and the product. An unavailable action shows its
reason. Editor contents remain until the product explicitly replaces them
after a successful operation.

Wrap every product response, including redirects and errors, in
`assets::secure`, or use `assets::document` for HTML. Route `/assets/*` to
`assets::serve` (`serve_with_theme` when applying an operator palette).
Assets support GET, HEAD, content types, cache validators and conditional
requests; unknown paths return 404 and other methods return 405. The policy
includes `font-src 'self'` because the default denial otherwise prevents
self-hosted fonts from loading. IBM Plex Sans and Mono are bundled with their
SIL Open Font License and pinned source revision.

`Theme::load(path, enabled)` reads the configured pywal-format palette, or
`$XDG_CACHE_HOME/wal/colors.json` (`$HOME/.cache/wal/colors.json` by default).
A missing default file uses the designed palette. Parse/read failures and
insufficient contrast return `fallback_reason` for the product's theme view.
Only six-digit RGB colours reach the generated stylesheet. Ink and each
product's accent must meet 4.5:1 against every ground. Semantic states keep
their designed ink and backing. Serve the resulting stylesheet through
`serve_with_theme`; the frame already links it under the same CSP.

### Browser attributes

The shell enhances these ordinary server-rendered controls:

| Attribute | Contract |
|---|---|
| `data-row`, `data-anchor` | keyboard item and stable window anchor; real DOM focus stays on the row |
| `aria-selected` | selection state; selection and focused descendants protect live rows |
| `data-copy` | `CopyTarget` JSON, emitted by `CopyTarget::render`; first form is the default |
| `data-follow="true"` | scroll container following at its end; `false` holds appends |
| `data-toggle-follow="ID"` | button controlling follow for that container |
| `data-find-match` | server-marked active query matches visited by `n` and `N` |
| `data-window="200"` | nominal number of rows retained in a scroll container; protected and visible rows are retained even above it |
| `data-window-before`, `data-window-after` | ordinary pagination links returning rows and the next link in that direction |
| `data-window-sentinel` | optional styling for a pagination link |
| `data-pointer="timeline"` | pan and pinch/Control-wheel zoom over `data-start` and `data-end` |
| `data-pointer="board"` | drag scroll over the board; the same range events support domain-specific zoom |
| `data-pointer-route` | optional existing GET view route returning HTML for the selected range |
| `data-editor` | editor closed by Escape without clearing its fields |

A window fetch adds `anchor` and `direction` to the existing pagination
route. Its response is HTML rows with stable anchors and a new directional
pagination link, or no link at the end. The opposite edge link is retained
or restored when rows are removed, so every row remains reachable. Removing
offscreen rows compensates the surviving anchor's offset, and never removes
a visible, selected or focused row. Pagination links remain usable without
JavaScript.

Pointer events dispatch `shell:pointer` with `{kind, start, end, deltaY,
phase}` (`preview` or `commit`). Products use the domain values for timeline
selection or board zoom. Each pointer uses its own movement delta; pinch
scaling compares the active touches before and after each move. The shell
handles capture, touch pinch and wheel
coordinates; products render domain geometry. An optional pointer route is
refetched only at commit, through the same superseding request controller.

### Live protocol

The frame's `feed` is an existing filtered subscription route. It emits SSE
`event: fragment` whose data is generated `FragmentUpdate` JSON, and whose ID
is the product's committed position. `append` and `replace` contain rendered
HTML; `ref` contains the same-origin URL of that rendered fragment. A `ref`
URL identifies the read position of its event,
so its HTML excludes later appends. The shell buffers appends behind an
unresolved reference and advances its resume cursor only after preceding
events have been applied or retained for explicit apply. A newer full
replacement or reference supersedes an older snapshot and its preceding
appends. Failed reference reads close the feed with a reload instruction and
leave the failed position unacknowledged. The shell never constructs a view
from a JSON answer. The shared `reload` event closes the
subscription and leaves a visible instruction to reload. Visibility changes
keep that feed closed until the page is reloaded. Its event name is
generated from command-surface alongside the shell types. Native EventSource
reconnect sends `Last-Event-ID`. A connection reopened after visibility
changes sends its position in the `since` query parameter; the product maps
both to the same shared resume mechanism.

Set `refresh` to the existing current-view operation returning `Refresh`:
`{position, fragments}` read under one snapshot. On return from a hidden tab,
that snapshot is applied before opening the stream strictly after its
position. There is one stream per visible tab, and hidden tabs abort all
requests. A late response to a superseded target is ignored even when its
transport ignores cancellation. A live update protects focus and selection;
held changes appear as an adjacent apply control. Replacement events
coalesce while held. An explicit apply still waits for protected rows to be
released. New appends require follow-at-end, or an explicit apply.

### Development

Products consume the committed `assets/shell.js` and need no JavaScript build
step. Maintainers need `node`, `tsc` and `esbuild` on PATH (the tools can also
be supplied through `TSC` and `ESBUILD`). No npm installation is used.

```sh
node crates/web-shell/build.mjs --test
node crates/web-shell/build.mjs --write --test
cargo run -p web-shell --bin web-shell-types -- --write crates/web-shell/typescript/types.ts
cargo xtask test -p web-shell
```

`keys::KEYS` is the one key declaration: Rust help and terminal
`picker_bindings()` read it directly, and `web-shell-types` generates the
`SHARED_KEYS` array used by browser dispatch. Modifiers and prefix sequences
are part of that declaration. Build through `nix develop` so the bundle uses
the pinned tools: TypeScript 7.0.2 and esbuild 0.27.2 in the current flake
lock. Another esbuild version can emit different bytes from the same source.

The default build checks strict TypeScript, rebuilds with esbuild, and compares
bytes with the committed artifact. `--write` updates it. `--test` executes the
request, clipboard, pointer, matching and fragment DOM tests under Node.
Rust tests compare generated TypeScript with its committed file, render a
shared typed answer through the full-page and live paths, exercise operation
forms, validate palettes, decode SSE with command-surface, and probe asset
and security responses. Products additionally test their actual routes and
layouts in a real browser against their daemon and visual baseline.

## Laws

- **same-key-same-act**: for every act in both the web key table and a
  terminal picker, the letter is the same, modulo the picker's `Ctrl` prefix.
- **csp-on-every-route**: every response carries the policy above, and no
  page contains an inline script or style attribute.
- **built-shell-matches-source**: rebuilding `shell.js` from its sources gives
  the committed file byte for byte.
- **live-fragment-equals-reload**: a fragment received live equals the same
  fragment in a reloaded page.
- **update-never-moves-reader**: no live update changes the scroll position,
  focus or selection of a reader that is not following.
- **stale-reply-never-overwrites**: a reply to a superseded request never
  replaces the result of a newer one.
- **denied-clipboard-is-honest**: when the clipboard is denied, the shell
  shows the text to copy and never reports success.
- **narrow-width-usable**: at 390 px no view scrolls horizontally.
- **unknown-never-zero**: no value state renders an unknown value as zero.
