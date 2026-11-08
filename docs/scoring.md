# scoring

Decided target (2026-11-01); the crate is not in the workspace yet, and this
page becomes its README when it is created.

Proper scores for forecasts, shared by Polylogue and Sinex: Sinex's forecasts
and their forecasters' track records, Polylogue's forecast assertions, and
Polylogue's pace and quota track records all score with these functions, so a
Brier score or an interval coverage means the same thing in both products.
The crate is pure functions over forecasts and outcomes. It holds no storage,
decides nothing about when a forecast resolves, and knows no product's
units; each product resolves its forecasts and passes the resolved set in.

## Forecasts and outcomes

A **forecast** has one of two shapes:

- **binary**: a probability `p` in `[0, 1]` that a check will hold by its
  horizon;
- **quantile**: values for any subset of the quantile levels from 5% to 95%,
  for a numeric outcome.

An **outcome** is `true` or `false` for a binary forecast, or the observed
value for a quantile forecast. An outcome the product could not establish
(the check's or the cell's coverage stayed incomplete) is **unknown** and is
never passed to a score: it is counted beside the score as unresolved. A
forecast with a probability outside `[0, 1]`, quantile levels outside
`(0, 1)` or quantile values that decrease as the level rises is refused.

## Scores

Every score here is proper: a forecaster minimizes its expected score by
reporting what it believes (Gneiting and Raftery 2007).

| Score | For | Definition |
|---|---|---|
| Brier | binary | `(p − o)²` with `o` = 1 for true, 0 for false; lower is better |
| log | binary | `−ln p` for true, `−ln (1 − p)` for false; a probability of exactly 0 or 1 on the wrong outcome scores infinity, reported as such |
| pinball | quantile, per level `τ` | `(y − q)·τ` when `y ≥ q`, else `(q − y)·(1 − τ)` |
| interval coverage | quantile | for a central interval (10%–90% gives 80%), the share of outcomes inside it, against the nominal share |
| CRPS | quantile | the continuous ranked probability score approximated from the quantiles as twice the mean pinball loss over the supplied levels |

A track record reports the mean of each score over the resolved set, the
count of forecasts scored and the count unresolved.

## Calibration

For binary forecasts, **calibration bins** group forecasts by probability
(ten equal bins by default) and report, per bin, the count, the mean
forecast probability and the observed frequency. A bin with no forecasts is
reported empty, never as zero frequency. For quantile forecasts, calibration
is the observed coverage of each supplied level.

## Skill

**Skill** compares a forecaster with a reference forecaster on exactly the
same resolved set: `1 − score / reference score` for a score where lower is
better. The two reference forecasters every product supplies are
**climatology** (the prior-only empirical distribution of the target at its
grain) and **persistence** (the last observed value). A forecast that has no
reference forecast for the same target and origin is left out of the skill
comparison and counted, so skill never compares two different sets.

## What a product supplies

The resolved forecasts with their outcomes, the reference forecasts for the
same targets and origins, and which forecasts count: a product decides that
a forecast counts in its issuer's record only when it was issued before its
target period and before any resolving evidence existed, and that a
backtest's forecasts never mix with prospective ones. Rendering, storage and
the issuers' identities stay in the product.
