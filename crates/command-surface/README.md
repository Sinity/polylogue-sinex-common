# command-surface

How an operation travels between a caller and a product daemon. Each product
declares one typed operation table; the CLI, the socket, HTTP and MCP are
generated from it, so every operation is reachable the same way from every
surface and none exists on one surface only. This crate holds the mechanics
both products share: the `Operation` trait, the `Failure` type with its exit
and HTTP table, the request and answer envelopes, latency classes and
deadlines, server-sent-event framing, and the status figure-and-verdict
format.

## Operations

An `Operation` is one row of a product's table. It declares:

- its name and command words, and the MCP tool name derived from them;
- its request and answer types (the MCP tool's `inputSchema` and
  `outputSchema` come from these);
- its HTTP route;
- its latency class;
- the failure kinds it can return.

Answers are typed values. Each surface renders them from the type: text for
the terminal, JSON for MCP and HTTP, HTML for the web. No surface assembles an
answer of its own.

## Requests and answers

A **request** carries the operation, its typed arguments, a deadline, and a
`request_id` chosen by the caller and reused on retry, so a write retried
after a lost reply is applied once. Callers never send an author; the product
computes authorship from how the request arrived.

An **answer** carries an outcome, the typed value, notes, and `next`: the
runnable follow-ups a caller may take (the next page, a narrower query, the
same query with the deadline lifted). A failure is returned in place of the
value, never beside a partial one.

## Failures and exit codes

A `Failure` carries `kind`, `message`, `position` where the input has one,
`suggestion` (a corrected request that runs as given, whenever one can be
formed), `hint` and `retryable`. One table maps every kind to one exit code
and one HTTP status:

| Exit | Kinds | HTTP |
|---|---|---|
| 0 | `ok` | 200 |
| 1 | `empty` (the read succeeded and matched nothing) | 200 |
| 2 | `invalid_query`, `unknown_field`, `bad_value`, `not_a_query`, `bad_request`, `ambiguous`, `stale_cursor`, `not_yet_available` (designed, with nothing producing it yet) | 400 (`not_yet_available`: 501) |
| 3 | `refused`, with the operation's stated reason | 403 |
| 4 | `not_found`, `conflict`, `io` | 404, 409, 500 |
| 5 | `degraded` (rows returned, part of what was needed is missing); `stopped_early` (an operation that did not finish; never reported as ok) | 200 with the outcome; 500 |
| 6 | `not_running` (the daemon is down) | none: the client reports it |
| 7 | `timeout`, `cancelled` | 504; none |

A product adds precise codes of its own in `Failure::code`, each classified
once into a shared `FailureKind` at its operation boundary. No refusal
reason refers to the caller being an agent or lacking a terminal. With
`--json`, a CLI prints `{"error": {...}}` on standard output and nothing on
standard error.

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

## Status figures and verdicts

Each product's `status` is a list of figures and one verdict, in one format:

- a **figure** is `{name, value, state, reason, age}`;
- its **state** is one of `measured`, `none_found` (measured, and there are
  none), `not_measured` (with the reason, such as `timed out`), `unavailable`
  (the thing measured cannot be reached), or `gap` (a known interval with
  missing data); each product uses the states that apply to it;
- the **verdict** is `healthy`, `degraded` or `unknown`, with the first reason
  when not healthy.

Every figure has its own deadline; one that misses it is
`not_measured: timed out` and the rest still answer within the class. A
figure is reused only while its key (file times, counters, positions) is
unchanged, and is then shown with its age; time alone never makes an old
value reusable. A ratio with a zero denominator is `not_measured`.
Unmeasured is never shown as zero or as healthy.

## What a product supplies

Its operation table; its own failure kinds and their place in the table; its
class budgets and default deadlines; its positions for event IDs; its status
figures and which states they use.

## Using the crate

Implement `Operation` with typed `Request` and `Response` values that implement
serde and `JsonSchema`. Register each implementation in `OperationTable`; its
`OperationSpec` supplies the command words, route, summary, reason, latency
class, declared failures and write flag. Registration rejects duplicate
commands, names, routes and MCP tool names. The table provides CLI parsing,
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

`Figure` validates that an unmeasured state has a reason and no value.
`FigureCache` reuses values only for an unchanged caller-supplied key;
`measure_figure` gives each independent measurement its own timeout. Use
`Status::new` to derive a verdict from those measured and missing states.

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
- **unmeasured-never-zero**: a figure that was not measured is never rendered
  as zero or counted as healthy.
- **figure-reuse-needs-same-key**: a figure is reused only while its key is
  unchanged.
