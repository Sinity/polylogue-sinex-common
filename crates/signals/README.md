# signals

How Polylogue and Sinex tell the operator, and agents, about something they
did not ask for at that moment. A **signal rule** is a watched query with a
delivery rule. Both products use one rule schema, one evaluator with one set
of laws, one interrupt budget and one design for desktop notifications, so
the operator's attention is spent by one set of rules on each host.

Each product supplies what is its own: what a rule's query can read (its
catalog), its occurrence keys, where rules and deliveries are stored, its
outlets, and its table of shipped rules (on each product's operations or
interfaces page).

## The rule

| Field | Meaning |
|---|---|
| `id`, `revision` | stable identity and revision. Rules are insert-only versions, and each delivery names the revision that fired |
| `name` | display label, unique per product |
| `query` | the watched query: `query:<hash>` or a saved query's name (a product may accept more, such as Sinex's `analysis:<recipe>`) |
| `fires` | `on_new`: each new matching row, once. `on_change` with `what` = `membership` (default), `ranking` or `top_k` (with `k`). `on_threshold` with `value` and `compare` (`<`, `>`): once per crossing into the condition. `at` a local time: once per logical day |
| `for` | how long a row must keep matching before the rule fires, judged against the read-time clock. A rule with `for` is a current-state rule |
| `min_change` | for `on_change`: how many rows must join or leave before a report (default 1) |
| `deliver` | `notice`; `value` (a number written to a configured target); or `action` (a configured command run outside the daemon, with the triggering reference as its last argument) |
| `target` / `command` | for `value` and `action`: a name from the deployment's signal configuration. A rule can use only what the operator configured |
| `interrupt` | whether a notice is shown as a desktop notification. Default `false` for every author, so whether a rule interrupts never depends on who wrote it; shipped rules declare their own |
| `quiet_hours` | when interrupting notices wait; the global quiet hours apply when the rule names none |
| `rate_limit` | a count per window; deliveries past it go to the next brief, marked "rate limit" |
| `scope` | optional: the repository, session or source the notices concern, for routing to a brief |
| `enabled` | on or off |
| author, `via` | computed by the daemon, never supplied |

Every author may write, revise, enable and withdraw rules, agents included.

- A **watch** is a rule with `fires: on_change` and `deliver: notice`; there is
  no second watch mechanism.
- **`follow`** is the same evaluator delivered to an open connection, with
  nothing recorded.
- **Shipped rules** are data in the product's code, listed and read like
  authored rules, with no author record. An authored rule with the same name
  replaces the shipped one; withdrawing it brings the shipped one back;
  disabling is an authored revision with `enabled: false`. Shipped rules count
  as the operator's for the interrupt budget.
- No rule starts or resumes an agent session (default; open operator
  question): every agent run traces to an explicit address or launch.

## Evaluation

Each statement is a law of the evaluator, tested by the conformance suite that
each product runs against its own catalog and occurrence keys.

- **Logical occurrences.** Each row a rule sees carries a stable occurrence ID
  and an episode, keyed by the product (Polylogue: the logical work session;
  Sinex: the citation). A trigger is the pair (rule identity and revision,
  occurrence and episode), and a trigger fires at most once. Re-parsing,
  replaying or re-importing, which produce new readings of the same
  occurrence, never fire again.
- **No self-amplification.** A rule's input never includes deliveries made by
  signal rules, or text a product injected into an agent session, so a recap
  of a notice cannot fire a notice.
- **Baseline.** Rows observed before a rule starts form its baseline and never
  fire. A new revision cancels the rule's pending work and takes the current
  matches as a new baseline without replaying them. Withdrawing a rule cancels
  its pending work.
- **Causality.** Only observed rows count. A row's future effective time (an
  end, a `for` deadline) becomes a wakeup only once the row has been
  observed. A `for` duration starts no earlier than the row's observation, so
  a closed episode observed late never becomes a present-state reminder.
- **Current and historical.** A current-state rule (any rule with `for`, and
  any rule over live state) rechecks at handoff that the row still matches; a
  reminder whose condition stopped being true before handoff is cancelled. A
  historical rule's match survives the row's later end.
- **Duration.** A `for` rule fires at its deadline with no new data, and fires
  nothing if the row stops matching first. A new episode of the same
  occurrence rearms it.
- **Changes.** `on_change` compares complete evaluations of the watched query;
  the first is the baseline. Membership compares row sets, ranking compares
  order, `top_k` compares the first `k`. A change of scores alone, or of the
  displayed page alone, never fires. Each report says which of the three
  changed.
- **Minimum change and cooldown.** An `on_change` rule reports only when at
  least `min_change` rows joined or left. After a report, further changes
  within one hour wait and are summarised in the next report, largest change
  first. The cooldown is measured from the kept baseline's run time, so it
  survives restarts with no other record.
- **Thresholds.** `on_threshold` fires on a false-to-true crossing; the first
  sample is the baseline; recovery rearms it.
- **Incomplete while catching up.** A rule evaluated while the data it reads
  is behind (the product's status is not up to date for it) reports
  `incomplete`, never a change and never a firing.
- **Handoff rechecks.** At handoff the evaluator rechecks that the rule is
  enabled at that revision, the quiet hours and the interrupt budget.
- **Quiet hours.** An interrupting delivery inside quiet hours is held to
  their end; a passive notice is not held. The delivery's basis (the rows and
  their observation) is fixed when it matched, not when it is released. Rate
  limits count actual handoffs.
- **Downtime.** After the daemon was down, the evaluator takes in what was
  observed during the downtime without handing anything off for past
  instants, then evaluates overdue work at the resumed clock: a current
  condition still true is delivered once, now; a condition that ended during
  the downtime is not delivered.
- **Partition invariance.** Advancing the evaluator through any partition of
  a time interval produces the same handoffs as one advance over it.

## Delivery and outcome

Every firing is recorded as a **delivery**: the rule and revision, the
triggering occurrences with their observation, the time, the channel, the
outcome, and the reason for any downgrade. Deliveries are inputs of the
product that made them, so "why did this fire" is a query.

A **notice** is passive unless its rule interrupts. Passive notices go to the
product's status, the next brief in their scope, its change feed and its web
UI. **What interrupts:** a notice interrupts only when the operator would want
to stop what he is doing for it; everything else is passive.

**One interrupt budget.** Interrupting deliveries from rules the operator did
not author share one budget, `signal.interrupts_per_hour` (default 4). Past
it, such a delivery becomes passive and says "interrupt budget spent". The
operator's rules and shipped rules are outside the budget, so rules written by
agents can never silence an alert the operator asked for.

**One deliverer per host.** Where Polylogue runs on Sinex, Polylogue's
interrupting notices travel to Sinex with its other deliveries, and Sinex's
deliverer shows them under the one budget and quiet hours, which are then
Sinex's settings. Polylogue shows its own notices only when it runs alone, and
for the one notice that Sinex cannot deliver: Sinex being unreachable or
refusing Polylogue's writes.

## Desktop notifications

Ruling, 2026-10-28, on desktop notifications: "it sounds like good idea, just
needs to be designed elegantly to not be a kludgy mess"
(cc:ee52794c-8a9e-43b6-9fbc-9749c7e78e8f:3ae3257e-e70a-433a-be97-d1222670c284).

- **Transport:** the freedesktop Notifications interface
  (`org.freedesktop.Notifications`) on the session bus, with the product's
  name and icon. Only interrupting notices are sent.
- **One notification per occurrence.** The delivery identity maps to the
  server's notification ID. Later news about the same occurrence (a stall
  still going, a grown count, an `on_change` summary) calls `Notify` with
  `replaces_id`, so it changes in place instead of stacking. When a
  current-state condition clears, the notification is closed
  (`CloseNotification`).
- **Urgency:** `normal` for interrupting notices; `critical` (no expiry) only
  where captured data is being lost now, such as a stalled capture whose spool
  cannot grow.
- **Actions,** when the server reports the `actions` capability: the default
  action opens the notice's reference in the product's web UI; `acknowledge`
  marks the delivery seen. Actions run as the operator's acts with
  `via: desktop`. `ActionInvoked` and `NotificationClosed` are recorded on the
  delivery (seen, acknowledged, dismissed, expired). Without the capability,
  the body names the command to run.
- **Grouping:** notifications carry a category per product and rule. While one
  rule has several open occurrences, those after the first fold into one
  summary notification replaced in place ("3 sources unreadable"). When the
  budget is spent, one summary notification replaced in place counts the
  notices held passive.
- **Outcome:** an attempt starts `unknown`; a returned notification ID records
  `delivered`; an error records `not_sent:<reason>`, including
  `no_notification_server`. A `not_sent` attempt is retried under the same
  delivery identity when the notification service appears (a current-state
  notice rechecks its condition first). A `delivered` notice is never shown
  again automatically, and an `unknown` attempt (no answer within the
  deadline) is not resent. A slow notification server never holds a writer.
- **Optional route:** a command named in deployment configuration receives each
  interrupting notice as JSON on standard input, with a deadline, instead of
  or beside D-Bus (`route: dbus | command | both`, default `dbus`), for
  forwarding to a phone or another device. Exit 0 records `delivered`.

## Configuration

Deployment configuration (the products' Nix modules, configuration file or
environment) names the value targets, action commands and the notification
route; a rule can reach only those. Quiet hours and the interrupt budget are
changeable settings of the deliverer's product.

## Rust interface and product adoption

`Rule` contains only authored fields. Products stamp `ActiveRule.version`,
`author` and `via` from their write path and set `live` when the query reads
current state. `Rule::validate` checks deployment target and command names;
`effective_rules` overlays authored rules on shipped defaults by product and
name. An authored disabled revision overrides its shipped default too.

`FIELDS` is the authored field table for help and schema rendering. Identity
and authorship are separate because they must never be client supplied.
`fires` serializes as `"on_new"`, `{"on_change":{"what":"membership","k":null}}`,
`{"on_threshold":{"value":10,"compare":">"}}` or `{"at":{"minute":600}}`.
Durations accept milliseconds or strings such as `20m`; quiet hours are
`{"from":1320,"to":480}` in local civil minutes. Local times use the product's
configured logical day, resolved through the common query language's
`DaySetting`. `DayCalendar` is its shared adapter for both products, including
scheduled gaps, earlier folds, skipped dates and complete quiet intervals.
Repeated civil minutes cannot re-enter an interval whose earlier-fold end
already passed. `Calendar` remains the test seam. Neither reads the system
zone or environment. Duration text uses the query language's duration parser
and must resolve to whole milliseconds, the evaluator clock's precision.

A product queues a complete `Evaluation` for each watched query, with all
pages joined in query order. Its `at` is the observation checkpoint, and each
`Row` names the logical `Occurrence` and episode, its original observation,
its effective start and optional end, a reference, and evidence. A new reading
keeps that occurrence and episode. New stretches use a new episode. Delivered
signals and injected context are marked by `Origin` and excluded before any
mode evaluates. A complete evaluation replaces the current matches; an
incomplete evaluation preserves them and prevents handoffs until a complete
one arrives. A newly installed query is incomplete until its first complete
evaluation, including scheduled rules. Numeric threshold samples use `value`.

`Evaluator::advance` visits every known causal boundary; `resume` folds
observations during downtime and hands overdue work off at the resumed
clock. Products persist the serializable evaluator checkpoint with the
owning query/delivery state. Revisions replace the observed baseline and
cancel pending work; withdrawal removes it. Private intervals consume their
observations and cancel pending outward work, including quiet-held work.
The evaluator's deterministic delivery ID hashes the product, rule ID and
revision, occurrence and episode. Evidence and reading IDs never enter it.
Change and threshold crossings use evaluator episodes; scheduled rules use
the logical day. Keep consumed triggers when rebuilding indexes.

The host deliverer's one `HostPolicy` owns quiet hours, the `InterruptBudget`,
actual handoff rate limits, pending intents and consumed delivery IDs. Persist
it atomically with emitted deliveries. `Evaluator::advance` and `resume` apply
this policy for a standalone product. Runtime wakeups combine the evaluator's
and host policy's `next_due`; neither is a polling interval.

In primary mode Polylogue uses the trusted `EvaluationMode::Forward`, which
emits `FiredIntent`s and spends no host budget. Sinex submits those intents to
its one `HostPolicy`, alongside its own local rules. Its `EvaluationMode::Local`
receives a trusted forwarded-condition callback for other products; its own
conditions use `Evaluator::condition`. The callback returns `Matching`,
`Ended` or `Incomplete`, checking enabled revision, current condition and source
freshness. Incomplete intents wait; ended intents are consumed without an
outward handoff. These modes are Rust runtime choices and cannot be supplied
in an authored rule or deserialized from a client request. The forwarding
adapter obtains authorship from authenticated product context. Duplicate
submissions preserve the first immutable basis and cannot consume the host
budget twice, including after a checkpoint restore. Commit the source evaluator
checkpoint and forwarded intent to its durable outbox together; commit the host
policy checkpoint and emitted delivery together. Transport retries reuse that
intent identity. The desktop worker then owns per-route attempt receipts under
the same identity.

Products execute `value` and `action` handoffs through the configured target
and command, outside their writer. The action reference is the last command
argument. No signal command is an agent launch.

A durable effect starts with an unknown `DeliveryReceipt`. The desktop
[`signal_delivery::Deliverer`](../signal-delivery/README.md) additionally persists `NotificationState` and
per-route `NotificationReceipt`s. Call `begin`, commit the unknown receipts
and state, then call `send`. Restored unknown attempts cannot become eligible
for a send. The asynchronous `signal_delivery::run` worker enforces this order
with a persistence acknowledgement and handles service appearance and desktop
protocol signals. It receives committed notices through a channel, never
through a writer lock. Products apply returned desktop actions through their
operation table as operator acts with `via: desktop`. The worker receives a
fresh policy callback for every effect, including historical occurrences and grouped replacements. The adapter applies current authorization and visibility even when historical occurrence matching remains valid.

When adopting, Polylogue removes its old signal evaluator and `notify-send`
command path, maps logical sessions/notes to occurrences, and saves checkpoints
with deliveries. Sinex maps citations to occurrences and uses the same worker
for its own and forwarded deliveries. Both run the shared reference/law cases
against their catalog adapters and retain the delivery outlets their product
docs own. Rule target execution and product routing are adapters, not a second
evaluator or notification implementation.

## Laws

- **trigger-fires-once**: a trigger fires at most once, and new readings of
  the same occurrence never fire again.
- **no-self-amplification**: deliveries and injected text never appear in a
  rule's input.
- **baseline-never-fires**: rows observed before a rule starts never fire, and
  repeated advances over one instant fire nothing new.
- **revision-takes-new-baseline**: revising or withdrawing a rule cancels its
  pending work, and a new revision fires nothing for current matches.
- **only-observed-rows-wake**: a time not yet observed never adds a wakeup.
- **duration-from-observation**: a `for` rule fires at its deadline without
  new data, fires nothing if the row stops matching first, and never turns a
  late-observed closed episode into a present-state reminder.
- **current-state-rechecks-at-handoff**: a current-state reminder whose
  condition ended before handoff is cancelled; a historical match survives
  the row's end.
- **change-kinds-stay-apart**: membership, ranking and `top_k` reports each
  fire only on their own kind of change, and never on scores or paging alone.
- **min-change-and-cooldown**: fewer than `min_change` joins and leaves never
  report, and changes within the cooldown are summarised in the next report.
- **threshold-crossing-once**: a threshold fires once per false-to-true
  crossing.
- **incomplete-never-fires**: a rule over data that is behind reports
  `incomplete` and fires nothing.
- **quiet-hours-hold-interrupts-only**: quiet hours hold interrupting
  deliveries to their end and never passive ones, with the basis fixed at
  match time.
- **budget-spares-operator-rules**: past the budget, non-operator interrupts
  become passive with the stated reason; operator and shipped rules are never
  downgraded.
- **no-retroactive-handoff**: after downtime no handoff is stamped in the
  past, and an overdue current condition is delivered once at the resumed
  clock.
- **partition-invariance**: any partition of an interval gives the same
  handoffs as one advance over it.
- **one-deliverer-per-host**: with Polylogue on Sinex, an interrupting
  Polylogue notice is shown by Sinex's deliverer and Polylogue makes no
  notification call except when Sinex is unreachable.
- **notification-replaced-in-place**: a second notice about one occurrence
  replaces the first through `replaces_id`, and a cleared condition closes it.
- **not-sent-retried-once-unknown-never**: with no notification server, an
  interrupting notice records `not_sent:no_notification_server` and is
  retried once under the same identity when the server appears; an
  unanswered attempt stays `unknown` and is not resent.
- **slow-server-never-holds-writer**: a notification server that never answers
  does not delay any commit.

`Evaluator`, `HostPolicy` and `NotificationState` expose the complete serialized checkpoint through `JsonSchema`, including private nested state. Products embed these types and the typed notification receipts directly in their atomic signal commit DTOs. Authenticated host rechecks use `Condition` with the closed wire values `matching`, `ended` and `incomplete`.

## Pure checkpoint and platform worker

This crate has no desktop or asynchronous runtime dependency. `notifications`
owns the serialized notice, attempt, receipt and checkpoint, with pure methods
for each transition. Its checkpoint exposes read-only attempt access. Platform
connections, process execution and persistence acknowledgements belong to
[`signal-delivery`](../signal-delivery/README.md).

Attempt handoff imports the original immutable snapshot without `begin`.
Repeating that snapshot preserves receiver progress, including after restart;
a conflicting snapshot under the same delivery identity is refused. An unknown
route remains terminal. Only an explicit D-Bus failure may use its one service
appearance retry; command outcomes are never selected by that retry. Server
identity scopes replacement IDs and desktop actions.
