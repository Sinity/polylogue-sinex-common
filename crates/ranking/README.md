# ranking

Ranking from judgments for Polylogue and Sinex: a judge orders k targets,
best first, and this crate fits one score per target under the
Plackett–Luce model, reports how sure the fit is, and proposes which targets
to compare next. Polylogue's comparisons and Sinex's library rankings use the
same fit, so the numbers mean the same thing in both.

The crate holds no storage and no vocabulary of its own. A product passes the
judgments it counts (the judge set is a parameter here), reads the fit, and
keeps its own records, scales and views.

## Judgments

A **judgment** is one judge's ordering of k targets, best first. Each element
of its order is a **tie group**: targets in one group are equally placed. A
pair is a judgment of two groups of one target; a k-ordering is k groups of one
target each. A judgment needs two distinct targets, no target may appear
twice, and no tie group may be empty. Malformed judgments are refused.

A **judge set** chooses which judges count: every judge, or a named set. A
product resolves its own lens or saved fragment into names. The fit reads only
the judgments the set counts, so another judge's judgments change nothing
(`rank.judge-isolation`).

## The model and the fit

Each target `i` has a worth `gamma_i > 0`; its **score** is `theta_i = ln
gamma_i`. The model is Plackett and Luce's (Plackett 1975; Luce 1959): for an
ordering, each target in turn is chosen from those still unplaced with
probability proportional to its worth. A k-ordering is one likelihood term,
not k(k−1)/2 pairs, so its information is not expanded into implied pairs
(`rank.k-not-expanded`). For pairs it reduces to Bradley–Terry (Bradley and
Terry 1952).

**Weak prior.** Every target has one virtual win and one virtual loss against
a reference of strength 1, which is never updated. This keeps the fit finite
for an unbeaten target and for a component with no path to the others, and it
makes the objective strictly concave, so the maximizer is unique.

**Tie groups.** A tie group of `g` targets is modelled by a uniform random
tie-break: each of its `g!` internal orders is equally likely, and the group's
term is the expectation over those orders. A tied pair therefore pulls its two
scores together, and a tie between two targets and a third says the third is
better or worse than the pair, not which of the pair is better
(`rank.ties-pull-together`). This is a modelling choice, not a standard
result. Tie groups over more than 20 targets are refused: the cost is `2^g`
subsets per iteration.

**Numerical method.** The maximum is found by Hunter's minorization–
maximization (Hunter 2004, for the Plackett–Luce generalization of
Bradley–Terry). Each step uses `ln S ≤ ln S_old + (S − S_old)/S_old` for each
stage's risk-set sum `S`, which gives the closed update

`gamma_i ← (1 + support_i) / D_i`, with `D_i = 2/(gamma_i + 1) + Σ w_U / S_U`

summed over the stages (and subsets `U` of a tie group) that contain `i`. Each
step cannot decrease the objective. The iteration stops when every score moves
by at most `tolerance` (default 1e-10), or after `max_iterations` (default
10,000). A run that hits the allowance returns `FitStatus::Stopped` with the
iteration count and last change; it is never reported as converged.

**Intervals.** The observed information is the negated Hessian of the
objective in the scores, which is `Σ w_U (diag(pi) − pi piᵀ)` over the stages
plus `2 pi (1 − pi)` on each diagonal for the prior. Its inverse's diagonal is
the Laplace variance of each score (Tierney and Kadane 1986), and the 95 percent
interval is the score ± 1.96 times its standard deviation. The reference is
fixed, so the information is block diagonal over components, and each
component is factored separately by Cholesky decomposition (`linalg.rs`).

**Ranks.** `rank` is the position by score, ties ordered by item. `rank_range`
is the best and worst position the intervals allow: one more than the number of
items whose lower bound lies above this item's upper bound, and the number of
items minus those whose upper bound lies below this item's lower bound.

**Components** are the groups of targets linked by judgments the judge set
counts, numbered by the first target in order. Scores in different components
are comparable only through the prior, so a product should show components
apart when it shows scores.

**Warm start.** `fit_warm` starts the iteration from a previous fit's scores.
The optimum is unique, so the result agrees with a cold fit within the
tolerance. The iteration saving depends on how far the new judgments move the
optimum: after one judgment added to a well-determined fit, the warm start takes
fewer iterations (`rank.warm-start-agrees`); on a sparse fit it may not.

**Determinism.** Judgments and targets are canonically ordered before the
fit, so any arrangement of the same input gives the same fit, bit for bit
(`rank.deterministic`).

## Selecting the next set

`next_set` proposes `k` targets from a fit's candidates under one of three
rules. Every rule first seats the targets no judgment in the judge set names
yet, in the rule's own order, so an uncompared target appears in the first
proposal and each proposal takes `min(k, uncompared)` of them
(`rank.next-includes-unseen`). The rules fill the remaining seats:

- **Information** (default in products): the pair with the largest expected
  reduction in the variance of its score difference, then greedily the target
  whose summed reduction against the seats so far is largest. For a pair with
  difference variance `v` and win probability `p`, one more comparison
  reduces the variance by `v² q / (1 + v q)`, `q = p(1 − p)`. This is the
  active-sampling idea of Chen et al. (2013), approximated: covariances between
  targets are ignored, so each difference's variance is the sum of the two
  targets' variances (`rank.next-information-max-reduction`).
- **Random**: a seeded shuffle (SplitMix64, Steele, Lea and Flood 2014;
  Fisher–Yates), so one seed names one proposal (`rank.next-random-seeded`).
- **UncomparedFirst**: the nearest scores to an anchor. The anchor is the first
  uncompared target, or, when none is uncompared, the least-supported one.
  Each seat records why it was chosen (`rank.next-uncompared-nearest`).

## Costs and bounds

- Memory and time: the information matrix is dense per component, `O(m²)`
  memory and `O(m³)` to invert, where `m` is the component's size. A sparse
  factorization is the follow-up for components of many thousands of targets.
- Each MM iteration costs the total stage size, `O(Σ |B| · 2^|B| · |R|)` over
  the stages and their tie subsets.
- The information rule scans all candidate pairs for its seed pair, `O(n²)`.
- Tie groups larger than 20 are refused (`RankError::TieGroupTooLarge`); the
  judging interface uses k ≤ 6.

## Products

- **Sinex** (`library`): replaces the Bradley–Terry code in
  `crates/sinex-library/src/rank.rs`, its `rank` view and its `next_pair`. The
  judging picker, `rank`, `judge next` and the levels that bin scores are
  product code over this crate.
- **Polylogue** (`comparisons`): the same fit for `| sort by rank(scale)` and
  the review queue.

## References

- Bradley, R. A. and Terry, M. E. (1952). Rank analysis of incomplete block
  designs. *Biometrika* 39.
- Chen, X., Bennett, P. N., Collins-Thompson, K. and Horvitz, E. (2013).
  Pairwise ranking aggregation in a crowdsourced setting. *WSDM*.
- Hunter, D. R. (2004). MM algorithms for generalized Bradley–Terry models.
  *Annals of Statistics* 32(1).
- Luce, R. D. (1959). *Individual Choice Behavior*. Wiley.
- Plackett, R. L. (1975). The analysis of permutations. *Applied Statistics* 24.
- Steele, G. L., Lea, D. and Flood, C. H. (2014). Fast splittable pseudorandom
  number generators. *OOPSLA*.
- Tierney, L. and Kadane, J. B. (1986). Accurate approximations for posterior
  moments and marginal densities. *JASA* 81.
