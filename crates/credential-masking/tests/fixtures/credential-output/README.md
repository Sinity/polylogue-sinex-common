# Credential output fixtures

Synthetic examples for the [streaming masking rules](../../../../../docs/credential-masking.md).
`catalog.json` owns the credential families and patterns; `conformance.json`
owns the handwritten expected UTF-8 byte ranges. The Rust tests exercise the
masker itself at every byte split and at chunk widths 1 through 8.

`conformance.json` holds 138 handwritten cases. The six `*-crossing-*` cases
require later secrets starting inside unfinished quoted values to be masked
as well.

Each input is stored as `text_parts` of at most eight characters, joined by
the test, so the source holds no credential-shaped literals. The test replaces
each output marker with a placeholder and compares the surrounding text with
the expected regions. Change the catalog, expected ranges and documented
behavior together.
