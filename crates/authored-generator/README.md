# Authored history generator

`history(n)` is a proptest strategy for histories of up to `n` authored
records: new records, revisions, forks, conflict resolutions, withdrawals
of revisions and roots, and withdrawals of withdrawals. Every link names
an earlier record, so generated withdrawals never form a cycle.

Polylogue and Sinex use this crate as a development dependency. They replay
these histories into their stores and compare their stored currency with
`authored::History::states`, including arrival orders different from the
record order. `WITHDRAWAL` names the generated withdrawal kind; other
records use `note`.

The generator has its own crate so enabling a test strategy never changes
the runtime `authored` crate's features or brings proptest into a package.
Its tests retain an independent whole-history statement of the currency
rule as their oracle.
