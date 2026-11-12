# Vocabulary

One name per concept for the mechanics Polylogue and Sinex share. A name says
what the thing is in ordinary words; the same names are used in identifiers,
local variables, comments, docs, error text and tests. Each product's own
vocabulary page names its own concepts; a word listed here means this
mechanic and nothing else in the shared crates.

## Stored bytes

| Name | Meaning |
|---|---|
| blob | the bytes of one stored input, named by their hash |
| name | a blob's hash: BLAKE3 of the content, or a keyed BLAKE3 for a sealed blob |
| plain, sealed | a blob stored as zstd frames, or as encrypted frames |
| data key, key reference | the key that seals a blob, and the product's name for it |
| frame | one independently decodable piece of a blob in a segment |
| segment | an append-only file of frames, written by one product's writer |
| extent | the byte range in one segment that holds one blob's frames; rows cite extents |
| chunk | a blob holding one content-defined piece of a stream rewritten whole; a revision is the list of its chunks' extents |
| cut point | where content-defined chunking ends a chunk, chosen by a rolling hash of the bytes before it |
| group | the blobs the writer appends and syncs together before the product commits them |
| commit | making a group's extents real, in the same transaction as the rows that cite them |
| tail | bytes after a segment's last committed extent; truncated when the segment opens |
| punch | releasing a committed extent's bytes with a hole punch |
| scan | the on-demand check of every frame of every segment, which reports and repairs nothing |

## Queries

| Name | Meaning |
|---|---|
| catalog | a product's units or tables, their fields, closed-set values and defaults |
| compiler | a product's translation of a parsed query into execution |
| normal form | the canonical parsed question, with source spans removed and commutative operands ordered |
| canonical JSON | the bytes every identity hash reads: a domain tag, a zero byte, then JSON with object keys sorted and numbers as the value holds them |
| question identity | the BLAKE3 hash of the normal form and declared parameter types |
| read position | a product-owned retained view that a continuation reads |
| version binding | a schema, definition, grant or ranking version fixed for a continuation |
| unit | what a query reads rows of (a product's sessions, events, tables …) |
| condition | one test of a row; conditions side by side are joined by `AND` |
| transform | one `\|` step of a query: a relation in, a relation out |
| relation | what a query reads or produces: a unit, a saved query, a kept result or a reference |
| natural match | what `field:value` means for a field's kind, declared once in the catalog |
| order relation | `near`, `before`, `after`, `during` or `containing`, by time or by steps |
| order scope | the sequence a unit's rows have steps in, such as a session's timeline |
| ranked list | one ordering of selected rows (text relevance, `similar:`, `fuse` …); ranking never changes membership |
| fusion | combining a query's ranked lists into one order by reciprocal rank |
| answer (of a query) | one evaluation of a query, `answer:<id>`; `@last` is the caller's latest and `@N` one of its rows |
| kept result | a stored answer with its read context, `result:<id>`; reading it never re-runs |
| mixed query | a query whose units belong to both products |
| part | the largest piece of a mixed query that one product evaluates alone |
| coverage state | how completely a source covers an interval: `complete`, `gap`, `unknown`, `outside_span`, `not_yet`, `excluded`, `unavailable` |
| expression | a field, literal, function call or arithmetic, with checked dimensions |
| step | one position in an order scope; `within 1 step` is adjacent |
| matched row | the one inner row an order condition exposes as `match.`; `pick first by` chooses it, otherwise the nearest (`MatchChoice`) |
| pattern | a top-level `exists` or `seq` condition; `\| matches` lists one row per step of each of its matches |
| exact text | a field kind for identifiers and closed words, whose `field:value` is equality |
| generated field | a field the language adds to a relation (`match.distance`, `change`, `coverage`); it never hides a stored field |
| catalog reference form | a reference a product declares: a prefix with an optional marker, a whole word, or a bare identifier |
| unknown | the third truth value: a condition over a missing value is neither true nor false |
| closed set | the fixed list of values a field may hold |
| namespace | the `pl.` or `sx.` prefix that says which product a unit belongs to |
| page size | how many rows travel in one response; never part of the question |
| cursor | the sealed key that continues one question from its last row |
| logical day | the day a person reads: from the start hour in the configured zone to the same hour the next day |
| day setting | the zone and start hour that define the logical day (`DaySetting`) |
| time quality | where an event time came from: intrinsic, captured, file modification time, the operator, or unknown |
| precision | how exact a time is: `instant` to `year`, or `bounded` with an end |
| coarse time | a time whose precision is a period rather than an instant |
| output schema | the typed fields and optional main time of an expanded saved question or kept result |
| implicit source | a source whose omitted unit inherits the enclosing actual row domain |
| bound query | a query whose source output scopes and suffix fields have passed shared binding |
| time bound | a `since` lower bound or `until` exclusive upper bound, preserved as its own query operation |

## Authored records

| Name | Meaning |
|---|---|
| authored record | anything a person or agent writes, in either product; shares the header and rules of the `authored` crate |
| header | `{root, rev, kind, key, value, author, via, at, revises, withdraws}`, the part every authored record has |
| root | the ID of a record's first revision, which names the record |
| revision | one version of a record, `<root>@<rev>` |
| withdrawal | a record that withdraws revisions or whole roots; withdrawing a withdrawal restores |
| withdrawn | named by a withdrawal that is not itself withdrawn |
| current | not withdrawn, and no descendant through `revises` is not withdrawn |
| head | a current revision; two heads of one record are a conflict |
| conflict | two or more current revisions of one record, resolved by one revision of every head |
| base | a revision a write revises |
| stale | a base that is no longer current: a command naming it is refused, markup and imports fork |
| affected roots | the roots whose currency can change when a record is added |
| arrival | how a request arrived: the computed author, the channel, the writer's time and the stale rule |
| write ID | the ID of the n-th write of a request, `<request>.<n>`; a request of one write uses the request ID; a request's writes are placed together or not at all |
| request digest | the hash of a request's content with keys sorted, for idempotent retries |
| uniqueness key | the components that say which records occupy the same uniqueness slot |
| uniqueness slot | one value of a kind's uniqueness key; one record holds it, or a write into it revises or answers "already" |
| order key | a base-62 fraction that orders records; a key exists between any two |
| sort key | an order key with the record root that breaks ties |

## Operations and transport

| Name | Meaning |
|---|---|
| operation | one row of a product's operation table, reachable from every surface it belongs on |
| request row, stream row, local row | an operation the daemon answers once; one whose events the daemon sends until it ends; one the calling process runs without the daemon |
| access level | what a row needs from its caller: `read`, `write` (attributed, undoable) or `permanent` (cannot be undone) |
| argument form | how a request field is written on the command line: positional, words, rest, named, repeated, flag |
| default operation | the row a command line runs when its first words name no command |
| operation declarations | the typed list of a product's operations, their schemas, surfaces and rendering, from which the CLI, socket, HTTP and MCP are generated; usable without handlers (`OperationDeclarations`) |
| operation table | a product's operation declarations bound to the handlers that run them (`OperationTable`) |
| request, answer | what a caller sends, and what it gets back: an outcome and an envelope |
| outcome | what kind of answer came back: `ok`, `empty`, `degraded` (with what is missing) or `error` (a failure and no value) |
| failure, failure kind | a typed error, and its stable name on the wire |
| failure code | a product's precise failure name, registered once against a shared failure kind |
| caller scope | the server's authenticated identity namespace for request retries; never supplied in request JSON |
| exit code | the CLI's process status for an answer, one per outcome and failure kind |
| caret | the failing input line with a marker under the failing span |
| suggestion | a corrected query, or a whole request, that runs as given |
| candidates | what a failing input could have meant, each runnable |
| latency class | the group an operation belongs to for its default deadline: local, status, completion, read, search, aggregate |
| deadline | how long a request may run; a default any caller can lift |
| position | where a product's stream of changes stands: its index generation and commit counter, `generation:counter`; an event's ID, what a read stood at, a seen mark's place |
| not built, not yet available | a request needing an index or derived product that does not exist until built; a request the product cannot answer yet because it is still opening |
| envelope | the fields every answer carries besides its outcome: count, shown, left out, next, notes, reading, positions, coverage, facets, caller, collapsed, used tokens |
| lower bound | a count over a source whose coverage was not complete |
| follow-up | a ready request in `next`, written as the surface runs it: a command line or an MCP tool call |
| section, entry | one part of an assembled answer; one whole item of a section, a line or several, citing at least one reference |
| section status | how complete one section of an assembled answer is: `complete`, `partial`, `not_estimable`, `not_observed`, `empty` |
| expand handle | a token in an answer that reads more of what was left out, at the answer's read context |
| value state | how complete a number is: `complete`, `partial`, `inferred`, `unknown`, `not_recorded`, `not_applicable`, `pending` |
| denominator | the population a number is out of: its count, unit and query |
| budget | how many tokens an answer may hold (`budget_tokens`); answers are fitted in whole lines |
| caller, author | who sent a request, gathered from what it carries, and the author computed from it |
| caller evidence | what a transport observed about a caller: harness, session, tool call, harness process, terminal, cgroup unit, whether its MCP server is shared, the session a call names |
| harness, agent session | an agent program that runs sessions (`claude-code`, `codex` …), and one of its sessions, `harness:native` |
| harness ancestor | a harness process above the caller; the nearest one launched it |
| shared server | an MCP server process serving several sessions, whose start-up variables name none of them |
| hook identity | the session (and subagent) a harness process or tool call belongs to, from a product's hook spool |
| via | the channel a request reached the product by: cli, mcp, web, extension, desktop, hook, markup, import, daemon; follow-ups are written for it |
| tool family | operation rows grouped into one MCP tool because their arguments share a shape and their hint |
| seen mark | per consumer and target, the latest position displayed to that consumer |
| consumer | who a seen mark belongs to: the operator or one agent session |
| showing | how a target came before a consumer; only a display (visible for the display time, one second) or a `since: seen` read moves a seen mark |
| value in text | an MCP answer in text format whose value its text part holds, left out of the structured part |
| measurement | one value in status, with its state, reason and age |
| measurement state | `measured`, `none_found`, `not_measured`, `unavailable`, `gap` |
| verdict | the one-word health of a product: `healthy`, `degraded`, `unknown` |

## References

| Name | Meaning |
|---|---|
| reference | the one written form of anything addressable: an object of one product or a locator, with an optional selector |
| global form | `<product>:<kind>/<id>[@<revision>][#<selector>]`: what is stored and sent |
| local form | `<kind>:<id>…`, written inside the object's own product and normalised to the global form at input |
| kind | the word after the product that says what sort of object a reference names (`session`, `ev`) |
| identity | the part of a reference that says which object of its kind, as its owner writes it |
| revision | the part after `@`: one exact revision, as the owner defines it |
| selector | the part after `#`: a place inside the thing (lines, time, rectangle, text range, quote, named place) |
| quote | the exact text a span held, with the text before and after it, for finding the span again |
| locator | the canonical native address of a thing outside both products' objects (`url:`, `file:`, `part:`, a catalog form) |
| catalog form | a locator from an outside catalog's identifier (`youtube:`, `doi:`, `isbn:` …) that URLs of that catalog fold into |
| canonicalizer | the versioned rules and data that turn an address into its locator |
| tracking parameter | a declared query parameter that only records how a link was shared, removed by the canonicalizer |
| URL pattern | a declared URL shape and the reference text it becomes |
| route | an object's web address, `/<kind>/<id>[@<revision>]`, under its product's base URL |
| sub-view | a named view of one object, a path segment after its route (`trace`, `history`) |
| context word | `self`, `here`, `last`, `latest`: words the caller's context resolves |
| row pick | `@N`, `@N.M`, `@N-M`, `@N,M`, `@*`, `@last`: rows of the last list printed for a caller |

## Signals

| Name | Meaning |
|---|---|
| signal rule | a watched query with a delivery rule |
| watch | a signal rule that reports changes as notices |
| occurrence, condition period | the thing a row is about, keyed by the product, and one stretch during which its condition holds (`period` in code) |
| trigger | a rule revision paired with an occurrence and condition period; fires at most once unless its delivery is retracted |
| retraction | dropping one delivery its product's writer refused, so its trigger can fire again |
| baseline | the matches a rule starts from, which never fire |
| report | what an evaluator tells the deliverer: fired, cleared, retracted, a rule's revision, progress |
| progress | a product's evaluation is up to date as of an instant, except the rules whose data is behind |
| host policy | the one deliverer's holds, rechecks, rate limits, budget and routing on a host |
| handoff | the moment a notice leaves the host policy with its decision |
| delivery | the record of one handoff: the firing, its decision, holds and the attention it was decided under; it never changes |
| outcome | what happened to a delivery afterwards, appended to it: an attempt, shown, closed, opened, acknowledged, snoozed, released, cleared, seen |
| route, decision | the pure function from a notice and the attention to show, send away, hold or passive, and its answer with a reason |
| attention | the deliverer's snapshot of quiet periods, presence, focus, deep work, breakpoints and budget used |
| hold | a notice waiting until a time or until its releasing input changes: quiet, away, deep work, incomplete, snoozed |
| breakpoint | a moment cheap to interrupt: an app or work-episode switch, the end of an input bout, an unlock |
| inhibition | a shipped root-cause rule folding the notices of the rules it names while its occurrence is open |
| summary | one notification standing for several deliveries: the budget summary, the away summary, consequences of a cleared root |
| producer notice | a notice any credentialed program submits, a delivery of the implicit rule `producer:<name>` |
| delivery receipt | the returned outcome of one value or action attempt, recorded against its delivery identity |
| attempt | one execution of a delivery through one outlet, initially unknown until a returned receipt |
| outlet | where an interrupting notice goes: the desktop or the away command |
| away command | the deployment's command that receives notices sent away while the operator is away |
| notice, value, action | the three kinds of delivery: a message, a number written to a target, a command run |
| interrupting, passive | a notice shown as a desktop notification or sent away, or one that waits in the inbox, status and briefs |
| interrupt budget | how many interrupting notices per hour rules not authored by the operator, and producers, may cause |
| quiet hours | when interrupting notices wait |
| deliverer | the one process on a host that shows desktop notifications |
| shipped rule | a rule that comes with a product, replaceable by an authored rule of the same name |
| follow, wait, backtest | the evaluator delivered to an open connection; follow with an exit at its first message; a replay of rules over stored history; none records anything |

## Web

| Name | Meaning |
|---|---|
| token | a named value of the shared visual language (`--surface`, `--failure-ink` …) |
| shell | the shared TypeScript that carries keys, palette, copy, live updates, windowed lists and the timeline |
| element | one shell behaviour as a custom element (`<shell-window>`, `<shell-timeline>` …) wrapping markup that works without script |
| fragment | server-rendered HTML for part of a page, sent live or fetched |
| subscription | one followed reference, saved query or product stream, resumed after its own position |
| union stream | a product's one event stream per browser, carrying every tab's subscriptions |
| feed hub | the shell's holder of the union streams, in a SharedWorker or, where there is none, in the tab |
| lane | a timeline row defined by a query, with a render (interval, point, count) and an optional grouping field |
| series row | one drawable timeline row: lane, group, start and its shape |
| shape | what a series row draws: an interval, a point, or a count of rows in one bin |
| bin | the power-of-two span of time, aligned to the epoch, whose narrow rows one count row stands for |
| card | a boxed decision in a view whose keys act only while it is entered |
| card layer | the keys an entered card rebinds, shown on its buttons |
| preview | a fragment of the same view shown on hover or focus; it never follows a feed or writes a seen mark |
| page frame | the page layout: header, rails, main column, status strip, footer |
| inspector | the right rail showing the facts of the page or the focused target |
| palette | the list of every action available for the current selection |
| copy menu | the forms a target can be copied as |
| value display | how each value state is shown (`≥ X (N unknown)`, `≈ X`, `—` …) |
| counterpart | the same place in the other product, reached with `g o` |
| mark | the symbol that, with its word, carries a meaning that colour alone never does |

## Rankings

| Name | Meaning |
|---|---|
| judgment | one judge's ordering of targets, best first, with tie groups |
| tie group | targets a judgment places equally; one element of its order |
| judge set | which judges' judgments a fit counts: all, or a named set |
| fit | the scores, intervals, ranks, supports and components for one judge set |
| score | a target's log-worth on the fit's scale, relative to the prior's reference |
| interval | the 95 percent Laplace interval on a score |
| rank range | the best and worst positions the intervals allow |
| component | targets linked by counted judgments; scores compare across components only through the prior |
| warm start | a previous fit's scores used as the starting point of the next fit |
| proposal, seat | a proposed set of targets to compare, and one target in it with the reason it was chosen |
| uncompared | a target that no counted judgment names yet |

## Development runs

| Name | Meaning |
|---|---|
| run | one `xtask` invocation, recorded in the history store |
| run stage | one timed step of a run |
| tree | the Git tree object of a run's HEAD; with the hash of uncommitted changes it says what a run tested |
| fixture | a value `#[devkit::test]` creates for a test parameter |
| schema database, template | the read-only database compile-time SQL checks use, and the database tests clone |
| slot | one of a fixed number of test database names per project on a server, held through a file lock while a test uses it |
| kept failed database | one of a fixed number of names a failed test's database is renamed to for inspection, replacing the oldest |
| sample | corpus files drawn for a benchmark, written down in a manifest |
| quiet run | a bench run during which every host-load reading was below the thresholds |
| scenario | one exported NixOS check selected for a VM run |
| corpus | the input files a fuzz target starts with and grows |
| region | a source span and execution count reported by LLVM coverage |
| development stack | a checkout's owned services and selected application processes |
| development lock | an exclusive file lock held while a checkout's development stack runs |
| local dependency link | a Cargo patch to a checkout, with original workspace locks kept until unlinking |
