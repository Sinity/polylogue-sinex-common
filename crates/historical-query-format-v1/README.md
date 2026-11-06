# Historical authored-query format 1

This decoder reads immutable authored-query format 1 material and stored query definitions during explicit versioned data upgrades. Polylogue and Sinex own their conversion into current executable templates. Current command admission uses the current shared grammar and product operation templates.

The parser, syntax, catalog, scope, number and identity modules come from `polylogue-sinex-common` revision `8c22e127bb1f5c10d8c1740de4c4f0bfe53ffdb7`. The duration normalizer is the portion of that revision's time module needed for original question identities. The old interpreter, binders, clocks, cursor machinery and unrelated APIs are omitted. Upstream test-module declarations are omitted; this package has its own format fixtures.

The distinct format-named package is deliberate. Linking the current `query-language` checkout cannot replace this implementation with that crate. Its dependencies contain no current `query-language` package.
