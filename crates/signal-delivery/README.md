# signal-delivery

The desktop and away-command worker for the pure
[`signals`](../signals/README.md) notification state. It is one task: it
takes committed notices from a channel, calls `Notify` or the away command
with a deadline, and returns outcomes. Holds, rechecks, the budget and routing
belong to the host policy in `signals`; nothing here decides whether a notice
should be shown.

Products supply an explicit D-Bus connection, or `None`, the configuration
(the away command and the per-call deadline) and the restored
`NotificationState`. The library never opens the operator's session bus
itself.

`run` receives `NotificationRequest::Deliver` for each notice whose decision
interrupts (and each summary) and `NotificationRequest::Clear` when a
delivery's condition clears or the operator acknowledges it. It returns
`NotificationUpdate` values; the product persists each state and its receipts
before acknowledging `persisted`. The worker records unknown before any
effect, so a crash leaves an unknown attempt, which is never resent. Once
`Notify` has been issued, a missing answer within the deadline leaves the
outcome unknown; a failure before that records `not_sent`. When the
notification service appears, each explicit desktop failure is retried once
under its original identity; the away command is never rerun. A slow server
never holds a writer, because notices arrive through the channel.

The notification carries the product's name and icon, `category`
`<product>.<rule>`, `urgency`, `x-<product>-delivery`, `desktop-entry`,
`transient` and `resident` hints, and the actions Open, Snooze, Acknowledge
and any rule-supplied ones when the server offers actions. Server signals
come back as receipts with an event: `ActionInvoked` as open (with the
`ActivationToken` the server sent just before it), snooze, acknowledge or a
rule action; `NotificationClosed` as expired, dismissed or closed. All are
the operator's acts, `via: desktop`. A later showing of the same delivery (a
snooze ending) is a new attempt under the same identity.

The tests run a private `dbus-daemon` with a fake notification server that
implements capabilities, `replaces_id`, actions, close reasons and
`ActivationToken`, and synthetic away commands.
