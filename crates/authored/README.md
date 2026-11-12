# authored

The rules every authored record follows in Polylogue and Sinex. An
**authored record** is anything a person or an agent writes: Polylogue's
assertions, pages, slots, variants and settings; Sinex's notes, library
curation records, signal rules and saved queries. Both products store their
records their own way; this crate holds the pure functions they share, so a
record is current, stale, a conflict or a duplicate by one definition.

The crate owns:

- the record **header** and the identities its links name;
- **currency** by the descendant rule, and the roots a write can affect;
- the **stale-base** check;
- **atomic requests**: several writes numbered `<request>.<n>`, placed
  together or not at all, with idempotent retries;
- **uniqueness slots**: one function for every kind's cardinality;
- **order keys** for ordered records;
- the in-memory history that both products compare their stored currency with.

Each product keeps its own storage and everything about its records that is
not a rule above: Polylogue's `link` table and its stored currency index,
kinds, value shapes and markup precedence; Sinex's request materials, the
library's shown-value precedence by origin level and its `library_shown`
projection.

## The header

```
{ root, rev, kind, key?, value?, author, via, at, revises: {revision…}, withdraws: {target…} }
```

| Field | Meaning |
|---|---|
| `root` | the ID of the record's first revision |
| `rev` | this revision's ID; equals `root` for the first revision |
| `kind`, `key`, `value` | what the record says; the product declares its kinds and their value shapes |
| `author`, `via` | who wrote it and the channel it arrived on, computed by the product from the authenticated caller; no request supplies them |
| `at` | the writer's clock in Unix milliseconds, for display and as-of reads; currency never reads it |
| `revises` | the revisions this one revises: of the same record, or of several records holding one uniqueness slot when the revision resolves their conflict |
| `withdraws` | the revisions or roots this record withdraws |

A revision is written `<root>@<rev>` (`Revision`), and a stored reference
`<type>:<root>@<rev>`: `assertion:<root>@<rev>` in Polylogue,
`sinex:curation/<root>@<rev>` in Sinex's global form. A withdrawal names a
revision or a whole root (`Target`). IDs are opaque to this crate; `@` is the
one character an ID may not contain.

## Currency

Revision R of a record is:

- **withdrawn** while a withdrawal that is not itself withdrawn names R or R's
  root;
- **current** when it is not withdrawn and no revision reachable from R
  through incoming `revises` links (its descendants, transitively) is not
  withdrawn.

Withdrawn revisions are therefore transparent. In R1←R2←R3, withdrawing R2
leaves R3 current and R1 not current; withdrawing R3 makes R2 current;
withdrawing the withdrawal of R2 restores it. Withdrawing a root withdraws
every revision. Two or more current revisions of one record are a
**conflict**, shown with every head and resolved by one revision that
`revises` every head. No head is ever chosen by time or write order.

An earlier statement of the rule, "R is current when no non-withdrawn
revision revises R", looks only at R's direct revisers: withdrawing R2 then
makes R1 current again beside R3, a false conflict. The descendant rule
replaces it in both products.

`Currency` evaluates the rule over a product's `Links` (revisions of a root,
revisers of a revision, withdrawers of a target, what a record withdraws),
remembering what it has computed. Withdrawals that withdraw each other have
no meaning and are reported as `CurrencyError::Cycle`, never resolved by
evaluation order; a write path cannot produce one, because a new record can
only name records that exist.

**Stored currency.** A product that keeps currency in an index (Polylogue's
`revision_state`) recomputes, in the writing transaction, exactly the roots
`affected_roots` returns for each new record: its own root, the roots of the
revisions it revises, the roots it withdraws, and through withdrawals of withdrawals the roots those name. The
index is then equal to the function by construction, like an SQL index, and
needs no rebuild or repair step.

## Stale base

A write with explicit bases (`revises`) arrives in one of two ways
(`OnStale`):

- **Refuse** (commands from the CLI, MCP, the web): every base must exist and
  be current, or the request is refused `Stale`, naming the current heads.
- **Fork** (markup, imports): the write is kept as another head, so the order
  of arrival cannot change the result; the fork is a visible conflict.

The bases of one write must belong to one record, or to several records that hold one uniqueness slot of the write: such records are one slot's conflicting values (each written first where the other was not yet seen, as two hosts or an imported bundle do), and one revision naming every head of every one of them resolves the conflict into the first record by root. Bases of records with no shared slot are refused `BasesSpanRoots`. The product decides which
view the check runs against: standalone, the committed links in the writing
transaction; in Polylogue's primary mode, the applied stream position plus
this host's unconfirmed writes.

## Requests

Every write arrives in a request `{id, writes}`. A `Plan` places the writes in
order against the product's links plus the request's own earlier writes:

1. Write n of a request of `count` writes gets the ID `<request>.<n>`
   (n from 1); a request of one write uses the request ID itself
   (`write_id`).
2. Withdrawal targets must exist.
3. A write with bases passes the stale-base check above. Under Refuse, the
   new content must not move into a uniqueness slot another record holds
   (`SlotTaken`).
4. A write without bases is placed by its uniqueness slots (below).
5. Every record carries the `Arrival`'s author, channel and time.

A later write sees the earlier ones, so one request can withdraw A and assert
B, assert a question's answer and judge it, or revise every slot of a page
edit. After any refusal the plan is abandoned: later writes and `finish` are
refused, so a partial request can never be committed. `finish` returns the
records, where each write went (`Placed::Written` with what it revised, or
`Placed::Already`) and the affected roots. The product commits the records in
one transaction.

**Retries.** The client chooses the request ID once per user action and
reuses it on retry. `RequestDigest` hashes the request's content with object
keys sorted, so the digest does not depend on map order or serializer
features, and `replay` classifies an arriving ID against the stored digest:
new, a repeat (answer the original outcome, write nothing) or a conflict
(the same ID with other content).

## Uniqueness

A kind declares its **uniqueness key** as an ordered list of component names
(`Uniqueness`), or none, which means a target may hold many. The kind is
always part of the key. The product supplies each component's value in a
write (`Part`), and `Uniqueness::unique` returns the **uniqueness slots** the
write occupies (in this section, "slot" means a uniqueness slot, never a
page slot):

| Part | Value in the slot |
|---|---|
| `Absent` | one slot with no value for the component |
| `One(v)` | `v` |
| `Each([v…])` | one slot per value: a record about two sessions holds a slot for each, so two records can never give one session two current values without either being a revision |
| `Set([v…])` | the values as one unordered value (a pair of items) |

When a write without bases arrives, the current records holding any of its
slots decide where it goes:

- none: a new record;
- one record: under `WhenHeld::Revise` the write revises every current head
  of that record (which also resolves a conflict); under `WhenHeld::Already`
  it writes nothing and answers "already" with the heads (a tag whose name and
  value are in the key);
- several records that all hold one of the write's slots: a conflict between
  records; under `WhenHeld::Revise` the write revises every current head of
  every one of them, resolving it into the first record by root, and under
  `WhenHeld::Already` it answers "already" with all their heads;
- several records holding different slots (a write about two sessions whose
  titles are separate records): refused `Ambiguous`, naming them; a forking
  channel writes a new record instead.

The product finds holders through `Holders`, typically an index on
`UniqueSlot::digest`, a 32-byte length-prefixed hash. Polylogue's components are
`kind`, `about`, `key`, `value`, `scope`, `author`, `rubric` and `machine`;
the library's are its target, tag, property, scale, author, pair, relation
and scope. Rules about the shape of a request, such as refusing a
single-valued kind over a query of more than one row, stay in each product.

## Order keys

Ordered records (page slots, collection entries, workspace panes) carry an
`OrderKey`: a fraction in (0, 1) written as base-62 digits after the point
(`0-9`, `A-Z`, `a-z`, which is also their byte order), with no trailing zero.
String order is therefore numeric order, and a key always exists strictly
between any two keys, so an insertion never rewrites a neighbour and there is
no precision limit.

`OrderKey::between(before, after, suffix)` returns the midpoint of the gap
followed by a suffix the caller draws at random. The midpoint is never a
prefix of `after`, so the suffixed key stays inside the gap, and two writers
inserting into one gap from the same snapshot get distinct keys. Equal keys,
which only imports can produce, are ordered by record root (`SortKey`).
`OrderKey::evenly(n)` gives `n` evenly spread keys for building a list at
once, such as converting another ordering.

The [`authored-generator`](../authored-generator/README.md) development crate
provides shared generated histories for the products' storage conformance tests.
