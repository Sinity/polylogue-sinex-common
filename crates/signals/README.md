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
| `deliver` | `notice`; `value` (a number written to a configured target); `action` (a configured command run outside the daemon, with the triggering reference as its last argument); or `ask` with `instrument: NAME`, a notice that carries a self-report question and accepts an answer ([elicitation](#elicitation); decided target, not built) |
| `target` / `command` | for `value` and `action`: a name from the deployment's signal configuration. A rule can use only what the operator configured |
| `interrupt` | whether a notice is shown as a desktop notification. Default `false` for every author, so whether a rule interrupts never depends on who wrote it; shipped rules declare their own |
| `quiet_hours` | when interrupting notices wait; the global quiet hours apply when the rule names none |
| `rate_limit` | a count per window; deliveries past it go to the next brief, marked "rate limit" |
| `fresh_within` | how old a triggering fact may be and still interrupt; default the rule's own window (`for`, the threshold's window) or 15 minutes for rules that fire on single rows. An older fact, such as one a device held while it was offline, is delivered passively ([late facts](#the-route-function)) |
| `scope` | optional: the repository, session or source the notices concern, for routing to a brief |
| `to` | optional recipients besides the operator's inbox: `session:REF`, `@alias` or `role:NAME`; a session query is frozen into its sessions when the rule is written |
| `enabled` | on or off |
| author, `via` | computed by the daemon, never supplied |

Every author may write, revise, enable and withdraw rules, agents included.
A rule is an authored record, written through each product's ordinary write
verb, never through a separate signals command: `polylogue QUERY watch`,
`polylogue assert signal:NAME …` and `sinexctl write signal:NAME …` write
rules; acknowledging and snoozing a delivery are acts; `QUERY wait` and
`signal:NAME backtest` are queries. Over MCP these are rows of each product's
write, act and query tools, and `wait` is an option of the query tool.

- A **watch** is a rule with `fires: on_change` and `deliver: notice`; there is
  no second watch mechanism.
- **`follow`** is the same evaluator delivered to an open connection, with
  nothing recorded and no baseline: rows already matching are its first
  messages. **`wait`** is follow with an exit, and **`backtest`** replays a
  rule over stored history (below).
- **Shipped rules** are data in the product's code, listed and read like
  authored rules, with no author record. An authored rule with the same name
  replaces the shipped one; withdrawing it brings the shipped one back;
  disabling is an authored revision with `enabled: false`. Shipped rules count
  as the operator's for the interrupt budget. Only a shipped rule declares an
  `urgency` (`critical` only where captured data is being lost now) or
  `inhibits` other rules; an authored revision of a shipped rule loses both.
- No delivery starts or resumes an agent session (default; open operator
  question): every agent run traces to an explicit address or launch. The
  product names its agent launchers, and `refuse_agent_launchers` refuses a
  configured action command that is one of them, at startup.

## Evaluation

Each statement is a law of the evaluator, tested by the conformance suite that
each product runs against its own catalog and occurrence keys.

- **Logical occurrences.** Each row a rule sees carries a stable occurrence ID
  and a condition period (one stretch of it), keyed by the product
  (Polylogue: the logical work session; Sinex: the citation). A trigger is the pair (rule identity and revision,
  occurrence and period), and a trigger fires at most once. Re-parsing,
  replaying or re-importing, which produce new readings of the same
  occurrence, never fire again.
- **No self-amplification.** A rule's input never includes deliveries made by
  signal rules, or text a product injected into an agent session, so a recap
  of a notice cannot fire a notice.
- **Baseline.** Rows observed before a rule starts form its baseline and never
  fire. A new revision cancels the rule's pending work and takes the current
  matches as a new baseline without replaying them. `replace_rule` takes the
  owner's captured observation instant explicitly. Fresh adoption must not
  precede the evaluator checkpoint; a scheduled revision starts at that instant,
  even if the checkpoint has not advanced during idle time. Adoption does not advance
  or fire the old revision. Identical active-revision retries preserve the
  original baseline and held occurrences, including a retry whose frozen
  observation now precedes the checkpoint; a changed body requires a newer
  revision. Observations for a revision cannot precede its adoption. Withdrawing
  a rule cancels its pending work.
- **Causality.** Only observed rows count. A row's future effective time (an
  end, a `for` deadline) becomes a wakeup only once the row has been
  observed. A `for` duration starts no earlier than the row's observation, so
  a closed period observed late never becomes a present-state reminder.
- **Current and historical.** A current-state rule (any rule with `for`, and
  any rule over live state) rechecks at handoff that the row still matches; a
  reminder whose condition stopped being true before handoff is cancelled. A
  historical rule's match survives the row's later end.
- **Duration.** A `for` rule fires at its deadline with no new data, and fires
  nothing if the row stops matching first. A new period of the same
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
- **Handoff rechecks.** At handoff the host policy rechecks that the rule is
  enabled at that revision, that a current-state condition has not cleared,
  that the producer is up to date, the quiet hours and the interrupt budget.
- **Quiet hours.** An interrupting delivery inside quiet hours is held to
  their end; a passive notice is not held. The delivery's basis (the rows and
  their observation) is fixed when it matched, not when it is released. Rate
  limits count actual handoffs.
- **Downtime.** After the daemon was down, the evaluator takes in what was
  observed during the downtime without handing anything off for past
  instants, then evaluates overdue work at the resumed clock: a current
  condition still true is delivered once, now; a condition that ended during
  the downtime is not delivered.
- **Rebuilding a lost checkpoint.** `Evaluator::rebuild` starts from the
  rules and the triggers of the deliveries already stored, then replays
  stored observations; it never produces a delivery identity that exists. A
  threshold rule whose condition is already true when rebuilt cannot tell a
  missed crossing from its baseline and reports `rearmed_late`.
- **Partition invariance.** Advancing the evaluator through any partition of
  a time interval produces the same handoffs as one advance over it.

## Reports and the host deliverer

The evaluator never shows anything. Each advance produces **reports**: a rule
fired (`Fired`, with its basis fixed at match time), a current-state
condition cleared (`Cleared`), a rule's active revision (`Rule`) or
withdrawal (`Withdrawn`), a delivery retracted (`Retracted`), and the
product's **progress** (`Progress`: up to date as of an instant, except the
rules whose data is behind).

A product whose writer refuses a delivery because its cited basis changed (a
privacy policy epoch, a cited reading archived by a replay) calls
`Evaluator::retract(delivery, at)`. At `at`, after the observations up to
then, the evaluator forgets that one firing and reports `Retracted`; the host
policy drops its held or routed copy and routes again any consequences folded
under it. The same evaluation fires the trigger again, under the same
identity and from the current rows, if the rule still finds it: an
occurrence still in a new-row rule's result, including a current-state
condition still true. A threshold, change or daily firing is keyed by its
crossing, change or day, so it fires again only on a new one. The rule's
other deliveries are untouched, and budget and rate-limit charges of an
earlier handoff stand.

The **host policy** (`HostPolicy`) is the one deliverer on a host. It takes in
reports from every product's evaluator and notices from other producers,
holds what must wait, rechecks at handoff, applies the rate limits and the
interrupt budget, and routes each notice:

- **Standalone,** a product advances its evaluator with its own host policy
  (`Evaluator::advance`), at every causal boundary of both, so any partition
  of an interval gives the same handoffs.
- **Primary mode,** Polylogue advances without a host (`Evaluator::reports`)
  and sends its reports to Sinex as ordinary facts; Sinex's host policy takes
  them in with its own. A forwarded current-state notice whose producer has
  not confirmed being up to date through the instant it fired is held as
  `incomplete`, never shown; once the producer has, the notice is routed at
  any later handoff, since an ended condition arrives as `Cleared`. Polylogue shows one notice itself: Sinex being unreachable or
  refusing its writes, because that path is what is broken.
- **Other producers** (`sinexctl notify`) submit a `ProducerNotice`: a
  delivery of the implicit rule `producer:<name>` with author class
  `producer`, inside the budget and passive unless it asks to interrupt. A
  notice with the same `key` replaces the earlier notification in place;
  `clears: KEY` closes it.

There is no transfer of attempts between owners and no per-effect policy
callback: deliveries travel as facts, conditions end with `Cleared`, and the
policy decides once, at handoff.

## The route function

`route(notice, attention) → show | send-away | hold(until, reason) |
passive(reason)` is a pure function. Every delivery stores the decision, the
attention it was taken under and its holds, so "why did this interrupt me"
and "why didn't it" are both answered by the record (`Delivery::why`:
`held 11m (deep work) · budget 2 of 4 this hour · shown`). The first rule
that applies decides:

1. Not interrupting: `passive (rule is passive)`.
2. Folded into an open root cause: `passive (caused by delivery:X)`.
3. Late: the triggering fact's own time is older than the rule's
   `fresh_within` when the rule fires (it arrived late, for example from a
   producer that held it while out of contact): `passive (late, held N)`.
   The delivery is recorded and states its lateness; a week of held facts
   never becomes a wall of interruptions on reconnection.
4. `critical`: shown now, and also sent away if the operator is away and an
   away command is configured. Never held, never deferred.
5. An agent's or producer's notice with the budget spent:
   `passive (interrupt budget spent)`.
6. Inside quiet hours (the rule's own, else the host's) or an explicit
   `quiet until T`: held until their end.
7. The subject is on screen (one of the notice's references is in the
   focused window): `passive (on screen)`, never counted as seen.
8. The operator is away: sent away when an away command exists, otherwise
   held until presence returns and released into one summary.
9. Deep work: held for the next breakpoint, at most 25 minutes from when it
   first waited (`DeepWorkDeferral`: agents and producers, all notices, or
   never). Only agents' and producers' notices wait (default; open operator
   question).
10. Otherwise shown.

**Attention** is the deliverer's snapshot: an explicit quiet period,
presence, the focused window's references, deep work, whether a breakpoint
happens now, the budget used, whether an away command exists, and the
deep-work setting. Products feed it as timed `AttentionInput` facts, which
apply at their instant like evaluations. Every input has an unknown value,
`unknown` presence never counts as away, and with every input unknown
routing is quiet hours and the budget alone: attention only removes
interruptions.

Value and action deliveries are not routed; their decision is `run`.

## Inhibition and grouping

A shipped rule may declare `inhibits: [rule names]` of its own product, in
the scope `host` or `source` (the delivery's `scope`). While it has an open
current-state occurrence (held or shown), interrupting deliveries of those
rules are recorded and routed `passive (caused by delivery:X)`, and the
product renders them as lines of the root's notification. When the root
clears, inhibited occurrences that are still current are shown at once as one
grouped notification. Inhibition never changes whether a delivery is
recorded, only whether it interrupts. Authored rules neither inhibit nor are
exempt.

Notifications are grouped as before: one per occurrence, updated with
`replaces_id`; a rule's further open occurrences fold into one summary
notification. **Summaries** (`Summary`) stand for several deliveries and are
replaced in place by the next of their kind: the budget summary (notices held
passive in the last hour), the away summary at the operator's return (each
notice once, as held, sent away, cleared or acknowledged), and the
consequences of a cleared root.

## Outcomes

A delivery never changes. What happens to it later is an **outcome**
appended to it: an outlet's attempt (`unknown`, `delivered`,
`not_sent:<reason>`), `shown`, `updated`, `closed` (expired, dismissed,
cleared, replaced), `opened` (via desktop, web or CLI), `acknowledged`,
`snoozed until T`, `released` (a snoozed or folded notice routed again),
`cleared`, and `seen` (displayed to an explicit consumer; previews, on-screen
suppression and agent reads never count). `DeliveryState::fold` gives a
delivery's state and `Usefulness` a rule's figures: shown, opened, dismissed
within 3 s of being shown, expired.

Acts go through the host policy: `acknowledge` (a held notice is handed off
passive), `snooze(delivery, until)` (at `until` a current-state notice is
shown again only if its condition still holds; a historical one as is).

## Watches, wait and backtest

- A **watch** is a rule with `fires: on_change` and `deliver: notice`; the
  product supplies the query that defines "changed" for a reference.
- **`Wait`** returns at the instant `Follow` with the same arguments would
  emit its first message (or, with `until: empty`, at the first complete
  evaluation with no rows), or at its required timeout with the last
  evaluation. It records nothing.
- **`backtest`** replays rules over stored observations and attention facts:
  the deliveries they would have made, each with the decision it would have
  received. For a rule enabled at one revision throughout, it equals what the
  live evaluator and host policy produced. It writes nothing.

## Desktop notifications

Ruling, 2026-10-28, on desktop notifications: "it sounds like good idea, just
needs to be designed elegantly to not be a kludgy mess"
(cc:ee52794c-8a9e-43b6-9fbc-9749c7e78e8f:3ae3257e-e70a-433a-be97-d1222670c284).

- **Transport:** the freedesktop Notifications interface
  (`org.freedesktop.Notifications`, specification 1.2) on the session bus,
  for notices whose decision interrupts and for summaries. `Outlets::of`
  maps a decision to the desktop, the away command, or both.
- **Content:** summary `<subject> · <what>` on one line; the body is the
  evidence and the folded consequences; `app_name` the product,
  `category` `<product>.<rule>`, `desktop-entry` the product's web app,
  `x-<product>-delivery` the delivery ID, `transient` for notices that should
  not stay in the server's history, `resident` for current-state notices
  that their condition closes.
- **One notification per occurrence,** replaced in place; a cleared
  condition closes it (`CloseNotification`).
- **Urgency:** `normal`; `critical` (no expiry) only for critical notices.
- **Actions,** when the server reports the `actions` capability: `default`
  (Open), `snooze`, `acknowledge`, and rule-supplied actions (Live sessions:
  Approve, Deny). The `ActivationToken` the server sends before
  `ActionInvoked` comes with the Open event, so the product passes it to the
  browser launch and focus moves under Wayland's activation rules. Without the
  capability the body names the command to run.
- **Own notifications:** Sinex's D-Bus source marks a captured notification
  carrying an `x-<product>-delivery` hint as a signal origin
  (`signal_origin`), so no rule reads the products' own notifications.
- **Outcome:** an attempt starts `unknown`; a returned notification ID records
  `delivered`; an error records `not_sent:<reason>`, including
  `no_notification_server`. A `not_sent` desktop attempt is retried once
  under the same identity when the notification service appears. A
  `delivered` notice is never shown again automatically, and an `unknown`
  attempt (no answer within the deadline) is not resent. A slow notification
  server never holds a writer.
- **Away command:** a command named in deployment configuration receives each
  sent-away notice as JSON on standard input, with a deadline. Exit 0 records
  `delivered`.

## Elicitation

Decided target (2026-11-01), not built. Elicitation is a signal with an
answer, so whatever the operator decides about prompts is configuration, not
a second mechanism. Which prompts may be sent, and when, is an open operator
question; until it is answered no `ask` rule is enabled and nothing is asked.

- An **instrument** is a named, revisioned, short question set that the
  product declares as an authored record: items with an `id`, a `prompt`, an
  answer type (`scale(min, max, labels)`, `choice`, `multi`, `duration`,
  `text`, skip allowed) and an `expires_after` (default 30 minutes). An
  instrument may declare `max_per_day`; it is the instrument's own limit,
  because answer quality, not attention, sets it.
- A **prompt** is a delivery of a rule with `deliver: ask` and an
  instrument. Its trigger is the rule's ordinary `fires`: `on_new` over a
  product unit for event-triggered prompts, `at` with a declared `jitter`
  window for random-time ones. Its `interrupt` and the route function apply
  unchanged.
- An **answer** is the product's own self-report record with the prompt
  delivery as its parent; the delivery gains the outcome `answered`. Answer
  channels, in the order the notification server allows: notification
  actions for scale and choice items (one click), inline reply where the
  server reports that capability, the web inbox, the product's CLI, and MCP
  elicitation in a harness that supports it.
- A prompt not answered by `expires_after` gets the outcome `missed`, and an
  answer after it is recorded as `late`. A missed prompt is never imputed.
- Instruments are for the operator's self-reports only. An agent that needs
  the operator pings `@operator` in its product; that ping is an
  agent-authored notice under the budget.

## Configuration

Deployment configuration (the products' Nix modules, configuration file or
environment) names the value targets, action commands and the away command;
a rule can reach only those. Quiet hours, the interrupt budget and the
deep-work deferral are changeable settings of the deliverer's product.

## Rust interface and product adoption

`Rule` contains only authored fields. Products stamp `ActiveRule.version`,
`author`, `via` and, for shipped rules, `shipped`, and set `live` when the
query reads current state. `Rule::validate` checks deployment target and
command names; `effective_rules` overlays authored rules on shipped defaults
by product and name. `FIELDS` is the authored field table for help and
schema rendering. `fires` serializes as `"on_new"`,
`{"on_change":{"what":"membership","k":null}}`,
`{"on_threshold":{"value":10,"compare":">"}}` or `{"at":{"minute":600}}`.
Durations accept milliseconds or strings such as `20m`; quiet hours are
`{"from":1320,"to":480}` in local civil minutes, resolved through the query
language's `DaySetting` by `DayCalendar`.

A product queues a complete `Evaluation` for each watched query, with all
pages joined in query order. Its `at` is the observation checkpoint, and each
`Row` names the logical `Occurrence` and period, its original observation,
its effective start and optional end, a reference, and evidence. Delivered
signals and injected context are marked by `Origin` and excluded before any
mode evaluates. A newly installed query is incomplete until its first
complete evaluation. The deterministic delivery ID hashes the product, rule
ID and revision, occurrence and period.

Products persist the `Evaluator` checkpoint with the reports it produced, and
the deliverer's `HostPolicy` atomically with the deliveries, outcomes and
summaries each advance returns. Both expose their complete serialized state
through `JsonSchema`. Runtime wakeups combine the evaluator's and host
policy's `next_due`; neither is a polling interval.

Products execute `value` and `action` handoffs through the configured target
and command, outside their writer, with an unknown `DeliveryReceipt` committed
before the effect. Interrupting notices and summaries go to
[`signal_delivery::run`](../signal-delivery/README.md) through a channel,
never through a writer lock; desktop acts come back as receipts, which
`NotificationReceipt::outcomes` turns into outcomes, and the product applies
snooze, acknowledge and rule actions through its operation table as the
operator's acts with `via: desktop`.

The `web` feature renders the **Inbox** (`/notices`: interrupted, held and
passive notices, unseen first, with the attention line), the **Why** panel
(rule, matched rows, route decision and its inputs, outcomes) and the
**Signals** view (rules with author, channel, last firing, deliveries in
seven days and usefulness) as maud functions, so both products show the same
view for the same data. Products supply titles, links and their time format.

## Laws

The evaluator laws:

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
  late-observed closed period into a present-state reminder.
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
- **one-deliverer-per-host**: with Polylogue on Sinex, Polylogue only
  reports; its interrupting notices are routed by Sinex's host policy under
  the one budget and quiet hours.
- **notification-replaced-in-place**: a second notice about one occurrence
  replaces the first through `replaces_id`, and a cleared condition closes it.
- **not-sent-retried-once-unknown-never**: with no notification server, an
  interrupting notice records `not_sent:no_notification_server` and is
  retried once under the same identity when the server appears; an
  unanswered attempt stays `unknown` and is not resent.
- **slow-server-never-holds-writer**: a notification server that never answers
  does not delay any commit.

Routing, attention, inhibition and outcomes:

- **route-pure**: `route` is a function of (notice, attention) only, and
  the decision with its inputs is stored on the delivery.
- **attention-only-removes**: with every attention input unknown, routing
  is quiet hours and the budget alone; attention inputs never turn a notice
  that would not interrupt into one that does.
- **critical-never-deferred**: a critical delivery is shown at once in
  every attention state, including quiet hours.
- **deferral-bounded**: a breakpoint hold ends at the first breakpoint or
  at its bound, whichever comes first.
- **away-summary-once**: notices held or sent away while the operator is
  away produce exactly one summary at the return, listing each once, as
  held, sent away, cleared or acknowledged; none is shown on its own.
- **on-screen-is-passive-not-seen**: on-screen suppression records
  `passive (on screen)` and never marks the notice seen.
- **inhibit-folds**: while an inhibiting occurrence is open in scope,
  inhibited deliveries are recorded, passive and lines of the root's
  notification; when the root clears, those still current are shown at once
  as one grouped notification.
- **inhibit-shipped-only**: an authored rule can be neither inhibitor nor
  exempt.
- **own-notifications-excluded**: a captured notification carrying an
  `x-<product>-delivery` hint is a signal origin, and no rule's input
  includes it.
- **snooze-rechecks**: a snoozed current-state notice is shown again at
  the snooze end only if its condition still holds; a historical one is shown
  again as is.
- **outcomes-append-only**: a delivery is handed off once and never
  changes; acts and attempts append outcomes.
- **checkpoint-loss-never-refires**: rebuilding the evaluator from rules,
  stored deliveries and stored observations never produces an existing
  delivery identity; at worst a threshold rule reports `rearmed_late`.
- **backtest-equals-live**: for rules enabled at one revision throughout,
  `backtest` over an interval returns exactly what the live evaluator and
  host policy handed off over it.
- **wait-is-follow**: `wait` returns at the instant `follow` with the same
  arguments would emit its first message, and records nothing.
- **external-notice-budgeted**: a producer notice asking to interrupt is
  charged to the budget and never exceeds it; a notice with an existing key
  replaces its notification in place.
- **forwarded-stale-holds**: a forwarded current-state notice whose
  producer has not confirmed being up to date through the instant it fired
  is held as `incomplete`, never shown; progress at the firing instant
  releases it at a later handoff.
- **trigger-never-starts-agent**: no delivery starts or resumes an agent;
  a configured action command that is an agent launcher is refused.
- **late-is-passive**: a triggering fact older than its rule's
  `fresh_within` never produces an interrupting delivery, and the delivery
  states its lateness.

Elicitation (decided target, not built):

- **elicit.answer-cites-prompt**: every prompted self-report has its prompt
  delivery as parent; a spontaneous one has none.
- **elicit.missed-is-recorded**: a prompt past `expires_after` without an
  answer has the outcome `missed`, and no figure imputes it.
- **elicit.cap**: an instrument never prompts more than `max_per_day` times
  per logical day.
- **elicit.none-until-enabled**: with no enabled `ask` rule, no prompt is
  ever sent.

`trigger-never-starts-agent` and `on-screen-is-passive-not-seen` are the
products' laws too: a page watch, a live-session trigger or a Sinex presence
reading that must not start an agent, and a supervised session's on-screen
notice, are cases of these laws, which each product runs rather than
restating.

The tests are in `tests/` (evaluator, adoption, route, host, follow, web and
checkpoint schema) and in `signal-delivery`'s fake notification server suite.
