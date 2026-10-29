# query-language

The one surface language Polylogue and Sinex both speak. The crate holds the
grammar, the syntax tree and its meaning, the parser, the completer, time
expressions, the logical day, the cursor type, and the law suite every
product compiler runs. It is pure: no I/O, no clock and no configuration of
its own; the caller passes the time, the day setting and the catalog.

Each product keeps its own **catalog** (its units or tables, their fields,
closed-set values and defaults) and its own **compiler** from the syntax tree
to execution, and adds its own forms and laws: Polylogue in
[its query page](https://github.com/Sinity/polylogue/blob/master/docs/query.md),
Sinex in [its query page](https://github.com/Sinity/sinex/blob/master/docs/query.md).

## Spelling

- `AND`, `OR`, `NOT` in capitals; a leading `-` negates. Conditions written
  side by side are joined by `AND`; parentheses group anywhere.
- Keywords are positional, so no word is reserved: a field may be called
  `where` or `count`.
- `|` separates stages; the tabular output is a "table"; `page_size` is how
  many rows travel in one response.
- `~` means contains; `similar:` asks for semantic ranking.
- Percentiles are nearest-rank.
- Cursors are sealed keyset cursors.

## Shape

```
[UNIT [where CONDITIONS]] [| STAGE]...
```

- **Atoms:** `field:value`; `field OP value`; `field between A and B`;
  `field ~ "text"`; a text term (bare word, `"phrase"`, `word*` prefix,
  `word~` approximate); `exists UNIT(…)`; `count(UNIT [where …]) OP N`;
  `FIELD in (QUERY)`; `similar:TEXT` or `similar:REFERENCE`; a namespaced
  unit (`pl.sessions`, `sx.events`). A product's catalog adds its own atoms
  and stages.
- **Values:** a word, a `"quoted"` string with backslash escapes, a set
  `(a|b|c)`, `*` (names the field without restricting it and drops its
  default), `unknown`.
- **Comparisons:** `> >= < <= = !=`; durations in `ms s m h d w`; money as
  `2.90` or `2.90usd`. There is one spelling: the compact `field:>=N` is
  refused with `field >= N` as the suggestion.
- **Command words:** a query whose first unquoted word is a command or unit
  name is that command or unit, never a text search; quoting a word searches
  for it. A first word within two typing mistakes of a command or unit name is
  refused with the name as a suggestion (Damerau–Levenshtein distance 1 for
  words of four to six letters, 2 for seven or more; words under four letters
  never count). A field name is never taken for a search word.

## Meaning

- **Three values.** A condition over an unknown value is neither true nor
  false: it satisfies neither the comparison nor its `NOT`. Unknown is never
  read as zero or empty.
- **Filters compose.** Successive filters equal their conjunction.
- **Grouped rows are a relation.** After `group by`, `where`, `sort`, `limit`
  and the following stages act on the groups.
- **Match order does not matter.** The order rows are enumerated in never
  changes which row a join, `exists` or sequence selects.
- **The page is not the answer.** Selecting and ordering exist only in the
  query text (`limit`, `sort`). `page_size` and the cursor travel beside the
  query and never change the answer or its count.
- **Closed sets refuse.** A value outside a closed-set field's values is
  refused, naming the set and the unit where that value exists; it never
  yields an empty result.
- **Namespaces bind from the tree.** `pl.` and `sx.` bind from the parsed
  query's unit and operator nodes, never from the query string:
  `pl.messages where text ~ "sx.events"` reads only `pl.messages`. A
  `similar:` node stays semantic ranking and never becomes a temporal
  relation.

## Errors

Every error carries `kind`, `message`, `position` (a byte offset, printed with
a caret line), `suggestion` (a corrected query that runs as given, whenever
one can be formed), `hint` and `retryable`. The language's kinds are
`invalid_query`, `unknown_field`, `bad_value`, `not_a_query`, `ambiguous` and
`stale_cursor`; a compiler may add its own. Each kind maps to one exit code and one HTTP status in
[command-surface](../../docs/command-surface.md#failures-and-exit-codes).

## Time expressions

- **Relative:** `7d`, `2w`, `3h` count back from now in exact hours. `last 7d` is the elapsed half-open span from that instant to `now`; `last 7 days` is a calendar span. Decimal durations retain nanoseconds and refuse unrepresentable precision.
- **Calendar:** ISO dates and datetimes, `today`, `yesterday`,
  `this|last week|month`, `last N days`, and a bare `HH:MM`, all on the
  logical day in the configured zone.
- **Natural-language dates** (`monday`, `3 days ago`, `last tuesday`,
  `march`) resolve to one instant or range, shown in the query's explanation;
  an ambiguous phrase is an error listing the readings.
- **Natural-language choices:** a bare weekday means its latest occurrence including today; `last WEEKDAY` is strictly earlier. A named month means that month in the current logical year. `last N days` includes the current logical day. Slash dates with two distinct valid readings are `ambiguous` and list their ISO readings.
- **Bounds:** dates are half-open whole days; `between` two dates includes both days. `since DATE` (or `since:DATE`) starts at that logical day's start; `until DATE` (or `until:DATE`) includes that entire day and stops at its exclusive end. `until INSTANT` stops exclusively at that instant. These operations select spans by overlap. Explicit comparisons keep their operator meaning: `when < DATE` stops at the day's start.
- **Bound syntax:** `ConditionKind::Since` and `ConditionKind::Until` retain the catalog's main-time operand and the unresolved value. They remain distinct from `Compare` through serialization, formatting and canonical identity. Compilers use `Time::start()` for a lower bound or explicit comparison and `Time::until_bound()` for an exclusive `until` bound; they never infer an operation from source spans.
- **Daylight saving:** a local time inside a skipped hour means the first
  instant after the gap; inside a repeated hour, the earlier instant, with a
  note.
- **Resolution:** a query resolves its times once, against one `now` and one
  zone, and records them in the answer. A cursor carries them, so every page
  of one question uses the same times.

## The logical day

A **logical day** is the day a person reads: from a configured start hour in
a named zone to the same hour the next day. `DaySetting` is that pair, and it
has one implementation, used by queries, SQL generation, derivations,
analysis and rendering in both products. Ruling, 2026-10-11: "it should be
configurable, and default should be sth like 4AM maybe … need to design the
system so that this works reliably, there are no unexpected exceptions"
(cc:e3e37f29-2490-47ae-89ef-fd40b6242e9a:d716993a-f5d6-4422-aa5e-97d91e430d93).

- The zone is an IANA name and travels with its name, so generated SQL never
  falls back to UTC. A `DaySetting` is constructed only from a resolved zone
  and a valid start; there is no default constructor, and the deployment's
  value is passed in. Where it is configured, and its default of 04:00, is
  each product's operations page.
- Logical date D covers `[start_of(D), start_of(D+1))`. A start inside a
  skipped hour moves to the first instant that exists; a start inside a
  repeated hour uses its first occurrence. Days around a clock change are 23
  or 25 hours long (23.5 and 24.5 in zones with 30-minute changes), and a
  date the zone skipped entirely is an empty interval.
- `logical_day(instant)` is derived from `start_of`, so the two cannot
  disagree. `days_between` counts date labels, not elapsed days. A boundary
  that cannot be represented is an error, never clipped.
- Every day- and hour-grain value records the boundary and zone it was cut
  with, so a changed setting is visible and its products are recomputed by
  the product's ordinary version mechanism.

## Coarse times

`time::TimeQuality` is the shared closed set `intrinsic`, `captured`, `file_mtime`, `operator`, `unknown`; each product classifies its own producer evidence. Quality and precision are separate. A time carries its precision: `instant`, `second`, `minute`, `day`, `month`,
`year`, or `bounded` with an end. A coarse or bounded time satisfies a time
condition only when its whole period does, fails when none of its period
does, and is otherwise unknown. In a temporal join, a coarse time takes part
only when the join's window is at least as wide as its period (a date cannot
establish 60-second proximity), then as its whole period; the match is marked
coarse and ranks after precise matches. A row whose time is unknown never
matches a time condition.

## Cursors

A cursor continues one question by key, never by offset. It holds the last
row's order-key values with the row's identity as the final key, the resolved
`now` and zone, the random seed, the product's read position, and a hash of
the resolved question. It is sealed: opaque to the caller and checked on use.

- Only the page size may change between pages.
- A cursor from a different question is `invalid_query`. A cursor whose read
  position the product no longer serves (a rebuilt index, a ranked cursor
  after the ranking data changed) is `stale_cursor`, never a silent restart.
- A page may be empty and still carry a cursor; only a page without a next
  cursor is the end.

## Saved and kept output binding

`Source::Saved` and `Source::Result` carry `OutputBinding::Deferred` until the product supplies their output in the immutable catalog's `source_outputs`. A `SourceOutput` identifies the saved name and arguments, or kept reference and `ResultPart`, and provides an `OutputSchema` of typed fields, optional main time and optional actual row unit. Products derive these schemas from expanded saved definitions or stored result columns. The parser performs no reads and never borrows the default unit's fields for either source.

A deferred tree is syntax only. Before execution, the product calls `bind::bind` with the derived snapshot; the resulting `BoundQuery` contains a tree whose saved and kept scopes are all known, including nested sources. Binding reuses the shared parser's field, type and closed-set checks and refuses missing schemas, unknown suffix fields and invalid values. Products use that same snapshot for canonical identity. The binding marker is validation state and does not enter question identity; the `since`, `until`, source identity and kept result part do.

Inline conditions on saved and kept sources are stored in their source's `condition`, separately from a piped `StageKind::Where`. This preserves the product's distinction between conditions applied to an expanded base relation and filters applied after that relation's output has been selected.

`Source::Unit.implicit` records whether the unit was omitted. An omitted nested source inherits its enclosing actual row unit and uses that unit's underlying catalog fields, even after grouping or projection; an explicit unit keeps its own row domain. Products supply `OutputSchema.unit` when a saved or kept source can contain such nested questions. Missing source schemas or unresolved inherited row domains cannot bind. Formatting preserves the omission, transport carries the marker, and canonical identity drops it once the actual unit is known.

A kept reference's `.rows` or `.matches` selector is a typed `ResultPart`. A quoted identity retains those characters as literal data: `result:"example.matches"` reads rows from that identity, while `result:"example".matches` selects matches from `example`.

## Completion

The completer proposes what may come next at a grammar position (units,
fields, closed-set values, stages, operators) from the catalog it is given.
It does no I/O, so a product can run it inside its CLI process from a catalog
file its daemon publishes, and answer within a keystroke whether the daemon
is up or not.

## What a product supplies

Its catalog (units or tables, fields with their meaning and unknown rule,
closed sets, defaults), its compiler, its own atoms and stages, its read
position for cursors, and the laws that are its own.

## Library and adoption

`parse::parse(text, &catalog)` returns an owned, byte-spanned `ast::Query`. The catalog holds unit names and aliases, field kinds, closed sets, defaults, identity keys, default order, reference forms, the sequence and recall-last domains, and structurally typed extension declarations. `catalog::STAGES`, `AGGREGATES` and `KEY_FUNCTIONS` expose the grammar declarations for product discovery and completion. Extension arguments are values, fields, units, conditions or subqueries; a compiler must implement or refuse each node. Namespace discovery walks those nodes and never reads literal text.

`format::format_query(query, catalog)` emits shared surface text from typed syntax, safely quotes values and preserves conditions, stages and extension slots. It reparses the result against the same catalog and checks canonical identity, refusing a manually constructed tree that has no equivalent surface spelling. Transport-law adapters use this renderer to run the shared cases through their actual text interfaces.

`time::Clock` fixes the question's `now` and `DaySetting`. `time::resolve` returns the original expression, its instant or half-open range, and any daylight-saving notes. Pass these resolved values onward to execution and every continuation. `DaySetting` is constructed from a caller-resolved named zone. `DaySettingWire`, `ClockWire` and `TimeValueWire` deserialize as pure records, then their `resolve` methods validate explicitly supplied resolved zones and construct runtime values. The runtime types serialize to those records and never resolve a zone implicitly.

`identity::normal_form(query, catalog)` drops source spans and scalar quoting, canonicalizes scalar spellings using their declared field types, sorts commutative boolean operands and sets, and preserves stage order and relative time expressions. Numeric-looking text stays text. `identity::question_id(query, catalog, parameter_types)` hashes this form together with declared parameter types using BLAKE3. Products resolve their field shorthands, saved names and surface references to canonical stored identities before hashing; a name or transport option is never a parameter type or part of the question.

`cursor::CursorContext` binds that question identity, clock, resolved times, seed, product read positions and version bindings. Products put schema, definitions, grants and ranking-data versions in its version map. `CursorSeal` authenticates the opaque wire token with a caller-supplied 32-byte secret and checks the seal before reading its payload. Authentication does not encrypt the payload. `open` checks the question, obtains the named zone through an explicit caller callback, then asks the product whether its bound read context remains available. The callback supplies a resolved zone; the crate never consults a global zone database. A missing or mismatched cursor zone is `stale_cursor`. Keep the sealing key outside queries and request logs.

`laws::check` runs the compiler/executor cases against independently specified answers, permutations and page sizes. `laws::check_modes` runs the same cases through caller-supplied execution modes and names a failing mode; `laws::check_refusals` checks catalog/compiler refusals. The reference interpreter is a synthetic-row oracle for the common relational mechanics. It explicitly refuses storage, ranking, time-bucket, saved-definition, coverage and extension mechanics that need product execution.

The continuation records sort values and a separate row identity, which is the final tie-break. `cursor::compare_positions` defines compatible scalar ordering, exact decimal numeric comparisons, unknowns last in either direction, and the final ascending identity key. It continues from the stored key even if the boundary row has since disappeared. `PageRequest` carries a nonzero page size and optional continuation beside the query. Shared cursor, `ClockWire` and resolved-time wire types implement `schemars::JsonSchema`, so operation envelopes can embed them directly; cursors are schema strings. A product may return an empty page with a continuation; exhaustion is the absence of the next continuation.

Polylogue's adoption replaces the grammar in `polylogue-query` while retaining the product compiler and its catalog of authored records. Convert its units and field registry into `Catalog`, translate the shared nodes into the existing SQL compiler, and remove its predecessor parser, AST, time resolver and page cursor in the same product change. Preserve product-specific reference resolution, defaults, saved-definition lookup, ranking and measure compilation at their existing seams.

Sinex's adoption replaces parsing and time mechanics in `sinex-model::query` while keeping table/schema binding and execution in its model/store layers. Build the catalog from its table and alias declarations plus registered payload schemas, and compile the shared AST directly. Preserve typed payload checks, privacy/read context, fill evidence and provenance walks in Sinex. Retire its predecessor lexer, parser, time resolver and cursor encoding when their connected callers convert. Both products run their compiler against the shared law harness in addition to their own integration tests; a parsing test alone does not establish execution parity.

## Laws

The crate's law suite runs against both catalogs; each compiler runs it plus
its own laws.

- **filter-composition**: `Q | where A | where B` returns the same rows as
  `Q | where A AND B`.
- **three-valued-filters**: a row whose value is unknown satisfies neither
  `field OP v` nor `NOT field OP v`.
- **grouped-rows-are-a-relation**: `where`, `sort` and `limit` after
  `group by` act on groups, equal to the same operations over the grouped
  result as a table.
- **match-order-independent**: permuting the enumeration order of input rows
  never changes which rows a join, `exists` or sequence selects.
- **execution-mode-parity**: CLI, socket, HTTP, MCP and briefs give the same
  rows, counts and refusals to the same query.
- **command-words**: a first word that is a command or unit name runs it; one
  within the typing-mistake distance is refused with the suggestion; a quoted
  word searches; a field name is never a search word.
- **page-is-not-the-answer**: the rows and count of a question are the same
  for every `page_size` and every way of paging through it.
- **closed-sets-refuse**: a value outside a closed set is refused naming the
  set and the unit where the value exists.
- **coarse-times**: a coarse time satisfies a condition only by its whole
  period, joins only windows at least as wide as its period, and ranks after
  precise matches.
- **namespace-binding**: units bind only from tree nodes; namespace-like text
  inside values never adds a unit.
- **days-tile-time**: in every zone, consecutive logical days share their
  boundary and no instant lies in two days or none.
- **instant-in-its-day**: every instant lies within the bounds of
  `logical_day(instant)`.
- **cursor-refusals**: a cursor from another question is `invalid_query`; one
  past its read position is `stale_cursor`.
- **cursor-keeps-rows**: rows whose order key did not change between pages
  are neither skipped nor repeated.
