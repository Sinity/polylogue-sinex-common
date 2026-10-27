# Vocabulary

One name per concept. A name says what the thing is in ordinary words; use these in identifiers, local variables, comments, docs, error text and tests.

| Name | Meaning |
|---|---|
| blob | Bytes stored under their hash, in a file under `blobs/` |
| plain, delta, sealed | The three blob layouts: zstd frames, a zstd patch against a plain base, encrypted frames |
| name | A blob's hash: BLAKE3 of the content, or a keyed BLAKE3 for sealed blobs |
| temporary file | A blob being written in `blobs/tmp/`, removed when the write ends |
| prepared blob | A written and hashed blob not yet under its name (`PreparedPlain`, `PreparedSealed`) |
| install | Hard-link a temporary file under its name, durably |
| write root, extra roots | The directory a store writes to, and further directories it reads from |
| extent | A byte range of one blob; a list of extents assembles one stream |
| data key, key reference | The key that seals a blob, and the caller's name for it |
| catalog | The credential families `credential-masking` recognizes |
| marker | `[credential: KIND, masked]`, the text that replaces a credential |
| run | One xtask invocation, recorded in the history store |
| stage | One timed step of a run |
| tree | The Git tree object of a run's HEAD; with the hash of uncommitted changes it says what a run tested |
| fixture | A value `#[devkit::test]` creates for a test parameter |
| schema database, template | The read-only database compile-time SQL checks use, and the database tests clone |
| sample | Corpus files drawn for a benchmark, written down in a manifest |
| scenario | One exported NixOS check selected for a VM run |
| corpus | The input files a fuzz experiment starts with and grows |
| region | A source span and execution count reported by LLVM coverage |
| development stack | A checkout's owned services and selected application processes |
| quiet run | A bench run during which every host-load reading was below the thresholds |

Words that do not appear: custody, reserve or reservation for bytes, publish or publication (except a NATS publish), retain, retained, retention, retire, retirement, canonical, admission, qualify, qualification, legacy, predecessor, control or proof for a test, witness, authority, closure, candidate (except its plain meaning), basis, frozen, lease, interpretation, backing. Comments and docs carry no issue numbers, specification section numbers or history.
