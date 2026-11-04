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
  table and palette, the copy menu, live fragments over one connection per
  browser, follow-at-end, windowed lists, aborting superseded requests, the
  client-drawn timeline, decision cards, previews and seen marks. Each
  behaviour is an autonomous custom element wrapping server-rendered markup
  (see [Elements](#elements)); keys, the palette and navigation are global
  controllers. Custom elements give each behaviour a lifecycle (connect,
  disconnect) that fragment swaps and lanes need, without a framework.
  The shell is checked by `tsc --strict` and bundled by `esbuild`. The built
  `shell.js` and `feed-worker.js` are committed, and a check rebuilds them
  and fails on any difference, so neither product needs a TypeScript step to
  build. There is no npm, no framework and no third-party script at run
  time.
- **Every view works without script.** The markup inside each element is the
  working document: rows, links, tables and forms. Every write is a form that
  works without the shell, which turns it into a JSON POST of the same
  request. The shell only enhances, so it never accumulates view logic, and
  the share file, `curl`, an agent reading HTML and a broken shell all keep
  working.
- **One content security policy** on every route:
  `default-src 'none'; script-src 'self'; style-src 'self'; font-src 'self'; img-src 'self' data:; connect-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'`.
  No inline script or style attribute, no `eval`, no WebAssembly. Fonts are
  served by the daemon; nothing is fetched from another origin. The feed
  worker is allowed by `script-src 'self'` (the `worker-src` fallback), and
  its own response carries the same policy.
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
- **Value states** render the one value-state vocabulary of
  [command-surface](../command-surface/README.md#answers): `complete` as the
  number (`0` only when measured zero); `partial` as `≥ X (N unknown)`;
  `inferred` as `≈ X`; `unknown` as `—` or `unknown (reason)`;
  `not_recorded` as `not recorded`; `not_applicable` as `n/a`; `pending` as
  `pending`. A failure shows `error`, and a stale view a banner. Unknown is
  never shown as zero.
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
  command that starts it), followed by the product's status measurements;
  each item links to its place in status.
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

| Key | Act | Where |
|---|---|---|
| `/` | focus the query field | everywhere |
| `:` or `Ctrl-K` | palette | everywhere |
| `?` | key help | everywhere |
| `g` then a letter | go to a section (the letters are the product's navigation) | everywhere |
| `g n` | go to the notices list | everywhere, reserved in both products |
| `g o` | go to this view in the other product, or say why there is none | everywhere, reserved in both products |
| `j` / `k`, `↓` / `↑` | next / previous row, hit, item or timeline shape | lists, timelines |
| `gg` / `G` | top / bottom | lists, timelines |
| `Enter` | open the focused row or shape; on a row holding a card, enter the card | everywhere |
| `x` | toggle selection | lists |
| `*` | select all visible | lists |
| `y` / `Y` | copy menu / copy the default form; `y c` copies the page's CLI command | everywhere |
| `n` / `N` | next / previous match of the active find | lists, readers |
| `←` / `→` | pan earlier / later | timelines |
| `-` / `=` | zoom out / in | timelines |
| `Esc` | leave a card; close a disclosure, palette or editor; clear the selection | everywhere |

`keys::KEYS` declares the table, each binding with its place. Timeline keys act
only while focus is inside a timeline; elsewhere the browser keeps them.

**Layers, not modes.** Global keys work everywhere. A focused decision card
(see [Cards](#cards)) is a layer: `Enter` enters it and `Esc` leaves it, and
while it is entered the keys shown on its buttons come before the shared
table. No keystroke made while moving through a list reaches a card's
actions.

**A product's own keys** are declared as `keys::ProductKey` values (key,
prefix, act, picker form). `keys::check_product` refuses a product key that
takes a shared key (`g n` and `g o` included), a key bound to two acts, and a
picker form that is not the key's `Ctrl` form. `keys::check_between` compares
the two products' tables: an act both products have takes the same keys, and
a key both bind means the same act. Navigation letters name each product's
own sections and are compared only against the shared table.

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

- **One event connection per browser.** Browsers allow about six HTTP/1.1
  connections per host and an event stream holds one for its whole life, so
  streams per tab, lane and preview would exhaust the pool. The shell runs a
  feed hub in a SharedWorker: one connection per product per browser
  profile, carrying the union of every tab's subscriptions. Where
  SharedWorker is unavailable, each tab runs the same hub itself and holds one
  stream.
- Events are framed as in
  [command-surface](../command-surface/README.md#server-sent-events). Payloads are
  `append` (a fragment for an open list that asked for it), `replace` (a
  fragment for a target) or `ref` (a changed reference whose fragments the
  shell refetches).
- A page declares what it follows with `<shell-feed>` elements (a
  reference, a saved query or a product stream such as `waiting`); a lane or
  fragment that brings its own feed subscribes on insertion and unsubscribes
  on removal. The server filters by reference and refuses by name a
  subscription it cannot evaluate.
- **Updates never move the reader.** A reader appends only while following
  and scrolled to the end, and otherwise shows "N new ↓"; a list holds
  reordering while a row has focus and shows "N updates · apply"; selection
  and focus are never removed by an update.
- Hidden tabs unsubscribe and refresh on return.
- A newer request aborts the older one, and a stale reply never overwrites a
  newer selection.
- Long lists are windowed: the shell keeps a window of rows in the DOM and
  fetches by anchor when scrolling past it, keeping focus and position.

## Elements

Each element wraps server-rendered markup that works on its own.

| Element | Without script | With the shell |
|---|---|---|
| `<shell-window data-size>` | a list with ordinary pagination links (`data-window-before`, `data-window-after`) | keeps about `data-size` rows in the DOM and fetches by anchor while scrolling |
| `<shell-feed>` | nothing | follows one subscription through the feed hub ([Live protocol](#live-protocol)) |
| `<shell-copy data-copy>` | the target's text | the copy menu and feedback |
| `<shell-timeline>` | a table of the series and links that pan and zoom | the client-drawn timeline ([Timeline](#timeline)) |
| `<shell-board data-start data-end>` | the board's markup | drag scroll and pinch or Control-wheel zoom, as `shell:pointer` events |
| `<shell-card>` | forms | a key layer ([Cards](#cards)) |
| `<shell-seen>` | a "mark as seen" form | the seen-mark write, sent once when shown ([Seen marks](#seen-marks)) |
| `<shell-preview data-route>` | a link | a preview popover of the same view's preview part, fetched on hover or focus |
| `<shell-editor>` | a form | `Esc` closes it keeping its text; `Ctrl-Enter` submits it |

## Timeline

One timeline element serves both products. It is drawn in the client from a
typed series, because server-rendered images per zoom step cannot pan and zoom
continuously; it holds geometry and references, never record text. The rows,
previews and tables under a selection stay server-rendered.

- A **lane** is a named query whose rows carry a time or an interval, with a
  render and an optional grouping field (`timeline::Lane`). It is a timeline
  row; a grouped lane draws one row per group value.
- A **series row** is `(lane, group, start, shape)`, where the shape is an
  `interval` (end, label, agency, reference, URL), a `point` (label, agency,
  reference, URL) or a `count` (end, number of rows). Times are milliseconds
  since the epoch; agency is operator, agent or unknown. A state is an
  interval with a label.
- **Fitting.** `Series::fit` fits rows to a window and a drawn width. An
  interval or count at least `minimum_pixels` wide is kept; narrower rows of
  one lane and group that start in the same bin become one count row, and a
  narrow row alone in its bin is kept. Bins are the smallest power of two
  milliseconds at least `minimum_pixels` wide, aligned to the epoch, so a bin
  lies inside one bin of every coarser zoom level and panning never moves a
  bin boundary. The product serves its series from its own read model
  (Polylogue from its session buckets and item index, Sinex from its tiles)
  and fits them here.
- **Drawing.** The element fetches `data-series?start=&end=&width=` and draws
  lane labels, a time axis in `data-time-zone`, and the shapes on a canvas.
  Agency is never colour alone: operator spans are filled, agent spans
  outlined, unknown ones hatched. While a pan or zoom waits for its series,
  the previous series is drawn at the new scale, and the newer request
  supersedes the older one.
- **Use.** A drag selects a window (`shell:select`), and when
  `data-rows`/`data-rows-route` are set the rows under the timeline are
  refetched for it; Control-wheel or a pinch zooms about the pointer;
  horizontal or Shift-wheel pans. Keys follow the shared table. `Enter`
  opens a shape's URL; on a count row it zooms into the count's span, so
  semantic zoom needs no other control. Each window change dispatches
  `shell:window`.
- **Without script**, `Timeline::render` writes the series as a table whose
  rows link to their objects (a count links to this view over its span), and
  links that pan and zoom by half the window.
- Charts that do not zoom continuously stay server-rendered SVG, so each
  chart kind has one renderer.

## Cards

`Card::new(label, body, actions)` renders a `<shell-card>` holding a decision:
a permission, a judgment, a drafted turn to approve. Each `CardAction` pairs
one character with a write `Action`, and the character is shown on its
button. `Enter` on a row holding a card, or on the card, enters it; `Esc`, or
focus leaving the card, leaves it. Inside an entered card its keys come
before the shared table, so a card may rebind any letter visibly. Card keys
are unique within a card, and a card action is always a write without a list
key of its own.

## Seen marks

A [seen mark](../command-surface/README.md#seen-marks) records, per consumer
and target, the latest position displayed to that consumer; the products
store it with their authored state, and "N new since you last looked" is read
from it. The
product renders `Seen::new(action)` where the displayed content ends, holding
its seen-mark write for this target and position. The shell sends the write
once, the first time the element is displayed: the document is visible and
not prerendering, the element is in the viewport and rendered, and it is not
inside a preview. Prerendering, hidden tabs, previews, polling and agent reads
of the HTML never send it. A fragment that replaces the element with a newer
position sends again when shown. A failed write shows the form again. Without
script, the element is a "mark as seen" form.

## What a product supplies

Its views and routes, its navigation and `g` letters, keys for its own acts
(checked with `keys::check_product` and against the other product with
`keys::check_between`), its accent, its copy forms per target, its
status-strip measurements, its notices route and counterpart link, its union
stream route and what each page follows, its timeline series and its
seen-mark write.

## Integration

`Frame::for_request(product, title, query_route, table, request)` supplies a
document frame whose CLI footer is generated by the operation table from that
request. `Frame::new` accepts an already generated command for aggregate pages.
Set navigation, optional rails, completion route, typed actions, status markup,
result handle, preferences, the page's `feeds`, the `notices` route and the
`counterpart` before calling `render(&answer, &view)`. `Counterpart::View`
links the other product's view of the same thing (`g o`); `Counterpart::Absent`
gives the reason there is none; a product running alone sets neither and
renders nothing of the other product. A `View<A>` is a function from the product's typed answer to maud
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
| `data-follow="true"` | scroll container following at its end; `false` holds appends |
| `data-toggle-follow="ID"` | button controlling follow for that container |
| `data-find-match` | server-marked active query matches visited by `n` and `N` |
| `data-window-before`, `data-window-after` | pagination links inside a `<shell-window>`, returning rows and the next link in that direction |
| `data-window-sentinel` | optional styling for a pagination link |
| `data-route` on `<shell-board>` | optional existing GET view route returning HTML for the committed range |
| `data-notices`, `data-counterpart` | written by the frame; followed by `g n` and `g o` |

A window fetch adds `anchor` and `direction` to the existing pagination
route. Its response is HTML rows with stable anchors and a new directional
pagination link, or no link at the end. The opposite edge link is retained
or restored when rows are removed, so every row remains reachable. Removing
offscreen rows compensates the surviving anchor's offset, and never removes
a visible, selected or focused row. Pagination links remain usable without
JavaScript.

A board dispatches `shell:pointer` with `{kind, start, end, deltaY, phase}`
(`preview` or `commit`). Products use the domain values for board zoom. Each
pointer uses its own movement delta; pinch scaling compares the active
touches before and after each move. An optional route is refetched only at
commit, through the same superseding request controller.

### Live protocol

The product serves one **union stream** route. Its `subscriptions` query
parameter is JSON, a list of `feed::Subscription { id, reference, since }`
read with `feed::subscriptions`; `since` is the event ID the subscription
resumes strictly after. IDs are assigned by the hub and unique in the
stream. `feed::catch_up(log, subscriptions, updates)` writes the opening of
the stream: for each subscription, the retained events after its own position
that concern it, in commit order, or a `reload` naming it when its position
is no longer retained. `feed::live(event, subscriptions, updates)` writes one
newly committed event for every subscription. `updates` is the product's
filter, the fragments one committed value gives one subscription. A
subscription the product cannot evaluate is answered with
`FeedReload::refuse(subscription, reason)`. The product subscribes to its
commit broadcast before reading the log, so nothing falls between the two.

Each SSE `event: fragment` carries generated `FeedEvent { subscription,
update }` JSON, and its ID is the product's committed position. `append` and
`replace` contain rendered HTML; `ref` contains the same-origin URL of that
rendered fragment. A `ref` URL identifies the read position of its event, so
its HTML excludes later appends. The shared `reload` event carries
`FeedReload { subscription?, reason }`: without a subscription it ends the
whole stream. The shell never constructs a view from a JSON answer.

`<shell-feed>` elements are rendered by `Feed { route, reference, since,
refresh }`, with `since` the position the page was rendered at. The hub
(`feed-worker.js` in a SharedWorker, or in the tab where SharedWorker is
unavailable) keeps one stream per route open for every subscription of every
tab. Subscription changes within a few milliseconds share one reopening of
the stream, and each subscription resumes from the last position the hub
delivered to its tab, so a reopening neither repeats nor skips an event. A
failed connection reopens the same way after a growing delay (one second,
doubling to thirty), instead of the browser's own reconnection, which would
resend the positions the stream was opened with. Tabs leave the hub on
`pagehide`.

In the tab, the shell buffers appends behind an unresolved reference and
advances a subscription's position only after preceding events have been
applied or retained for explicit apply. A newer full replacement or reference
supersedes an older snapshot and its preceding appends. A failed reference
read stops that subscription with a reload instruction and leaves the failed
position unacknowledged. A reload stops only the subscription it names.

`refresh` on a feed is the existing current-view operation returning
`Refresh`: `{position, fragments}` read under one snapshot. When a hidden tab
is shown again, that snapshot is applied before the feed subscribes strictly
after its position; a feed without `refresh` resumes from its last applied
position. Hidden tabs unsubscribe and abort all requests. A late response to
a superseded target is ignored even when its transport ignores cancellation.
A live update protects focus and selection; held changes appear as an
adjacent apply control. Replacement events coalesce while held. An explicit
apply still waits for protected rows to be released. New appends require
follow-at-end, or an explicit apply.

### Development

Products consume the committed `assets/shell.js` and `assets/feed-worker.js`
and need no JavaScript build step. Maintainers need `node`, `tsc` and `esbuild` on PATH (the tools can also
be supplied through `TSC` and `ESBUILD`). No npm installation is used.

```sh
node crates/web-shell/build.mjs --test
node crates/web-shell/build.mjs --write --test
cargo run -p web-shell --bin web-shell-types -- --write crates/web-shell/typescript/types.ts
cargo xtask test -p web-shell
WEB_SHELL_WRITE_FIXTURE=1 cargo xtask test -p web-shell -E 'test(timeline_zoom_fixture)'
```

`keys::KEYS` is the one key declaration: Rust help and terminal
`picker_bindings()` read it directly, and `web-shell-types` generates the
`SHARED_KEYS` array used by browser dispatch. Modifiers and prefix sequences
are part of that declaration. Build through `nix develop` so the bundle uses
the pinned tools: TypeScript 7.0.2 and esbuild 0.27.2 in the current flake
lock. Another esbuild version can emit different bytes from the same source.

The default build checks strict TypeScript, rebuilds with esbuild, and compares
bytes with the committed artifacts. `--write` updates them. `--test` executes
every `tests/*.test.mjs` under Node: requests, clipboard, pointers, matching,
fragment DOM handling, keys and cards, the feed hub, the display rule and the
timeline geometry over the Rust-fitted zoom fixture `tests/timeline-zoom.json`.
Rust tests compare generated TypeScript with its committed file, render a
shared typed answer through the full-page and live paths, exercise operation
forms, validate palettes, decode SSE with command-surface, and probe asset
and security responses. Products additionally test their actual routes and
layouts in a real browser against their daemon and visual baseline.

## Laws

- **same-key-same-act**: for every act in both the web key table and a
  terminal picker, the letter is the same, modulo the picker's `Ctrl` prefix;
  a product key never takes a shared key or `g n`/`g o`, a key means one act
  in a product, and an act both products have takes the same keys in both.
- **card-keys-need-an-entered-card**: no keystroke made while moving through a
  list reaches a card's action; a card's keys act only between `Enter` and
  `Esc`.
- **one-connection-per-browser**: any number of tabs of one browser profile,
  with any number of subscriptions, hold one event connection per product
  route.
- **display-acknowledges-only-when-shown**: a seen-mark write is sent only
  when its element is displayed; prerendered, hidden, offscreen, unrendered
  and previewed views send none, and one showing sends one write.
- **timeline-fit-conserves-rows**: the counts of a fitted series add up to the
  rows that touch its window, and at most one shape narrower than the minimum
  is drawn per bin, lane and group.
- **timeline-zoom-refines**: a row drawn as itself at one zoom level is drawn
  as itself at every finer level whose window holds it, and panning by whole
  bins leaves the shapes of the shared span unchanged.
- **works-without-script**: every element wraps markup whose rows, links,
  tables and forms work with JavaScript disabled.
- **csp-on-every-route**: every response carries the policy above, and no
  page contains an inline script or style attribute.
- **built-shell-matches-source**: rebuilding `shell.js` and `feed-worker.js`
  from their sources gives the committed files byte for byte.
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
