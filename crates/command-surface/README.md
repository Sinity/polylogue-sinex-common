# command-surface

How an operation travels between a caller and a product daemon, and what
comes back. Each product declares one typed operation table; the CLI, the
socket, HTTP and MCP are generated from it, so every operation is reachable
the same way from every surface and none exists on one surface only. This
crate holds the mechanics both products share: the `Operation` trait, the
answer contract every surface of both products uses, the `Failure` type with
its exit and HTTP table, who the caller is and who authored a write, the
seen mark, latency classes and deadlines, server-sent-event framing, and the
status format of measurements and a verdict.

## Operations

An `Operation` is one row of a product's table. It declares:

- its name and command words;
- its request and answer types (its part of the MCP tool's `inputSchema`
  and `outputSchema` comes from these);
- its HTTP route;
- its latency class;
- the failure kinds it can return.

Answers are typed values. Each surface renders them from the type: text for
the terminal and for a model, JSON for programs, HTML for the web. No surface
assembles an answer of its own.

### MCP tools

MCP tools are grouped from the operation table: one tool per family of rows
whose arguments share a shape and whose read or write hint is the same for
every row. Each tool's input schema is generated, with one `oneOf` branch per
row where the rows differ, and its description is kept in the table. Every
row that is not local-only is reachable from exactly one tool; local-only
rows stay on the CLI. Completion is the protocol's `completion/complete`
method, not a tool. Each product lists its tools on its own interfaces page.

## Requests

A **request** carries the operation, its typed arguments, a deadline, and a
`request_id` chosen by the caller and reused on retry, so a write retried
after a lost reply is applied once. Every request may also carry:

- `budget_tokens`: how much text the answer may hold, the one name for it on
  every surface; each operation has its own default, and a caller whose
  harness declares a tool-output cap has the budget clamped to it;
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
| `outcome` | `ok`, `empty`, `degraded` or `stopped_early` (the failure table below) |
| `count` | how many rows or items the question has, whatever was shown |
| `shown` | how many this answer holds |
| `left_out` | `[{what, count, expand}]`: what was not shown, how much, and an expand handle |
| `next` | the runnable follow-ups: the next page, a narrower request, the same request with the deadline lifted |
| `read_as` | the request's reading as a sentence |
| `positions` | each product's read position and lag that the answer read at |
| `coverage` | present only when a source's coverage state affects the answer, in the coverage vocabulary of the [query language](../query-language/README.md#transforms) |
| `caller` | the computed author and channel |
| `collapsed` | references rendered as one line because the caller already holds them |
| `used_tokens` | the size of the rendered answer |

**Sections.** An answer assembled from several queries (a pack, a brief, a
status page) is a list of sections. Each section has a `status`:
`complete`, `partial(reason)`, `not_estimable(reason)`,
`not_observed(sources)` or `empty`. Every line is cited by reference, and an
**expand handle** reads more of what was left out at the answer's own read
context, so expanding never mixes two states of the archive.

**Values.** A number carries `{value, unit, kind, state, unknown_count,
denominator, query}`. Its `state` is one vocabulary for both products and the
web: `complete`, `partial` (some inputs unknown, counted in
`unknown_count`), `inferred`, `unknown(reason)`, `not_recorded`,
`not_applicable` or `pending`. Unknown is never zero.
[web-shell](../web-shell/README.md#visual-language) renders each state.

**Fitting a budget.** The answer is fitted to `budget_tokens` in whole lines:
pending before past, every omission counted in `left_out` with its expand
handle, never a cut inside a structure. A deadline, a page size and a budget
each bound only what they name; none changes the question.

**Text and data.** The text part is the rendering: rows one line each,
values every row shares printed once in a header line, absolute times, and
`next` last. Rows travel as data only with `format: json`. Over MCP the text
is the model's view and the envelope is `structuredContent`; the rows and the
text are never both sent, so a model reads each row once.

**What the caller already holds.** A product that records what it sent to an
identified session (its deliveries) renders a passage that session already
holds, by reference and content hash, as one line naming the earlier
delivery, and lists it in `collapsed`. `fresh` turns this off for one
request.

## Failures and exit codes

A `Failure` carries `kind`, `message`, `position` where the input has one,
`suggestion` (a corrected request that runs as given, whenever one can be
formed), `hint`, `candidates` (what the input could have meant: an ambiguous
reference's matches, or known names close to an unknown one) and
`retryable`. One table maps every kind to one exit code
and one HTTP status:

| Exit | Kinds | HTTP |
|---|---|---|
| 0 | `ok` | 200 |
| 1 | `empty` (the read succeeded and matched nothing) | 200 |
| 2 | `invalid_query`, `unknown_field`, `bad_value`, `not_a_query`, `bad_request`, `ambiguous`, `stale_cursor`, `not_yet_available` (designed, with nothing producing it yet, or still starting: it then carries its progress), `not_configured` (the request needs something this deployment does not configure, such as the other product; the configuration key is named) | 400 (`not_yet_available`, `not_configured`: 501) |
| 3 | `refused`, with the operation's stated reason | 403 |
| 4 | `not_found`, `conflict`, `io` | 404, 409, 500 |
| 5 | `degraded` (rows returned, part of what was needed is missing); `stopped_early` (an operation that did not finish; never reported as ok) | 200 with the outcome; 500 |
| 6 | `not_running` (the daemon is down) | none: the client reports it |
| 7 | `timeout`, `cancelled` | 504; none |

A failure teaches: an unknown name carries the nearest known names as
`candidates`, and `explain` as `next`. Language errors become failures in one
conversion, in the query language. A product adds precise codes of its own
in `Failure::code`, each classified once into a shared `FailureKind` at its
operation boundary. No refusal
reason refers to the caller being an agent or lacking a terminal. With
`--json`, a CLI prints `{"error": {...}}` on standard output and nothing on
standard error.

## Callers and authors

Both products compute who a caller is with one implementation, and
authorship from it with one function, `author_of(caller, via) → author`;
each product keeps its own stored columns.

- The **caller** is gathered from what the request carries: the harness's
  session variables, MCP per-call metadata (`_meta`), the socket peer and
  the controlling terminal. The server records it after its own
  authentication; it never travels in the request JSON.
- **A shared MCP server never guesses a session.** When one server process
  serves several sessions, identity comes per call; without it the author
  is `agent:<origin>`, never the session the server started in.
- **The caller is told who they are.** Status begins with the computed
  author and channel, and every write's answer names its author.

## Seen marks

A **seen mark** is, per consumer (the operator or an agent session) and
target, the latest position that consumer was shown, plus the anchor inside
a session: latest only, no history. It is written only when the target was
displayed to that consumer (the web shell's
[display rule](../web-shell/README.md#seen-marks)) or when an agent's read asks
`since: seen`; previews, prerendering, polling and hidden tabs never write
one. Each product stores seen marks with its authored state, so they survive
a rebuild of everything derived. `--since seen` reads from the mark; an
explicit position works as before.

## Latency classes and deadlines

Every operation belongs to one class: **local** (answered without the
daemon: help, grammar-only completion, refusals of malformed input),
**status**, **completion**, **read**, **search** or **aggregate**. The class
gives the request's default deadline; each product sets its classes' budgets
and default deadlines on its interfaces page.

- Every request carries its deadline. The CLI's socket timeout is the
  deadline plus 50 ms, so a hung daemon never hangs a shell.
- A request whose deadline passes ends `timeout`, never `ok` with a partial
  page. The daemon stops the work within 100 ms of the deadline, of a
  cancellation (MCP cancellation, Ctrl-C) or of the caller disconnecting; no
  abandoned work keeps running.
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

## Using the crate

Implement `Operation` with typed `Request` and `Response` values that implement
serde and `JsonSchema`. Register each implementation in `OperationTable`; its
`OperationSpec` supplies the command words, route, summary, reason, latency
class, declared failures and write flag. Registration rejects duplicate
commands, names and routes, and a tool family whose rows differ in their
read or write hint. The table provides CLI parsing,
help and command completion, a JSONL duplex socket server/client, HTTP
request/response dispatch, and MCP `tools/list` and `tools/call` JSON-RPC.
MCP hosts retain responsibility for initialization, authentication, transport
lifetime and routing cancellation notifications to a `CancellationToken`.

The CLI accepts schema-derived named fields or `--request JSON`, plus
`--request-id`, `--deadline MS|none` and `--json`. Request IDs are mandatory
on every wire transport. `cli_command` emits a quoted, runnable command from
the exact request, including its retry identity and deadline.
HTTP accepts an envelope at `POST /v1` or the declared operation route,
as JSON or an ordinary URL-encoded form with one `request` field containing
the envelope JSON. This lets an operation form work without JavaScript.
`GET /v1/commands` returns the generated descriptors and schemas. Products
supply ordinary GET document routes from their typed answers. The socket
allows one outstanding request per connection. MCP tool arguments wrap the
same typed `arguments`, `request_id` and optional `deadline`; schemas include
both the envelope and the operation's nested type definitions.

`Deadline::Default` is omitted on the wire, using the product's `Deadlines`
configuration; `Deadline::None` is JSON null. No class has a universal hard
limit. Dispatch owns the operation future and drops it on timeout,
cancellation or socket disconnect. `RequestContext::progress` records the
stage, measured scan count and optional narrower request for the timeout answer,
and checks cancellation. `progress_unknown` reports a stage without a measured
row count; unmeasured work, including admission, is described as unknown rather
than zero. Blocking work must check its token within 100 ms and
must remain owned by that request. A future which blocks its executor
without yielding cannot satisfy the cancellation contract.

`Answer<T>` separates ok, empty and degraded values from an error carrying
no value. `FailureKind` is closed; a product's precise failure name goes in
`Failure::code` and is classified into one shared kind at its operation
boundary. `register_failure_code` records each classification once and
dispatch rejects an unregistered or differently classified code. This keeps
every exit and HTTP mapping in one exhaustive table.
The registry checks that operation failures were declared, while transport
failures remain available to every operation.

The server supplies `Caller::authenticated(scope, identity)` after its
authentication or peer inspection. The identity remains a typed Rust value,
read with `RequestContext::caller().identity::<T>()`, and never travels in
the request JSON. The `*_as` adapter methods preserve it while computing
the transport origin themselves. Products use a stable authenticated scope
for retry identity across CLI, HTTP and MCP.

The table coalesces only in-flight writes with one caller scope and request ID.
Changed arguments conflict while that identity is in flight. On completion,
error, timeout, cancellation or owner-future drop, the entry leaves the registry;
already-admitted waiters retain its final normalized answer until they finish. `call_as` returns a nonserialized `DispatchResult` containing that answer and the trusted caller context which actually produced it. A shared result retains its owner's context even after registry retirement. A waiter's own timeout or cancellation instead retains that waiter's context. Products validate both the owner's retained publication authority and the current waiter's authority at their output boundary; a new caller scope observation cannot relabel an old result. Neither the retained context nor its extensions enter JSON, schemas or Debug output. Anonymous `call` projects only the answer. Convenience transport adapters render only the answer. A product with a native publication fence uses `call_as` at that boundary and retains the returned owner context until its final bounded output activation.
Dropping or cancelling a waiter does not cancel the owner. Dropping the owner
cancels its work and releases its waiters with a cancelled outcome.

Every sequential retry reaches the product writer. The writer atomically records
the immutable request identity, intent and committed result with its mutation;
this makes retry and conflict checks survive a daemon restart. Publication or
availability evidence can advance after acceptance, so the writer reads the
current truthful outcome on retry rather than replaying a stale transport
answer. Common retains no completed-write or refusal log.

`SseEvent` encodes JSON without field injection; `SseDecoder` handles split
UTF-8, multiline data and CRLF. `EventLog` accepts committed product events
with opaque generation/cursor positions. Retention advances explicitly with
`retain_after`; a missing or obsolete resume position produces one reload
event. The log does not infer a commit from a published event.

A measurement with an unmeasured state must have a reason and no value. The
measurement cache reuses values only for an unchanged caller-supplied key,
and each independent measurement runs under its own timeout. The verdict is
derived from those measured and missing states.

The `surfaces` integration tests define a real typed sample table and send
its requests through CLI parsing, a Unix socket pair, HTTP envelopes and MCP
JSON-RPC. Operation lifetime tests use an independent SQLite fixture writer
whose request log and domain mutation commit atomically, including restart,
concurrent waiters and timeout-after-commit retry. `protocol` tests include generated serialization cases and every
byte boundary of SSE framing. Run `cargo xtask test -p command-surface`.

## Laws

- **one-kind-one-exit**: every failure kind maps to exactly one exit code and
  one HTTP status, and every surface reports the same kind for the same
  request.
- **surface-parity**: the CLI, socket, HTTP and MCP return the same answer, or
  the same failure, for the same request.
- **timeout-never-partial**: a request past its deadline ends `timeout` with
  no rows reported as an `ok` answer.
- **cancel-stops-work**: a cancelled or disconnected request leaves no work
  running in the daemon.
- **any-caller-lifts-deadline**: a lifted deadline is honoured for every
  caller, and the request runs until done or cancelled.
- **retry-applies-once**: a write sent twice with one `request_id` is applied
  once and both replies name the same committed row.
- **resume-from-last-event-id**: a reconnect with `Last-Event-ID` receives
  exactly the events after it, or `reload`.
- **unmeasured-never-zero**: a measurement that was not taken is never
  rendered as zero or counted as healthy.
- **measurement-reuse-needs-same-key**: a measurement is reused only while
  its key is unchanged.
- **id-one**: Polylogue and Sinex compute the same author for the same caller
  fixture, both through the shared `author_of`.
- **tools-generated**: the MCP tool list equals the grouping of the operation
  table: every row that is not local-only is reachable from exactly one
  tool, and each tool's hint is the same for all its rows.
- **once**: for each supported harness's recorded MCP fixture, the
  model-visible bytes of an answer contain each row or passage once.
