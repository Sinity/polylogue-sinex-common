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
  [command-surface](command-surface.md)). There is no web-only endpoint.
- **One TypeScript shell** carries the behaviour the server cannot: the key
  table and palette, the copy menu, the event-stream client and fragment
  swaps, follow-at-end, windowed lists, aborting superseded requests, and the
  pointer handling of timelines and boards. It is checked by `tsc --strict`
  and bundled by `esbuild`. The built `shell.js` is committed, and a check
  rebuilds it and fails on any difference, so neither product needs a
  TypeScript step to build. There is no npm, no framework and no third-party
  script at run time.
- **One content security policy** on every route:
  `default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'`.
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
filters with the same matcher as the terminal picker. The palette runs
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
  [command-surface](command-surface.md#server-sent-events). Payloads are
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
