# Credential output fixture

Synthetic fixtures from `/realm/inbox/spec`, commit `caf388a03b14e4714e4b2bc7d48562f0d5dc577f`:

- `polylogue-sinex-joint/prototype/masking/catalog.json`, SHA-256 `c1e5395a33a22987c6dc77905a12d9bf0a9306bee0108fd96871157bef6635af`, copied unchanged.
- `polylogue-sinex-joint/prototype/masking/conformance.json`, SHA-256 `9b492762d96f587cca5435dc62bef6271ff6812cc8e847a5e952a379901a0d83`.

The policy is `polylogue-sinex-joint/credential-masking.md` at that commit. Version 2 recognizes independent valid overlapping starts within each family; version 1 used non-overlapping enumeration. Its 138 handwritten cases preserve all 132 version-1 texts and expected byte regions and add six crossing quoted-value cases. The version-1 fixture directory remains unchanged.

Each text is stored as `text_parts` of at most eight characters, joined by the test, so the source holds no credential-shaped literals. The joined texts match the source corpus exactly. Update these files only from a reviewed revision of the policy and corpus. The test replaces each marker with a placeholder and compares the surrounding text with the expected regions.
