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
| group | the blobs the writer appends and syncs together before the product commits them |
| commit | making a group's extents real, in the same transaction as the rows that cite them |
| tail | bytes after a segment's last committed extent; truncated when the segment opens |
| punch | releasing a committed extent's bytes with a hole punch |
| member | one file inside a PAX tar blob, with its metadata and byte range |
| scan | the on-demand check of every frame of every segment, which reports and repairs nothing |

## Queries

| Name | Meaning |
|---|---|
| catalog | a product's units or tables, their fields, closed-set values and defaults |
| compiler | a product's translation of a parsed query into execution |
| unit | what a query reads rows of (a product's sessions, events, tables …) |
| condition | one test of a row; conditions side by side are joined by `AND` |
| stage | one `\|` step of a query |
| unknown | the third truth value: a condition over a missing value is neither true nor false |
| closed set | the fixed list of values a field may hold |
| namespace | the `pl.` or `sx.` prefix that says which product a unit belongs to |
| page size | how many rows travel in one response; never part of the question |
| cursor | the sealed key that continues one question from its last row |
| logical day | the day a person reads: from the start hour in the configured zone to the same hour the next day |
| day setting | the zone and start hour that define the logical day (`DaySetting`) |
| precision | how exact a time is: `instant` to `year`, or `bounded` with an end |
| coarse time | a time whose precision is a period rather than an instant |

## Operations and transport

| Name | Meaning |
|---|---|
| operation | one row of a product's operation table, reachable from every surface |
| operation table | the typed list of a product's operations from which the CLI, socket, HTTP and MCP are generated |
| request, answer | what a caller sends, and the typed value it gets back |
| outcome | what kind of answer came back (rows, empty, incomplete …), or a failure |
| failure, failure kind | a typed error, and its stable name on the wire |
| failure code | a product's precise failure name, registered once against a shared failure kind |
| caller scope | the server's authenticated identity namespace for request retries; never supplied in request JSON |
| exit code | the CLI's process status for an answer, one per failure kind |
| latency class | the group an operation belongs to for its default deadline: local, status, completion, read, search, aggregate |
| deadline | how long a request may run; a default any caller can lift |
| position | where a product's stream of changes stands; an event's ID |
| figure | one value in status, with its state, reason and age |
| figure state | `measured`, `none_found`, `not_measured`, `unavailable`, `gap` |
| verdict | the one-word health of a product: `healthy`, `degraded`, `unknown` |

## Signals

| Name | Meaning |
|---|---|
| signal rule | a watched query with a delivery rule |
| watch | a signal rule that reports changes as notices |
| occurrence, episode | the thing a row is about, keyed by the product, and one stretch of it |
| trigger | a rule revision paired with an occurrence and episode; fires at most once |
| baseline | the matches a rule starts from, which never fire |
| handoff | the moment a delivery leaves the evaluator |
| delivery | the record of one firing: rule, revision, rows, channel, outcome |
| notice, value, action | the three kinds of delivery: a message, a number written to a target, a command run |
| interrupting, passive | a notice shown as a desktop notification, or one that waits in status and briefs |
| interrupt budget | how many interrupting notices per hour rules not authored by the operator may cause |
| quiet hours | when interrupting notices wait |
| deliverer | the one process on a host that shows desktop notifications |
| shipped rule | a rule that comes with a product, replaceable by an authored rule of the same name |

## Web

| Name | Meaning |
|---|---|
| token | a named value of the shared visual language (`--surface`, `--failure-ink` …) |
| shell | the shared TypeScript that carries keys, palette, copy, live updates and windowed lists |
| fragment | server-rendered HTML for part of a page, sent live or fetched |
| page frame | the page layout: header, rails, main column, status strip, footer |
| inspector | the right rail showing the facts of the page or the focused target |
| palette | the list of every action available for the current selection |
| copy menu | the forms a target can be copied as |
| value state | how a value that is not a plain measured number is shown (unknown, partial, inferred …) |
| mark | the symbol that, with its word, carries a meaning that colour alone never does |

## Development runs

| Name | Meaning |
|---|---|
| run | one `xtask` invocation, recorded in the history store |
| run stage | one timed step of a run |
| tree | the Git tree object of a run's HEAD; with the hash of uncommitted changes it says what a run tested |
| fixture | a value `#[devkit::test]` creates for a test parameter |
| schema database, template | the read-only database compile-time SQL checks use, and the database tests clone |
| sample | corpus files drawn for a benchmark, written down in a manifest |
| quiet run | a bench run during which every host-load reading was below the thresholds |
| scenario | one exported NixOS check selected for a VM run |
| corpus | the input files a fuzz target starts with and grows |
| region | a source span and execution count reported by LLVM coverage |
| development stack | a checkout's owned services and selected application processes |
| development lock | an exclusive file lock held while a checkout's development stack runs |
| local dependency link | a Cargo patch to a checkout, with original workspace locks kept until unlinking |
| law | a property a page states under `## Laws`, with a stable key that test names carry |
