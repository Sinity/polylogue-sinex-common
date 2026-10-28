# Credential output fixtures

Synthetic fixtures originated in the specification repository at commit `592156e2`, under `polylogue-sinex-joint/prototype/masking/` (`catalog.json` and `conformance.json`). This identifies their historical source.

`conformance.json` holds 138 handwritten cases. The six `*-crossing-*` cases require later secrets starting inside unfinished quoted values to be masked as well.

Each text is stored as `text_parts` of at most eight characters, joined by the test, so the source holds no credential-shaped literals. The test replaces each marker with a placeholder and compares the surrounding text with the expected regions. Edit these fixtures together with the crate implementation and its tests.
