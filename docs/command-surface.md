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

A product adds kinds of its own, each mapped once into this table. No refusal
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
