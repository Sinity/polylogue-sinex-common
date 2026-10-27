# Credential output fixtures

Synthetic fixtures derived from the specification repository's `polylogue-sinex-joint/prototype/masking/` (`catalog.json` and `conformance.json`); the policy is `polylogue-sinex-joint/credential-masking.md`.

`conformance.json` holds 138 handwritten cases. The six `*-crossing-*` cases require later secrets starting inside unfinished quoted values to be masked as well.

Each text is stored as `text_parts` of at most eight characters, joined by the test, so the source holds no credential-shaped literals. The test replaces each marker with a placeholder and compares the surrounding text with the expected regions. Change these files only together with the policy.
