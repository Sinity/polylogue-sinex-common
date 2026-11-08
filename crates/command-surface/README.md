# command-surface

How an operation travels between a caller and a product daemon, and what
comes back. Each product declares one typed catalog with the actual CLI,
HTTP and MCP exposure of every operation. Transport adapters share that
catalog and the same authenticated execution handlers. This
crate holds the mechanics both products share: the `Operation` trait, the
answer contract every surface of both products uses, the `Failure` type with
its exit and HTTP table, who the caller is and who authored a write, the
seen mark, latency classes and deadlines, server-sent-event framing, and the
status format of measurements and a verdict.

## Operations

An operation is one row of a product's table. Its `OperationSpec` declares:

- its name and command words; an empty command means no CLI surface;
- its **kind**: a **request** the daemon answers once, a **stream** of events
  the daemon sends until it ends or the caller leaves (a change feed,
  `follow`), or a **local** row the calling process runs without the daemon
  (help, installers, the MCP server and hook entry points);
- for a request, its optional MCP tool family; `None` means no MCP exposure;
- its optional HTTP route (always absent for a local row);
- its latency class, declared failure kinds, access level and default token
  budget;
- how each request field is written on the command line;
- whether help lists it.

The catalog keeps every request and answer schema (or a stream's event
schema). A request naming a tool family also supplies that tool's
`inputSchema`; `GET /v1/commands` describes all rows and their explicit
surface capabilities. A request row implements
`Operation`, a stream row `StreamOperation`; a local row registers its
request type only, and the product runs it after `parse_cli` says it is
local.

Answers are typed values. Each surface renders them from the type: text for
the terminal and for a model, JSON for programs, HTML for the web. The one
pure text renderer accepts an explicit `TextStyle` (`Plain` or `Ansi`). MCP
and the shared transport helpers use Plain; a product CLI passes its captured
output style. The shared renderer reads no process environment. No surface
assembles an answer of its own.

### Declaration and execution

`OperationCatalog::builder(product, deadlines)` creates a `CatalogBuilder`.
It declares tool families, request options, precise failure codes and typed
rows. `declare::<Request, Response>(spec, render)` and
`declare_stream::<Request, Event>(spec, render)` take a pure
`fn(&ValueType, TextStyle) -> String` rendering function.
`declare_local::<Request>(spec)` declares a caller-local row.
`finish()` returns an immutable `Arc<OperationCatalog>`.

The catalog alone supplies CLI parsing, help, completion, schemas, MCP tool
lists, fallible follow-up requests, read deadlines, connection budgets and
answer/event rendering with an explicit style. A client can use all of them
without constructing a daemon, operation or runtime table.

`cli_arguments(operation)` projects the existing argument declarations and
typed schema into owned `CliArgument` values: `spec`, `kind`, `required` and
closed textual `choices`. It includes implicit named fields and calling-process
flags. A product may build help and completion from this projection; actual
argument parsing remains in the common parser.

A request with `tool: None` has no tools/list entry and cannot be selected by
tools/call, including a forged operation name in another family. Empty CLI
commands are absent from parsing, help, completion and ready command lines.
An absent HTTP route excludes HTTP dispatch. The HTTP command listing is
the full catalog, with each surface explicitly present or absent. Native
execution remains subject to product authentication and access policy.
`listed` controls human help/completion visibility, not transport access.
`follow_up` fails when its requested ready-call surface is absent; MCP never
falls back to a CLI invocation. Answer footers display that absence explicitly.

`OperationTable::new(catalog)` binds execution separately.
`bind(name, operation)` and `bind_stream(name, stream)` require an existing
declaration with the matching row kind and request/response or event schemas;
a second binding is refused. Operations own only execution and typed access
policy. `required_access(request, declared, caller)` defaults to the declaration's
access; an override may use the authenticated caller to classify the typed request. The table borrows
all pure views from that same catalog. Product transactions, retry identity,
cancellation and transport execution retain their existing owners.

### Access

Each row needs one level: **read**, **write** (attributed writes that can be
undone) or **permanent** (changes that cannot, such as a tombstone). A level
includes the ones below it. The server states the highest level a caller's
credential grants (read, unless it says more), and dispatch
refuses a row above it before the row runs.

### The command line

Every exposed surface but the command line uses the request's own field names. The
command line writes each field in its row's declared form:

| Form | Written | Value |
|---|---|---|
| positional | one word by position | strings as written, other types as JSON (including an entire array); required when the request type requires it |
| words | the remaining words, flags between them still flags | joined by spaces for a string field, a list for an array field |
| rest | everything after it, flags included | as for words |
| named | `--name VALUE` or `--name=VALUE` | a string as written, other types as JSON |
| repeated | `--name VALUE`, repeatable | a list |
| flag | `--name` (or `--name=false`) | a boolean |

A field without a declaration is written `--name VALUE`. A flag that acts on
the calling process (`--out FILE`) is read by the command line and kept out
of the request. Known global options may precede a command (`--json status`,
`--deadline 100 read REF`) or follow it. Longest command selection normalizes
only that leading global prefix; `--` and Rest arguments retain their literal
meaning. `--` ends the flags. Every row also takes `--json`,
`--deadline MS|none`, `--request-id ID`, `--budget N|none`, `--cursor C`,
`--fresh` and `--request JSON` (the whole value, for any request type). A
product may name one **default operation**, run when a command line's first
words name no command (`polylogue QUERY`).

A repeated list argument may explicitly declare `delimiter(',')`, stored as
`value_delimiter`, to split one CLI flag value into several items. This does
not split Words/path lists, HTTP query values or canonical `--request` JSON.
A ready CLI call containing a literal delimiter inside one item uses the
canonical request form so its identity survives parsing.

A request written back as a command line uses the row's own forms whenever
every field can be written that way, and `--request JSON` otherwise; flags
come before positionals, and reading the line back gives the same request.
Help lists each listed row with its usage (`read TARGET [WORDS...]`).

### MCP tools

MCP tools are grouped from the operation table: one tool per **tool family**
of rows whose arguments share a shape and whose access, and so whose read or
write hint, is the same for every row. The product declares each family
with the description sent to the model and whether its rows reach beyond the
product; registration refuses a row whose family is undeclared or whose
access differs from the family's.

Each tool's input schema is generated small, because a harness sends the
whole tool list in every session: `operation` is an enum of the family's
rows (required when there are several); `arguments` is one flat object, the
union of the rows' top-level request fields deduplicated by name, each
reduced to its shallow type (a scalar keeps its type and enum values; an
object, a list or a choice among them is only its bare type), so a deeply
nested request costs no more than a flat one. A field whose argument
declaration is unlisted (`ArgumentSpec::unlisted`) is left out of the list
and still accepted, since `arguments` admits any field; then `request_id`,
`deadline`, and those of `budget_tokens`, `cursor`, `fresh` and `format`
the table serves (`OperationTable::accept(RequestOption)`; a table serves
none of them until it names them, and dispatch refuses one it does not
serve; the command line's `--json` still prints JSON). A field two rows type
differently is a choice of its types, described by the rows that use each;
nothing else is described, the names saying what they are. Each row's own
schema checks the arguments at dispatch, so a field of another row or a
mistyped value is refused there. Tools carry no `outputSchema`: results keep
`structuredContent`, and a product serves the answer's schema
(`OperationTable::answer_schema`) and each family's full row schemas
(`OperationCatalog::family_schemas`) once, from its explain or capabilities
answer. `laws::tools_list_within` holds a product's whole tool list to the
byte budget it passes. Hints come from the
family: read-only for read access, destructive for permanent, and
idempotent (a retried `request_id` is applied once) and open-world as the
family declares; a family whose rows start a new action on every call is
not idempotent.
Every request row is reachable from exactly one tool; stream and local rows
stay off MCP (an MCP call answers once, and local rows run on the caller's
machine). Completion is the protocol's `completion/complete` method, not a
tool. Each product lists its tools on its own interfaces page.

## Requests

A **request** carries the operation, its typed arguments, a deadline, and a
`request_id` chosen by the caller and reused on retry, so a write retried
after a lost reply is applied once. Every request may also carry:

- `budget_tokens`: how much text the answer may hold, the one name for it on
  every surface; absence uses the operation's default, and JSON null
  (`--budget none`) lifts it. A caller's declared tool-output cap still applies;
- `cursor`, to continue an answer;
- `fresh`, which renders in full what the caller already holds (below);
- `format`: `text` (the default) or `json`.

Callers never send an author; the product computes authorship from how the
request arrived.

## Answers

One answer contract serves every operation of both products on every
surface. A failure is returned in place of the value, never beside a partial
one.

**The envelope:**

| Field | Meaning |
|---|---|
| `outcome` | `ok`, `empty`, `degraded` (with `missing`, naming what the answer lacks) or `error` (with the failure, and no value; `stopped_early` is one, in the table below) |
| `count` | how many rows or items the question has, whatever was shown |
| `shown` | how many this answer holds |
| `left_out` | `[{what, count, expand}]`: what was not shown, how much, and an expand handle |
| `next` | the runnable follow-ups: the next page, a narrower request, the same request with the deadline lifted |
| `notes` | anything else the caller should know, such as a budget lowered to its output cap |
| `read_as` | the request's reading as a sentence |
| `positions` | the position each product's read stood at (`{product, position, lag}`) |
| `coverage` | present only when a source's coverage state affects the answer, in the coverage vocabulary of the [query language](../query-language/README.md#transforms) |
| `facets` | for a sliced answer, counts of its rows by a field's values |
| `caller` | the computed author and channel |
| `collapsed` | references rendered as one line because the caller already holds them |
| `used_tokens` | the size of the rendered answer |

**Sections.** An answer assembled from several queries (a pack, a brief, a
status page) is a list of sections. Each section has a `status`:
`complete`, `partial(reason)`, `not_estimable(reason)`,
`not_observed(sources)` or `empty`; whether it is **pending** work; its
**entries** (a line, or several lines that belong together, each citing at
least one reference); and the request that reads it in full. An **expand
handle** (the `expand` request of a `left_out` item) reads more of what was
left out at the answer's own read context, so expanding never mixes two
states of the archive.

**Values.** A number carries `{value, unit, kind, state, unknown_count,
denominator, query}`. Its `state` is one vocabulary for both products and the
web: `complete`, `partial` (some inputs unknown, counted in
`unknown_count`), `inferred`, `unknown(reason)`, `not_recorded`,
`not_applicable` or `pending`; only the first three hold a value, so
unknown is never zero. The `denominator` is the population the number is out
of (a count, its unit and the query that lists it); a ratio over a zero
denominator is unknown.
[web-shell](../web-shell/README.md#visual-language) renders each state.

**Fitting a budget.** The answer is fitted to `budget_tokens` in whole logical entries:
pending before past, every omission counted in `left_out` with its expand
handle, never a cut inside a structure. A deadline, a page size and a budget
each bound only what they name; none changes the question. The fitter:

- keeps a section whole when it fits, and otherwise takes its entries in
  order until the next one would not fit beside the section's left-out line,
  the rest becoming one left-out item;
- offers pending sections room first, then the rest in their order, and
  keeps the sections' own order in the text;
- holds back, for each section not yet placed, what it is sure to need
  (being kept whole or named as left out, whichever is cheaper), so no
  section takes the room another needs and everything is kept whenever
  everything fits; a section with no room for its heading is left out whole;
- counts each left-out line as the answer's surface prints it (a command
  line, or an MCP tool call), and holds back the envelope's other lines
  (`RequestContext::fit` takes the typed answer), so the whole text stays within
  any budget that can hold what every section is sure to need.

The budget is the request's `budget_tokens`, else the operation's default,
lowered to the caller's output cap; a lowered budget is named in `notes`.
Tokens are counted one per four bytes, rounded up (`approximate_tokens`),
the same way everywhere. The operation fits its typed logical sections
through `RequestContext::fit`, before giving its response to a transport.
That method preserves the outcome and updates the typed value, shown count,
omissions and any cap note together.
Every omitted section or entry keeps an expansion request with the budget
explicitly lifted; expanding cannot accidentally request the same default
budget again. The response retains exactly the fitted sections and entries,
and `shown` counts those entries.

Every surface that shows an answer as text takes one rendering path,
`OperationTable::render_answer`: the command line and MCP here, and a
product's client for an answer it read from the socket. The socket and HTTP
carry the typed answer, and the web renders HTML. Rendering measures the
final value and envelope into `used_tokens` and preserves the operation's
logical entries. It does not fit a second time or trim rendered lines.

**Text and data.** The text part is the rendering: rows one line each,
values every row shares printed once in a header line, absolute times, and
`next` last. Rows travel as data only with `format: json`. Over MCP the text
is the model's view and the envelope is `structuredContent`; the rows and the
text are never both sent, so a model reads each row once. In text format the
structured part leaves the value out and says so (`value_in_text`); a client
reads it as `Answer<Option<T>>`, whose value is then `None`, while an
`Answer<T>` refuses it and says where the value is. With `format: json` the
value is in the structured part and the text part holds only the envelope's
lines. After the value
come, in order: a failure's caret, hint, suggestion and candidates; what a
degraded answer is missing; `shown N of [at least] M`; facets; coverage that
is not complete; left-out items with the request that reads each; notes; and
`next`. A follow-up is written as the surface runs it: an MCP tool call over
MCP, a command line elsewhere. On the command line the value goes to
standard output and the rest to standard error.

Dispatch completes every answer the same way: in an answer that is
degraded, or over a source whose coverage is not complete, the count and
every facet's counts become lower bounds; a degraded answer naming
nothing missing, an undeclared failure kind and an unregistered product code
become `io` failures; and the caller's computed authorship is echoed in
`caller`.

**What the caller already holds.** A product that records what it sent to an
identified session (its deliveries) renders a passage that session already
holds, by reference and content hash, as one line naming the earlier
delivery, and lists it in `collapsed`. `fresh` turns this off for one
request.

## Failures and exit codes

A `Failure` carries `kind`, `message`, `code`, `position` where the input
has one, `caret` (the input line with a marker under that span),
`suggestion` (a corrected query, or a whole request, that runs as given,
whenever one can be formed), `hint`, `candidates` (what the input could have meant: an ambiguous
reference's matches, or known names close to an unknown one) and
`retryable`. One table maps every kind to one exit code
and one HTTP status:

| Exit | Kinds | HTTP |
|---|---|---|
| 0 | `ok` | 200 |
| 1 | `empty` (the read succeeded and matched nothing) | 200 |
| 2 | `invalid_query`, `unknown_field`, `bad_value`, `not_a_query`, `bad_request`, `ambiguous`, `stale_cursor`, `not_built` (an index the request needs is not built), `not_yet_available` (designed, with nothing producing it yet, or still starting: it then carries its progress), `not_configured` (the request needs something this deployment does not configure, such as the other product; the configuration key is named) | 400 (`not_built`, `not_yet_available`, `not_configured`: 501) |
| 3 | `refused`, with the operation's stated reason | 403 |
| 4 | `not_found`, `conflict`, `io` | 404, 409, 500 |
| 5 | `degraded` (rows returned, part of what was needed is missing); `stopped_early` (an operation that did not finish; never reported as ok) | 200 with the outcome; 500 |
| 6 | `not_running` (the daemon is down) | none: the client reports it |
| 7 | `timeout`, `cancelled` | 504; none |

A failure teaches: an unknown name carries the nearest known names as
`candidates`, and `explain` as `next`. Empty and degraded are outcomes,
never failure kinds. Language errors become failures in one conversion,
`Failure::from_query_error(error, query)`, which keeps the kind, message,
hint, suggestion, candidates and retryability and places the failure in the
query with its span and caret; command-surface depends on the query language
for its coverage states, so the conversion lives on this side.
`not_yet_available` is temporary (the product is still opening, and says how
far; retrying succeeds); `not_built` is a state (an index or derived product
the request needs does not exist, and will not until it is built; the
message names what to build). A product adds precise codes of its own
in `Failure::code`, each classified once into a shared `FailureKind` at its
operation boundary. No refusal

reason refers to the caller being an agent or lacking a terminal. With
`--json`, a CLI prints `{"error": {...}}` on standard output and nothing on
standard error.

## Callers and authors

Both products compute who a caller is with one implementation, and
authorship from it with one function, `author_of(caller, via, hook) →
authorship`; each product keeps its own stored columns.

- The **caller** is gathered from what the request carries:
  `gather_current` (the calling process) and `gather_peer` (a socket peer,
  from `/proc`) read the supported harnesses' session variables, the nearest
  harness ancestor in the process tree, the controlling terminal and the
  cgroup unit, and keep nothing else from the environment. The transport
  adds what only it sees: whether its MCP server serves several sessions,
  and the session an MCP call names in its per-call metadata (`_meta`,
  available to operations as `RequestContext::metadata`). The server
  records it after its own authentication; it never travels in the request
  JSON.
- **The rules**, the same in both products: the operator's own surfaces
  (web, extension, desktop) are the operator; a hook identity from the
  product's spool refines the caller when it names the caller's harness
  process and, if per call, its tool call (a subagent's names the
  subagent); an MCP call naming its own session is that session; a command
  line with several harnesses' variables and no harness ancestor is
  unattributed; otherwise the nearest harness's session wins, then its
  harness, then a terminal is the operator, then a cgroup unit names a
  script, and anything else is unattributed.
- **A shared MCP server never guesses a session.** When one server process
  serves several sessions, identity comes per call; without it the author
  is `agent:<origin>`, never the session the server started in.
- **The caller is told who they are.** The server passes the computed
  authorship to dispatch, and every answer echoes it in `caller`; status
  begins with it, and every write's answer names its author.

Authors are stored as text: `operator`, `agent:<harness>:<native>`,
`agent:<harness>`, `agent`, `script:<unit>`, `unattributed`. A product whose
stored session IDs differ from the harness's own (Polylogue's
profile-qualified Hermes sessions) maps the session after gathering. The
shared caller fixtures (`laws::author_cases`) state every rule;
`laws::id_one` runs them against the function a product's transports call.

## Seen marks

A **seen mark** is, per consumer (the operator or an agent session) and
target, the latest position that consumer was shown, plus the anchor inside
a session: latest only, no history, and it only moves forward. It is written
only when the target was displayed to that consumer (the web shell's
[display rule](../web-shell/README.md#seen-marks)) or when an agent's read asks
`since: seen`; previews, prerendering, polling, hidden tabs and plain reads
never write one (`Showing::moves_seen_mark`). A view counts as displayed
once it has been visible for `Showing::DISPLAY_TIME` (one second, so
scrolling past or a flicker never counts; `Showing::of_visible_view`); the
web shell measures visibility and reports the showing. `SeenMark::show`
applies a showing. `SeenMark<A>` takes the product's ordered item anchor;
within an unchanged position, later anchors advance the mark. Opaque IDs
alone do not establish reading order. It moves the mark only forward within one generation of
the product's index, and a showing from a new generation replaces it. Only the operator and an
identified session are consumers. Each product stores seen marks with its
authored state, so they survive a rebuild of everything derived. `--since
seen` reads from the mark; an explicit position works as before (`Since`).

## Latency classes and deadlines

Every operation belongs to one class: **local** (answered without the
daemon: help, grammar-only completion, refusals of malformed input),
**status**, **completion**, **read**, **search** or **aggregate**. The class
gives the request's default deadline; each product sets its classes' budgets
and default deadlines on its interfaces page.

- Every request carries its read deadline. `connection_timeout` adds 50 ms
  for connection/readiness admission. After admission, the server owns the
  read timer and a client awaits its typed terminal result or an explicit
  outer caller cancellation.
- A read whose deadline passes ends `timeout`, never `ok` with a partial
  page. The daemon stops read work within 100 ms of the deadline, of a
  cancellation (MCP cancellation, Ctrl-C) or of the caller disconnecting.
  A writer that already won `finish_read_phase` returns its actual
  acknowledgement; the read timer cannot turn an accepted mutation into a
  timeout.
- **Any caller can lift the deadline**, agents included: `--deadline MS` or
  `--deadline none` on the CLI, `deadline: null` over MCP and HTTP. A deadline
  is a default for interactive display, not a limit on what may be asked, so
  no valid request is refused for its size.
- A timeout answer names the stage that was running and the rows scanned, and
  carries as `next` a narrower request the daemon can serve in time and the
  same request with the deadline lifted.

## Server-sent events

Streams (a product's change feed, `follow`, the web shell's live updates) use
one framing:

- each event's `id` is the product's position after that event, so a
  reconnect with `Last-Event-ID` receives everything after it;
- the event name says what the payload is;
- a reconnect from a position the server no longer serves, or across a new
  generation of the product's index, receives one `reload` event instead of a
  silent restart;
- events are sent after the commit they describe.

## Status measurements and verdicts

Each product's `status` is a list of measurements and one verdict, in one
format:

- a **measurement** is `{name, value, state, reason, age}`;
- its **state** is one of `measured`, `none_found` (measured, and there are
  none), `not_measured` (with the reason, such as `timed out`), `unavailable`
  (the thing measured cannot be reached), or `gap` (a known interval with
  missing data); each product uses the states that apply to it;
- the **verdict** is `healthy`, `degraded` or `unknown`, with the first reason
  when not healthy.

Every measurement has its own deadline; one that misses it is
`not_measured: timed out` and the rest still answer within the class. A
measurement is reused only while its key (file times, counters, positions) is
unchanged, and is then shown with its age; time alone never makes an old
value reusable. A ratio with a zero denominator is `not_measured`.
Unmeasured is never shown as zero or as healthy.

## What a product supplies

Its operation table, with each tool's description; its own failure kinds and
their place in the table; its class budgets and default deadlines and its
default budgets; its positions for event IDs and answers; the caller
evidence its harnesses provide; where it stores seen marks; its status
measurements and which states they use.

## Retries

A request carries a `request_id` chosen by the caller and reused on retry.
The product's writer records the request identity, intent and result
atomically with its mutation, so a write retried after a lost reply, a write
sent twice at once, and a retry after a restart are each applied once, and a
retry with changed arguments is a conflict. On a retry the writer reads the
current outcome rather than replaying a stale answer. The table keeps no
request state of its own.

## Using the crate

Declare typed values implementing serde and `JsonSchema` in a
`CatalogBuilder`, finish the catalog, and bind real `Operation` or
`StreamOperation` handlers in `OperationTable`. `declare_local` needs only
the local row's request type. Declaration rejects duplicate names, commands and routes, a request
row naming an undeclared family or a family of another access level, rows of
one family that define one schema name two ways, and argument declarations
the request type cannot hold or the command line cannot read back.

The table provides `parse_cli` and `cli_command`, help and completion,
`cli_write` (writing as it goes: a request row's output, or a stream's
events as each arrives) and `cli_as` (the same, collected), a JSONL socket
server and client (a stream's events arrive as `{"event": …}` lines, then
one answer ends it; `StreamLine` reads either; a line holding an MCP
JSON-RPC message is answered as MCP, so the stdio MCP server each harness
starts is a proxy forwarding to the daemon's one socket, as the agent
contract's performance design has it, with each call's `_meta` reaching the
operation and follow-ups written as tool calls; while a call runs, a
`notifications/cancelled` naming it cancels it and it gets no reply, other
notifications are taken, and lines are read whole without holding up the
call), HTTP dispatch at `POST
/v1` or each row's route (JSON, or a URL-encoded form with one `request`
field, so a form works without JavaScript; `GET /v1/commands` lists the
full catalog with explicit surface capabilities), `open_stream` for a product's server-sent events, and MCP
`tools/list` and `tools/call`. MCP hosts keep initialization,
authentication, transport lifetime and cancellation notifications. Request
IDs are mandatory on every wire transport.

A product's GET routes use `parse_http_query(operation, pairs, request_id)`
on already decoded query pairs. Flat fields use the same typed field kinds
as the CLI: repeated text-list keys remain whole items, structured values
are JSON, and the existing flat HTTP boolean spellings (``, `true`, `1`)
mean true. `request_id`, `cursor`, `deadline`, `budget_tokens` and `fresh`
are envelope fields; a declared argument called `format` remains an argument.
Repeated non-list keys are refused.

An `operation` or `arguments` query key selects canonical envelope mode.
It deserializes one envelope, preserves the original argument JSON and
omitted argument defaults, rejects duplicate keys and requires the operation
to match its route. The route and supplied request identity fill missing
envelope identity fields; typed payload validation stays with the handler.
Canonical JSON booleans retain strict JSON typing. CLI delimiters never
apply to HTTP query pairs.

`Deadline::Default` is omitted on the wire, using the product's `Deadlines`;
`Deadline::None` is JSON null. Dispatch owns each read future and drops
it on timeout, cancellation or socket disconnect. `RequestContext::progress`
records the stage, the measured scan count and an optional narrower request
for the timeout answer, and checks cancellation; `progress_unknown` reports
a stage without inventing a count. Blocking work checks its token within
100 ms and stays owned by its request. Opening a stream and receiving all its
events use the same absolute read deadline and cancellation token. A
`Receiver<Result<Event, Failure>>` carries late typed failures directly;
those terminate the stream with a failure rather than a successful end.

`TransferOperation` implements both `Operation` and `StreamOperation` for one
request type. Its pure typed `streaming(request)` method chooses a bounded
answer or finite event delivery. `declare_transfer::<Request, Response, Event>`
declares the answer and event renderers and separate schemas in one row;
`bind_transfer` binds its real handler. `OperationTable::streaming` lets a
transport choose delivery without inspecting JSON fields. Direct calls and
MCP accept bounded answers and refuse event delivery with a request to use a
streaming transport or a bounded page.

`Operation::has_read_phase(request)` is evaluated before the read timer
starts. A request with a read phase receives its absolute `read_deadline()`
and exact accepted envelope through `RequestContext::request()`. Before a
mutation is handed to its writer, `finish_read_phase()` arbitrates against
the timer and cancellation under one mutex. Expiry or cancellation winning
first refuses the handoff. Ownership winning first makes dispatch await the
writer's actual acknowledgement, even if the read deadline later passes or
its caller cancels. Product writer admission and publication fences remain
product-owned. Pure writes may declare that they have no read phase;
authentication and pre-admission cancellation still apply.

`FailureKind` is closed; `register_failure_code` records each product code's
kind once, and dispatch reports an unregistered or differently classified
code, or a kind the operation did not declare (transport kinds excepted), as
`io`. The server supplies `Caller::authenticated(scope, identity)` with its
granted access and computed authorship; the identity stays a typed Rust
value (`RequestContext::caller().identity::<T>()`). Products use a stable
authenticated scope for retry identity across surfaces.
An MCP call through `mcp_as` (stdio, or an MCP frame on the socket)
computes its caller the same way: from the evidence the server attached
(`Caller::with_evidence`) and the session the call names in
`_meta.session` (`harness:native`), by `author_of` with `via` mcp, so the
shared-server rule holds on every path. `RequestContext::budget` resolves
the answer's budget and
`RequestContext::fit` fits sections, counting left-out lines and the
envelope's lines as the request's surface prints them. `RequestContext::via`
is the channel the request arrived by (the transport sets it: `cli` for the
command line and for socket request frames unless the server's caller says
`hook`, `mcp` for MCP and MCP frames, `web` for HTTP); follow-ups are written
for it.

`SseEvent` encodes JSON without field injection; `SseDecoder` handles split
UTF-8, multiline data and CRLF. A `Position` is a product's generation and
commit counter, written `generation:counter`; counters compare only within a
generation. Events, a read's positions and seen marks all use it. `EventLog`
accepts committed product events; a missing or obsolete resume position, or
one from another generation, produces one reload event.

A measurement with an unmeasured state must have a reason and no value.
`MeasurementCache` reuses values only for an unchanged caller-supplied key,
and `measure` gives each independent measurement its own timeout;
`Status::new` derives the verdict.

The tests send a typed sample table's requests through CLI parsing, a Unix
socket pair, HTTP and MCP; check the retry law against an independent
SQLite writer whose request log and mutation commit atomically (restart,
duplicates sent together, timeout after commit); and run property tests at
the serialization, command-line and fitter boundaries. Run
`cargo xtask test -p command-surface`.

## Laws

- **one-kind-one-exit**: every failure kind maps to exactly one exit code and
  one HTTP status, every surface reports the same kind for the same request,
  and empty and degraded are outcomes, never failure kinds.

- **surface-parity**: the CLI, socket, HTTP and MCP return the same answer, or
  the same failure, for the same request.
- **timeout-never-partial**: a read phase past its deadline ends `timeout`
  with no rows reported as an `ok` answer. A writer that won ownership first
  returns its actual acknowledgement.
- **cancel-stops-work**: a cancelled or disconnected read leaves no read work
  running in the daemon. An already-owned writer finishes through its own
  transaction and admission policy.
- **any-caller-lifts-deadline**: a lifted deadline is honoured for every
  caller, and the request runs until done or cancelled.
- **retry-applies-once**: a write sent twice with one `request_id`, one after
  the other or both at once, is applied once and both replies name the same
  committed row; this holds across a restart.
- **resume-from-last-event-id**: a reconnect with `Last-Event-ID` receives
  exactly the events after it, or `reload`.
- **unmeasured-never-zero**: a measurement that was not taken is never
  rendered as zero or counted as healthy.
- **measurement-reuse-needs-same-key**: a measurement is reused only while
  its key is unchanged.
- **id-one**: Polylogue and Sinex compute the same author for the same caller
  fixture, both through the shared `author_of` (`laws::id_one` over
  `laws::author_cases`, run by each product).
- **id-shared**: an MCP call to a server that serves several sessions,
  without its own identity, is authored `agent:<harness>`, never the
  server's start-up session.
- **tools-generated**: the MCP tool list equals the grouping of the operation
  table: every request row is reachable from exactly one tool, no stream or
  local row is a tool, each tool's hints hold for all its rows, and every
  declared family holds a row. `laws::tools_generated` checks any table;
  each product runs it on its own.
- **tools-list-within**: a table's `tools/list`, as compact JSON, is within
  the byte budget its product passes (`laws::tools_list_within`); tools
  carry flat, shallow, listed arguments and no output schema, and an
  unlisted argument still dispatches.
- **once**: for each supported harness's recorded MCP fixture, the
  model-visible bytes of an answer contain each row or passage once.
  The common test checks the property on a synthetic table: an MCP answer's
  text holds each row once, and its structured content repeats the rows only
  with `format: json`.
- **access-checked**: a row above the caller's access is refused before it
  runs, and a caller the server granted nothing more reads only.
- **lower-bound-under-gaps**: a count over a source whose coverage is not
  complete is a lower bound on every surface, and so are the count and
  facets of a degraded answer.
- **degraded-names-missing**: a degraded answer names what it lacks.
- **error-teaches**: a failure in the input carries its position and caret,
  a suggestion that runs as given when one can be formed, and the
  candidates nearest an unknown name.
- **query-error-converts-once**: each query-language refusal becomes the
  failure kind of the same name, with its position, suggestion and hint.
- **cli-command-round-trips**: reading back the command line written for a
  request gives the same request.
- **whole-entries**: the fitter keeps or leaves out each entry whole.
- **pending-first**: pending sections are offered room before past ones.
- **fit-within-budget**: what is kept and left out add up to every entry;
  everything is kept whenever everything fits; and the text with its
  left-out lines stays within any budget that can hold what every section is
  sure to need.
- **text-within-budget**: the operation fits the typed answer through its
  context, reserving its envelope lines. Rendering preserves every retained
  entry and measures the final text into `used_tokens`; it never trims the
  value again. A budget large enough for the required envelope and omission
  lines holds the rendered answer within that budget.
- **expansion-lifts-default-budget**: an omission's expansion explicitly
  lifts the budget, so executing it cannot fall back to the same default.
- **json-keeps-data**: a text budget does not cut rows carried as JSON.
- **text-answers-read-back**: an MCP answer in text format reads back as
  `Answer<Option<T>>` with the value `None` and `value_in_text`, and
  `Answer<T>` refuses it saying where the value is.
- **socket-carries-mcp**: an MCP message sent over the socket is answered as
  an MCP call, authored from the server's evidence and the call's `_meta`
  as on stdio, with follow-ups written as tool calls; a cancelled call gets
  no reply.
- **streams-print-as-they-arrive**: a stream's events reach the command
  line's output while the stream is still open, and a stream with no events
  prints nothing and succeeds.
- **every-entry-cited**: an entry has at least one line and one citation.
- **seen-mark-moves-forward**: a seen mark never moves to an earlier
  position.
- **only-showing-moves-seen-mark**: only a display or a `since: seen` read
  moves a seen mark.

For a row that both reads and writes, `required_access` classifies its typed request before execution. The row's declaration advertises its maximum access, so tool annotations remain conservative. Ordinary rows use their declared access for every call.
