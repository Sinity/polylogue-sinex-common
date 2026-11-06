# query-language

The one surface language Polylogue and Sinex both speak. Both products accept
the same text with the same meaning; only the units differ. The crate holds
the grammar, the syntax tree and its meaning, the parser and formatter, the
binder and typing, the normal form and its hash, time expressions, the
logical day, the cursor type, the fusion of ranked lists, the federation
planner's types, the completer and the law suite every product compiler
runs. It is pure: no I/O, no clock and no configuration of its own; the
caller passes the time, the day setting and the catalog.

Each product keeps its own **catalog** (its units, their fields, closed-set
values, defaults, order scopes, search fields and extensions) and its own
**compiler** from the syntax tree to execution, and adds its own forms and
laws: Polylogue in
[its query page](https://github.com/Sinity/polylogue/blob/master/docs/query.md),
Sinex in [its query page](https://github.com/Sinity/sinex/blob/master/docs/query.md).

## Concepts

The language has seven concepts, and nothing else is a language concept:

- a **relation**: a unit, a saved query, a kept result or a reference (one
  row);
- a **condition**: a three-valued test of a row;
- a **transform**: one step after `|`, a relation in and a relation out,
  applied in written order;
- a **ranking**: ranked lists over the selected rows, fused into one order;
  it never changes which rows are in the answer;
- an **order relation**: `near`, `before`, `after`, `during` or
  `containing`, measured in time or in steps;
- the **read context**: `now`, the day setting, resolved times, the random
  seed, definition versions and each product's read position;
- the **answer**: one evaluation of a query, in the answer envelope of
  [command-surface](../command-surface/README.md#answers).

Recall, briefs, facets, watches, Today and the web's lists are queries, or
assemblies of queries that print the queries they ran.

## Spelling

One spelling in both products (default; open operator question): there is
no dialect switch, and a query written for one product parses with the same
meaning in the other.

- `AND`, `OR`, `NOT` in capitals; a leading `-` negates. Conditions written
  side by side are joined by `AND`; parentheses group anywhere. A lowercase
  `and`, `or` or `not` standing between two conditions is refused with the
  capitalised query as the suggestion; quoted, it is searched for.
- Keywords are positional, so no word is reserved: a field may be called
  `where` or `count`.
- Every transform follows `|`. The tabular output is a "table"; `page_size`
  is how many rows travel in one response.
- Aggregates take the function form: `agg sum(cost), p90(duration) as slow`.
- Sets are written `F:(a|b|c)`.
- `sort by random` and `sort by random()` both leave the seed unspecified;
  `random(N)` keeps the explicit unsigned seed.
- Percentiles are nearest-rank.
- Cursors are sealed keyset cursors.

Correction paths produce edits to the original question. The outer parser chains
those edits until the whole question parses, then checks the resulting tree
against the catalog once. An intermediate correction need not be runnable.

There is no command-name typo guessing. Ruling, 2026-07-05: "typo hint is
actually unnecessary given we do not want bare query support anyway"
(cc:900c6128-99a3-4986-8ed4-d5d7aacb38fe:e5f52f29-85e8-4cfc-9088-b5ea79b7bccc).
Near-miss suggestions for fields, units and transforms inside a query
remain.

## Shape

```
query     := [source] [conditions] [attach] { "|" transform }
source    := unit ["where"]
           | "query:" (NAME ["(" args ")"] | HASH)
           | "result:" ID [".matches"]
           | reference
unit      := NAME | ("pl" | "sx") "." NAME
cond      := cond "OR" cond | cond ["AND"] cond | "NOT" cond | "-" atom
           | "(" cond ")" | atom
atom      := field ":" value | field cmp value | field "~" text | field "^" text
           | field "in" range | field "between" v "and" v | field "in" "(" query ")"
           | "since:" time | "until:" time
           | term
           | "exists" unit "(" cond ")" | "count(" unit ["where" cond] ")" cmp N
           | order "(" query ")" ["within" distance] ["at" point] ["in" scope]
           | "seq(" step { "->" [gap] step } ")"
           | "in:" "result:" ID | "query:" NAME ["(" args ")"] | reference
           | rankatom
value     := word | "quoted" | "(" v "|" v … ")" | "*" | "unknown" | "known" | "$" param
cmp       := "=" | "!=" | "<" | "<=" | ">" | ">="
order     := "near" | "before" | "after" | "during" | "containing"
point     := "when" | "start" | "end" | "match.when" | "match.start" | "match.end"
distance  := duration | N "steps"
attach    := "with" child "(" fields ")" ["[" cond ["," ("first:"|"last:") N] "]"] { "," … }
transform := "where" cond | "sort by" keys | "limit" N | "offset" N | "first" N ["by" keys]
           | "distinct" ["by" f] | "group by" keys | "agg" aggs | "count" | "fill"
           | "fields" proj | "matches" | unit ["where" cond]
           | ("union" | "intersect" | "except" | "fuse" | "diff") "(" query ")"
           | "join" order "(" query ")" "within" distance | "join" "(" query ")" "on" f "=" g
           | "as of" (position | time) | "segment by" "(" query ")" | extension
```

Parenthesis and nesting depth is checked against 64: a bound on parser
recursion, not on the size of a query.

- **Values:** a word, a `"quoted"` string with backslash escapes, a set
  `(a|b|c)`, `*` (names the field without restricting it and drops its
  default), `unknown`, `known`, a `$` parameter.
- **Comparisons:** `> >= < <= = !=`; durations in `ms s m h d w`; money as
  `2.90` or `2.90usd`. The compact `field:>=N` is refused with
  `field >= N` as the suggestion.
- **Default unit.** Without a unit, the unit is the product's default
  (Polylogue `sessions`, Sinex `events`). A mixed query names every unit.
- **Namespaces.** Each catalog declares its product namespace and may declare
  a unit's singular form. Both resolve to the canonical unit name, preserving
  question identity. Inside a product its own units are unprefixed; the other
  product's units carry `pl.` or `sx.`.

### Match operators

Four operators, each with one meaning in both products:

| Operator | Meaning |
|---|---|
| `F:v` | the field's **natural match**, declared once per field kind in the catalog: equality for closed sets, IDs, booleans and numbers; the whole-component subtree for paths, case-sensitive; case-insensitive substring for short text; word search for prose; inside the period for times (`started:yesterday`); the same object for references; `tag:name[:value]` for tags |
| `F = v`, `!=`, `<`, `<=`, `>`, `>=` | exact comparison of the stored value, typed by field kind; `!=` is three-valued |
| `F ~ "s"` | contains (case-insensitive substring); for prose fields, through the trigram index |
| `F ^ "s"` | starts with; on path fields, is or lies under, by whole components |

- `F:unknown` is true where the value is unknown and `F:known` where it is
  known. Both are two-valued, so they select exactly the rows three-valued
  conditions skip.
- **Times.** `since:T` and `until:T` act on the unit's main time by overlap;
  `F in RANGE` acts on any named time field (`when in last week`);
  `F between A and B` takes two endpoints. A bare duration compared with `<`
  or `>` (`when > 7d`) is refused, naming the two correct forms, because it
  reads as the opposite of what it means.
- **Text terms** (bare word, `"phrase"`, `word*` prefix, `word~`
  approximate) search the unit's declared search fields.
- **Related rows:** `exists child(C)`; `count(UNIT [where C]) OP N`, which is
  two-valued; `F in (QUERY)`, whose inner query is a value set of the same
  type; `seq(A -> [next|within D] B …)`; `in:result:<id>`, membership in a
  kept result.
- **Expressions.** `+ - * /` over typed numbers in conditions and in
  `fields`, with dimensions checked at bind time: duration − duration is a
  duration, money / count is money, time − time is a duration, money +
  duration is refused. An unknown input makes the result unknown. Built-in
  `abs(X)` preserves a number, money or duration's dimension and propagates
  unknown; affine time is refused. Other built-in functions are the time buckets `hour`, `day`, `week`, `weekday`, `month`,
  `month_day` and `year` on the logical day, `len()`, and
  `case_eq(a, b, value)` and `glob(text, pattern)`; a product adds its
  own functions as catalog extensions. Built-in function names are
  case-insensitive at their call position and stored under their declared
  spelling (`DAY(when)` becomes `day(when)`). `case_eq` returns `value` when
  both comparison inputs are known and exactly equal, otherwise unknown. Its
  result retains the third argument's kind and physical lineage, including
  reference identity, time precision and discrete values. It composes with
  aggregates, for example `min(case_eq(position, 1, step.command))`.
  `glob` matches the whole text case-sensitively; `*` matches any sequence of
  characters. Every other character is literal, including `%`, `_` and
  backslash. Missing text or pattern produces unknown.

## Meaning

- **Three values.** A condition over an unknown value is neither true nor
  false: it satisfies neither the comparison nor its `NOT`. Unknown is never
  read as zero or empty.
- **Defaults bind once.** A unit's default filters apply at binding; after
  that, every condition only narrows. A condition on a defaulted field, or
  `F:*`, drops that default. A reference used as a condition means "this row"
  and drops the defaults for its own match.
- **Filters compose.** Successive filters equal their conjunction.
- **Grouped rows are a relation.** After `group by`, `where`, `sort`,
  `limit` and the following transforms act on the groups; there is no
  `having`. Grouped output carries the implicit `count` beside its keys.
- **Match order does not matter.** The order rows are enumerated in never
  changes which row a join, `exists`, sequence or order relation selects.
- **The page is not the answer.** Selecting and ordering exist only in the
  query text (`limit`, `sort`). `page_size`, the presentation budget and the
  deadline travel beside the query and never change the answer or its count.
- **Closed sets refuse.** A value outside a closed-set field's values is
  refused, naming the set and the unit where that value exists; it never
  yields an empty result.
- **Namespaces bind from the tree.** `pl.` and `sx.` bind from the parsed
  query's unit and operator nodes, never from the query string:
  `pl.messages where text ~ "sx.events"` reads only `pl.messages`.
- **Exact numbers.** Integer and decimal fields are validated and compared
  on their exact decimal text, with no machine-integer or float range and no
  exponent cap; the bound value keeps its original spelling. A product's
  store reports its own range errors.

## Transforms

| Transform | Meaning |
|---|---|
| `where C` | filter; after `group by` it filters groups |
| `sort by K [asc\|desc], …` | total order; unknowns last; the row identity is the final key. `sort by random[(seed)]`; `sort by rank` names the fused rank |
| `limit N`, `offset N` | selection; part of the question; no maximum |
| `first N [by K]` | per-group selection after `group by` |
| `group by K, … \| first N` | the first N original input rows of each group in their input order; their original fields and provenance remain available |
| `distinct [by F]` | deduplication over the declared identity or a field |
| `group by K, …` | grouped relation; keys are fields, expressions or time buckets |
| `agg A, …`, `count` | `count`, `sum(F)`, `avg(F)`, `min(F)`, `max(F)`, `pNN(F)`, `count_distinct(F)`, `histogram(F)`, `corr(F, G)` and `covered(start, end)`, the length of the union of the intervals, so overlapping intervals are not counted twice. Each reports `unknown_F` and `partial_F` |
| `fill` | after a time-bucketed group: every bucket of a finite range, each with the product's coverage state for it (below) |
| `fields F [as N], E as N, …` | projection and computed columns |
| `with CHILD(fields)[cond, first:k\|last:k]` | attached child lists; never changes the parents |
| `matches` | one row per step of each match of the relation's one top-level `exists` or `seq`: `parent` (the row that matched), `match` (its number within the parent), `position` (the step's place in the match), `step` (a reference to the matched row) and the step row's fields as `step.F`. A relation without such a pattern is refused, and one with two is ambiguous; a saved definition can carry its captured top-level match units in its declared semantic output; without that provenance it is refused. A kept result remains opaque and reads its retained `.matches` part |
| `UNIT [where C]` | switch to the related rows of another unit (`messages … \| sessions` lifts to the containing sessions) |
| `union (Q)`, `intersect (Q)`, `except (Q)` | set algebra over one unit, or over identical projected schemas |
| `fuse (Q)` | adds Q's order as one more ranked list without adding rows |
| `join REL (Q) within D`, `join (Q) on F = G` | pair relations: one row per matching pair, or the exact-key join (commit hash, URL, path, native ID) |
| `diff (Q)` | for two relations of one unit: the rows that `joined` and `left`, and those that `moved` in rank, with old and new rank. It uses the comparison the signals evaluator uses for `on_change`, so a watch's report and a `diff` always agree |
| `as of POSITION\|TIME` | read at an earlier read position; a unit whose product keeps no history for it refuses with `not_yet_available` |
| `segment by (Q)` | splits each order-scope instance at Q's rows into consecutive segments, one output row per segment, with its delimiting rows as `segment.start` and `segment.end` |
| extensions | a product's own transforms, declared in its catalog, never in the shared syntax tree |

**Coverage states for `fill`.** A bucket's state is one of `complete`,
`gap(reason)`, `unknown`, `outside_span`, `not_yet`, `excluded` or
`unavailable(cause)`; the same vocabulary describes a source's coverage of an
interval in both products. Polylogue answers `complete` inside its import
span and `outside_span` outside it. A bucket with no rows and a `gap` state
shows the gap, never `0`. The states are `coverage::CoverageState`, which
command-surface's answer envelope uses for its coverage facet; any state but
`complete` makes a count over the interval a lower bound.

## Order relations

One form covers time and steps:

```
REL ( QUERY ) [within DISTANCE] [at when|start|end] [in SCOPE]
REL      := near | before | after | during | containing
DISTANCE := DURATION | N steps
```

- **As a condition** it is a semijoin: an outer row is kept when at least one
  inner row stands in the relation, so matches never multiply rows. It
  exposes the selected witness as `match.` and `match.distance`, the signed distance in the distance's unit. An outer row whose
  time or position is unknown is unknown. It composes with `NOT` and `OR`
  like any condition. Every operand reads the immutable incoming row. After
  the whole predicate, exactly one positive order under top-level `AND`
  exposes its witness. Multiple orders, and orders under `OR` or `NOT`,
  retain any prior match namespace instead of replacing it.
- **As a transform,** `| join REL (QUERY) within D` returns one row per
  matching pair; the change of grain shows in the query and in its reading.
- **Steps** count positions in a unit's declared **order scope**: the
  sequence its rows have steps in (Polylogue's session timeline of messages
  and tool calls; a Sinex shell session's commands), set in
  `Unit.order_scope`; units naming one scope interleave. `before(Q)` keeps
  the rows earlier than a Q row and `after(Q)` the rows later than one, so
  `before(Q) within 1 step` is "immediately followed by a Q row", and `next.`
  and `previous.` are the one-step relations as fields. Steps never cross a
  scope instance or an order scope; steps with `during`, `containing` or `at`
  are refused, as is a unit without an order scope. `near` needs a distance.
- **Time** distance crosses scopes, units and products. Spans are
  half-open; a point inside a span is distinct from two spans overlapping.
  `at when` and `at start` select the outer row’s start; `at end` selects
  its end. The inner row retains its native period and start. The signed
  distance is the inner start minus that outer anchor. Choosing an anchor
  does not turn a coarse period into a point or bypass the whole-period
  window requirement.
- `seq` stays for an ordered pattern of three or more steps; its steps may be
  any units in the scope's order, and a step may carry `within N steps` or
  `within D`.

An order condition can append `pick first by EXPR [asc|desc], ...` to choose
its exposed witness. Without a pick clause it chooses the nearest witness
(precise before coarse, distance, then identity). Explicit keys use the inner
query's actual output; unknown keys sort last in either direction, and identity
breaks ties. Inner query ordering does not change witness selection. This
choice changes neither semijoin membership nor outer identity and ordering;
order joins still return all matching pairs.

Within a pick clause, `match.id`, `match.distance` and `match.coarse` describe
the current candidate, shadowing any prior match metadata on the inner row.
Distance is the signed selected-anchor offset (a duration, or a step count);
it is available to pick keys even without an explicit window. These local
metadata fields do not expand the ordinary selected output schema.

`matches` retains the actual outer relation's declared fields under `parent.`
with their original types and provenance. It adds no public `parent.id` when
the input declares none; a grouped parent offers only its actual group output.

## Ranking

Conditions select and ranking orders; ranking never changes membership or
`count`. Ruling, 2026-01-24, on embeddings: "Not as a filter, but as a
scoring criteria, to rank order items"
(cc:1d6c0b33-94c7-4a2a-9957-347e5383f5c5:0bdc45a3-7aca-41a6-9b70-6f6fb086bbbe).

- **Ranked lists.** A query's lists are `relevance` when text terms are
  present; each ranking atom (`similar:"text"`, `similar:REF`,
  `similar:(Q)`, `related:REF`, `around:T`) in a top-level `AND` position;
  each `fuse (Q)` operand; and each ranked operand of a `union` chain. A
  ranking atom anywhere else (under `OR` or `NOT`, inside `exists`) is
  `invalid_query` with the reason.
- **Fusion.** One n-ary reciprocal-rank fusion: a row's score is the sum,
  over the lists that rank it, of 1/(60 + rank) (Cormack, Clarke and
  Büttcher 2009), computed in `f64` with the largest contribution first, so
  two rows with the same ranks in different lists tie exactly. Ties go to the more recent row, then to identity. k = 60 is
  a declared constant shown in the row explanation, not a setting.
- **Exact early stop.** Fusion reads the lists one depth at a time. After
  depth d an unseen row's best possible score is the sum over lists of
  1/(60 + d + 1); once the page holds `page_size` rows whose scores all
  exceed that bound, the page is final (the threshold algorithm of Fagin,
  Lotem and Naor, specialised to this fusion). The order equals full fusion,
  and no row is dropped. The cursor carries each list's depth and partial
  scores, so the next page continues the lists.
- **Rows outside every list** follow the ranked rows in the unit's default
  order, counted as `unknown.rank`.
- `sort by` replaces ranking; `sort by rank` uses the fused rank as a key.
  An answer that only counts or groups computes no ranking.
- **A list that cannot run** (no query vector while model calls are off; the
  other product unreachable) is left out and named, and the answer is
  `degraded`. Keeping a ranked answer whose ranking is incomplete is
  refused, naming the list.

The crate holds fusion as a pure function over each list's cursor; each
product supplies its lists.

## Query objects

- **Normal form.** The bound syntax tree with every unit qualified by its
  namespace, surface references and saved names resolved to stored forms,
  scalars canonicalised by field type, commutative operands sorted, spans
  and spelling dropped, and relative times kept as written (`since:7d`, not
  the instant). Numeric-looking text stays text.
The normal form has an explicit semantic encoding, independent of Rust AST
serialization. Constructs with unchanged meanings retain their original identity
encoding. New meanings, including one row per matched step and projections at an
intermediate relation boundary, have distinct encodings. Source spans, known-output
markers and display spellings are absent; parameter declarations remain part of
the question.

- **`query:<hash>`** is the BLAKE3 of the normal form and its declared
  parameter types. One algorithm serves both products, and a mixed query has
  one hash. A name or a transport option is never part of the question.
- **`answer:<id>`** names one evaluation: its query, read context, computed
  author, outcome, count and exactness. The product appends the record
  after the answer is sent; a read never waits on it. `answers` and
  `queries` are units, and `@last` is the caller's latest answer.
- **Names** are authored records pointing at hashes. Saving again re-points
  the name; a name never changes an identity. A name two authors hold is
  `ambiguous` until it is qualified (`query:NAME@AUTHOR`). Parameters are
  typed scalars in value positions, substituted in the tree, never as text.
  A saved relation (no transforms) can stand where a condition can, as
  membership; a saved query with transforms or ranking is a source only
  transforms may follow.
- **`result:<id>`** is a kept answer with its read context, exactness,
  columns and rows (or rows by reference where the product can reproduce
  them `as of`), plus ranks and each list's contribution for ranked answers.
  Reading it reads the stored rows, never a re-run; only transforms may
  follow it.

Before execution a product binds the parsed tree: it supplies the output
schema of every saved and kept source it uses (their fields, main time and
row unit), and binding checks fields, types and closed sets against them.
Binding returns the typing of every scope, before and after each transform,
so a compiler never reconstructs types from names. Binding, formatting,
identity and completion all read one catalog snapshot, and no provider call
or I/O runs inside parsing, binding or deserialisation.

## References and catalog declarations

A reference is one literal node and, as a condition, means "this row". The
language's own forms need no declaration: `query:`, `result:`, `answer:<id>`,
`@last` and `@N`. Every other form is declared in the catalog's
`references`: a prefix with an optional marker that tells a reference from a
field with the same prefix (`tool:Bash` is a field, `tool:s#c` a call), a
whole word (`self`, `last`, `here`) or a bare hexadecimal identifier of a
minimum length. `sinex:` and `polylogue:` are global prefixes each product
declares.

A product's functions are declared in `functions` (name, arity, result kind)
beside the built-in ones. Its transforms and atoms are extensions declared
with typed slots (`Keyword`, `Value`, `Name`, `Field`, `Unit`, `Condition`,
`Query`, `Expression`, `Optional`, `List`); Polylogue's `measure` is a
`List(Value)` of selectors and an optional `by` with a `List(Expression)` of
keys. A unit has one name and no aliases.

## Errors

Every error carries `kind`, `message`, `position` (a byte offset, printed
with a caret line), `suggestion` (a corrected query that parses, binds and
runs as given, whenever one can be formed), `candidates`, `hint` and
`retryable`. The language's kinds are `invalid_query`, `unknown_field`,
`bad_value`, `not_a_query`, `ambiguous` and `stale_cursor`; a compiler adds
its own (`not_built`, `not_configured`) at its own boundary. A language error becomes a
command-surface failure in one conversion, `Failure::from_query_error(error, query)`
in command-surface (this crate stays pure and depends on neither), with one

exit code and HTTP status per kind
([failure table](../command-surface/README.md#failures-and-exit-codes)).

- An unknown field's, unit's or transform's `candidates` are the nearest
  known names (Damerau–Levenshtein distance 1 for names of four to six
  letters, 2 for longer ones), the nearest first; a closed-set refusal's are
  the set's values. `parse` offers a suggestion only once it parses.
- Former spellings are refused with the one spelling as the suggestion:
  a clause without `|`, `F = [a, b]`, `F exists`, `sum:F`, a lowercase
  boolean, `[within:D]`, a ranking atom under a top-level `OR`.
- At the end of an otherwise complete parenthesised condition, a missing
  closing parenthesis offers the balanced query. Quoted parentheses remain
  data; incomplete operands and random seeds never receive invented values.
- An unknown colon field such as `TODO:` is a field error, while `"TODO:"`
  is text.
- An empty answer is only the result of a read that succeeded, and comes
  with the per-condition funnel and the nearest relaxation; a failure never
  yields `empty`.

## Reading and explaining

- Every field, operator, transform, order relation and ranking atom declares
  a short phrase, and `read_as` assembles the query's reading from them
  without a model.
- `explain` shows the parse with spans, the defaults applied and dropped,
  each name with the hash it resolved to, resolved times, the ranking
  lists, a mixed query's parts and connectors, `query:<hash>` and
  `read_as`. With the plan, it adds each part's estimated rows per step and
  a deadline prediction; run with the plan, the actual rows and time beside
  the estimates.
- Explain and completion never write, build, embed or call a model.

## Cross-system queries

A query whose bound units span both namespaces is **mixed**. Polylogue's
daemon evaluates it, because it holds the AI text Sinex does not index;
Sinex's daemon forwards a mixed query to Polylogue's configured endpoint and
relays the answer. The crate holds the planner's types as pure values; the
executor is Polylogue's.

1. **Split** the bound tree into **parts**: maximal subtrees whose units all
   belong to one product.
2. **Connect** parts only through an order relation, `F in (Q)`,
   `join … on`, `exists` over the other product's unit, set algebra over
   projected rows, or `fuse (Q)`. Anything else across a namespace is
   `invalid_query` with the reason.
3. **Estimate** each part's rows from its product.
4. **Bind** the smaller part first and pass its join values to the other as
   a typed parameter: an interval set (each outer time widened by the
   distance) for order relations, a key set for `on` and `in`. Values never
   enter query text.
5. **Batch.** A large bind set travels in batches, all read at the one Sinex
   commit position the first batch pinned. The batch size is a transfer
   unit; the answer does not depend on it.
6. **Join** by the connector's meaning, then apply the transforms above the
   join, and fuse ranked lists from either side once.

The answer reports both products' read positions and lags. With Sinex
unreachable its parts are `degraded`, named, never empty; with Sinex not
configured, `sx.` units are refused as `not_configured`, naming the
configuration key. Set algebra across products needs identical projected
schemas, and a resulting row's identity is its side and source reference.
Keeping a mixed answer stores both sides' rows (Sinex's by reference) with
the pinned commit position.

## Time expressions

- **Relative:** `7d`, `2w`, `3h` count back from now in exact hours.
  `last 7d` is the elapsed half-open span from that instant to `now`;
  `last 7 days` is a calendar span. Decimal durations keep nanoseconds and
  refuse precision that cannot be represented.
- **Calendar:** ISO dates and datetimes, `today`, `yesterday`,
  `this|last week|month`, `last N days`, and a bare `HH:MM`, all on the
  logical day in the configured zone. A bare four-digit value is a calendar
  year; `20:26` and `T2026` are clock times.
- **Natural-language dates** (`monday`, `3 days ago`, `last tuesday`,
  `march`) resolve to one instant or range, shown in the query's reading. A
  bare weekday is its latest occurrence including today; `last WEEKDAY` is
  strictly earlier; a named month is that month of the current logical
  year; `last N days` includes the current logical day. A phrase or slash
  date with two readings is `ambiguous` and lists them.
- **Bounds:** dates are half-open whole days; `between` two dates includes
  both days. `since:DATE` starts at that logical day's start; `until:DATE`
  includes that whole day; `until:INSTANT` stops at that instant. These
  select spans by overlap. An explicit comparison keeps its operator's
  meaning: `when < DATE` stops at the day's start. `since` and `until` stay
  their own operations through formatting and identity.
- **Daylight saving:** a local time inside a skipped hour means the first
  instant after the gap; inside a repeated hour, the earlier instant, with a
  note.
- **Resolution:** a query resolves its times once, against one `now` and
  one zone, and records them in the answer. A cursor carries them, so every
  page of one question uses the same times.

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

A time carries its quality (`intrinsic`, `captured`, `file_mtime`,
`operator`, `unknown`; each product classifies its own evidence) and its
precision (`instant`, `second`, `minute`, `day`, `month`, `year`, or
`bounded` with an end). A coarse or bounded time satisfies a time condition
only when its whole period does, fails when none of it does, and is
otherwise unknown. In an order relation, a coarse time takes part only when
the window is at least as wide as its period (a date cannot establish
60-second proximity), then as its whole period; the match is marked coarse
and ranks after precise matches. A row whose time is unknown never matches a
time condition.

## Cursors

A cursor continues one question by key, never by offset. It holds the last
row's order-key values with the row's identity as the final key, the
resolved `now` and zone, the random seed, the product's read positions and
version bindings (schema, definitions, grants, ranking data), the ranked
lists' depths, and the question's hash. It is sealed with a key the product
holds: opaque to the caller and checked on use. The seal authenticates; it
does not encrypt.

- Only the page size may change between pages. The count is computed once
  per question and read position and carried in the cursor.
- A product resolving an immutable recorded run may authenticate the token
  before lookup with `CursorSeal::authenticate`. The resulting proof exposes
  only captured context. It must then bind to the question established by the
  stored object and validate the current request, grants, versions and read
  positions before obtaining a continuation. Captured context grants no authority.
- A cursor from a different question is `invalid_query`. A cursor whose read
  context the product no longer serves (a rebuilt index, a ranked cursor
  after the ranking data changed) is `stale_cursor`, never a silent restart.
- Order keys compare exactly: numbers as exact decimals, unknowns last in
  either direction, identity ascending last. A continuation resumes from the
  stored key even if the boundary row has since disappeared.
- A page may be empty and still carry a cursor; only a page without a next
  cursor is the end.

## Completion

The completer proposes what may come next at a grammar position (units,
fields, closed-set values, transforms, operators, saved and kept sources'
fields) from the catalog snapshot it is given. It does no I/O, so a product
runs it in its CLI process from a catalog file its daemon publishes, and
answers within a keystroke whether the daemon is up or not; Polylogue's
catalog snapshot includes Sinex's published catalog, so a mixed query
completes too. Inserting a candidate leaves text whose prefix parses; a
candidate inside a quoted value includes its quotes and escapes.

## What a product supplies

Its catalog (units, fields with their natural-match kind and unknown rule,
closed sets from its stored names, defaults, order scopes, search fields,
phrases for `read_as`, extensions); its compiler and each ranked list's
cursor; reference resolution against its data; `fill` coverage states and
`as of`; its read positions; the output schemas of its saved and kept
sources; the executor of mixed plans (Polylogue); and the laws that are its
own.

## Laws

The crate's suite runs against both catalogs, through each product's real
compiler; each product also runs its own laws. The reusable fixtures name outer and child numeric fields separately and put their source order in the query explicitly, so a product retains its own ordinary listing order. The reusable fixture time ticks are seconds, matching the explicit duration units in their queries; an adapter converts those ticks to its native timestamp representation. The reference evaluator, a
synthetic-row oracle, is the independent answer for relational mechanics,
order relations, ranking and federation; it refuses storage, time-bucket and
extension mechanics that need a product.

Stored keys carry the scope `ql.` with dots for the area: `order-semijoin`
is stored as `ql.order.semijoin`, `three-valued-filters` as
`ql.filter.three-valued`.

Language:

- **one-spelling**: every query example in both products' docs parses with
  the one grammar, and no catalog carries a dialect switch.
- **format-roundtrip**: formatting a parsed query and parsing it again gives
  the same normal form, including implicit time bounds, explicit random
  seeds and extension arguments; formatting twice gives the same text.
- **identity-spelling**: spellings with one normal form have one
  `query:<hash>` in both products; any changed condition changes it.
- **filter-composition**: `Q | where A | where B` returns the same rows as
  `Q | where A AND B`.
- **three-valued-filters**: a row whose value is unknown satisfies neither
  `C` nor `NOT C`; `F:unknown` and `F:known` partition the rows.
- **grouped-rows-are-a-relation**: `where`, `sort` and `limit` after
  `group by` act on groups, equal to the same operations over the grouped
  result as a table.
- **match-order-independent**: permuting the enumeration order of input rows
  never changes which rows `exists`, `seq`, an order relation or a join
  selects.
- **page-is-not-the-answer**: the rows and count of a question are the same
  for every `page_size` and every way of paging through it.
- **closed-sets-refuse**: a value outside a closed set is refused naming the
  set and the unit where the value exists.
- **generated-names**: a field the language generates (`match.distance`,
  `diff`'s `change`, `old_rank` and `new_rank`, `fill`'s `coverage` and
  `coverage_detail`) never hides a stored field in either catalog.
- **namespace-binding**: units bind only from tree nodes; namespace-like
  text inside values never adds a unit.
- **defaults-bound-once**: after binding, every further condition only
  narrows the rows.
- **lowercase-boolean**: lowercase `and`, `or`, `not` between conditions is
  refused with the capitalised query as the suggestion; quoted, it searches.
- **days-tile-time**: in every zone, consecutive logical days share their
  boundary and no instant lies in two days or none.
- **instant-in-its-day**: every instant lies within the bounds of
  `logical_day(instant)`.
- **coarse-times**: a coarse time satisfies a condition only by its whole
  period, takes part only in windows at least as wide as its period, and
  ranks after precise matches.
- **cursor-refusals**: a cursor from another question is `invalid_query`;
  one past its read context is `stale_cursor`.
- **cursor-keeps-rows**: rows whose order key did not change between pages
  are neither skipped nor repeated.

Order relations, ranking and transforms:

- **order-semijoin**: an order-relation condition never multiplies outer
  rows, and `count` equals the semijoin's.
- **order-join-grain**: `join REL` returns exactly the pairs whose semijoin
  keeps the outer row.
- **order-steps**: `within 1 step` equals the `next.` and `previous.`
  relations, and steps never cross a scope instance.
- **rank-membership**: adding, removing or reordering ranking atoms never
  changes membership or `count`.
- **rank-unranked-kept**: rows outside every list are returned after the
  ranked rows and counted as unknown.
- **rank-fusion-oracle**: the fused order equals an independent n-ary
  reciprocal-rank fusion evaluator.
- **rank-early-stop-exact**: the early-stop fusion returns the same page as
  full fusion for every page size.
- **union-fuses-once**: a chain of `union` and `fuse` at one level is one
  fusion.
- **diff-watch**: `A | diff (B)` reports exactly the membership, ranking and
  top-k changes the signals evaluator reports for B → A.
- **expr-dimensions**: arithmetic over incompatible dimensions is refused at
  bind time; unknown in, unknown out.
- **covered**: `covered(start, end)` equals the measure of the union of the
  intervals, against a sweep-line oracle over generated intervals.

Objects, errors and explaining:

- **saved-reuse**: a saved relation selects the same rows by name and
  inline.
- **result-reread**: `result:<id>` reads back unchanged after new data and
  after re-parsing.
- **error-suggestion-runs**: every `suggestion` parses, binds and runs as
  given on the same catalog.
- **error-never-empty**: a failure never yields `empty`; `empty` comes only
  from a successful read.
- **read-as-coverage**: every field, operator, transform, order relation
  and ranking atom has a phrase.
- **explain-pure**: explain and completion never write, build, embed or call
  a model.
- **complete-offline**: completion answers from the catalog file with the
  daemon down.
- **complete-parses**: inserting any completion candidate leaves text whose
  prefix parses.
- **execution-mode-parity**: CLI, socket, HTTP, MCP and briefs give the same
  rows, counts and refusals to the same query.

Cross-system, run by Polylogue's executor against a devkit Sinex:

- **mixed-split-equals-oracle**: federated execution equals the federation
  oracle (both datasets as plain rows, the mixed query evaluated without
  splitting).
- **mixed-batching**: the answer does not depend on the bind batch size, and
  every batch reads one Sinex commit position.
- **mixed-positions**: every mixed answer reports both products' positions
  and lags.
- **mixed-incomplete**: with Sinex stopped, `sx.` parts are `degraded` and
  named, never empty; with Sinex not configured, they are `not_configured`.
- **mixed-forward**: `sinexctl` given a mixed query returns the same answer
  as `polylogue` given it.
- **mixed-typed-binds**: bind values cross only as typed parameters; a value
  containing query punctuation stays one value.

`at match.when` selects the matched row's occurrence evidence, retaining any
uncertainty. `at match.start` and `at match.end` select independently declared
state endpoints; an occurrence's uncertainty bounds are not state endpoints.
An open state has no selected end. With matched selectors, `during` tests the
outer occurrence against the inner declared lifetime and orders states by their
start; `containing` tests the selected inner evidence against the outer declared
lifetime. State ends carry their declared inclusion rule. Products refuse a
selector when their source has no such capability; missing evidence in an
otherwise capable source remains unknown.
