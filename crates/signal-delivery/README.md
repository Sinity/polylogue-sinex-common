# signal-delivery

The desktop and configured-command worker for the pure [`signals`](../signals/README.md) checkpoint. Products supply an explicit D-Bus connection, or `None`, configuration, restored state and a fresh policy callback. The library never opens the operator session bus itself.

`run` receives `NotificationRequest` values and returns `NotificationUpdate` values. Persist each state and its receipts before acknowledging `persisted`. The worker records unknown before sending; restored unknown attempts cannot automatically resend. A D-Bus appearance retries each eligible explicit failure once, under its original identity. Readiness is per route, so this never reruns a completed or unknown command.

Every actual Notify or command call requires the fresh policy callback, including historical occurrences and grouped replacements after clearing an occurrence. The product owns authorization, source freshness and condition matching. A callback may retain its policy lease until the next update, and must release it before persisting that update if the writer needs the same lock.

`Import { attempt, imported }` adopts an authenticated, immutable attempt after persisting it. The product validates authority before requesting import. Duplicate original snapshots retain local progress. The worker probes the actual server and persists any eligible D-Bus retry as unknown before sending. Delivered, cancelled and unknown routes are never retried by import.

`Release { delivery_id, released }` stops the source owner. Its update carries the original attempt in `released` alongside the inactive checkpoint. Persist that original handoff in the existing product outbox atomically with the checkpoint before acknowledgement; the source may then forward it to the new owner. Neither operation resets the retry flag or rewrites historical outcomes.

The tests use a private fake notification server and synthetic command routes.
