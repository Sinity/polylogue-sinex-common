# Streaming credential masking

`crates/credential-masking` masks the 22 credential families of the shared `credential-output` catalog in streamed UTF-8 text, through `Masker::new`, `push`, `block_boundary` and `finish`. It changes display bytes only; stored content is untouched.

A credential becomes `[credential: KIND, masked]`, with no preview. Labels such as `--secret=` or `Authorization: Basic` stay visible unless they lie inside another secret. URL masking keeps the scheme, the last `@` before the host, the host and the path. PEM masking covers any uppercase label ending in `PRIVATE KEY`, through the matching END line or the end of the string. Overlapping matches merge into one marker; adjacent matches keep separate markers.

Each valid start is recognized on its own, including a start inside an earlier match of the same family. In `--secret="unfinished --token="synthetic-later-value" tail`, the first quoted value ends at the later value's opening quote. Both matches contribute their secret ranges, so the later value is masked too; the six `*-crossing-*` cases in the catalog test this.

`push` pieces never end a match. `block_boundary` ends token matches and writes unfinished prefixes as plain text, but an open PEM block stays masked. A PEM header and its END line each count only whole within one block, so the state that crosses a block boundary is just whether a PEM block is open and which END line closes it; a reader can resume masking at any block from that alone. `finish` ends the string. Masking never refuses input; the only errors come from the sink.

Text that may still become a credential is held back, at most 512 UTF-8 bytes. When an undecided match would hold more, it is forced: what it has read is masked (from its secret part if that has begun, otherwise from its start), and it keeps masking each character it reads until it ends. A credential longer than the window, such as a JWT with a large payload or a URL with a long password, is masked whole; a forced URL also masks its host, and a forced match that turns out not to be a credential stays masked. A PEM label longer than 512 bytes masks to the end of the string, since its END line is not kept. Memory stays bounded: a later candidate is dropped when an earlier mask covers its consumed bytes and the same pattern state proves their future continuation is identical. A label inside a mask alone is insufficient.

Left word boundaries use a Unicode 16.0.0 table of Python `str.isalnum` plus underscore (`src/word_ranges.rs`). Whitespace includes U+001C to U+001F, as in Python. Update the table and the boundary tests together.

A sink error is returned unchanged and closes the stream; later calls return `BrokenPipe` without writing. `finish` also closes the stream. The masker never calls `flush`.

The tests run all 138 catalog cases at every byte split and at chunk widths 1 to 8, plus overlap, block boundary, Unicode boundary, long-credential and sink-error cases, and randomized checks that output does not depend on chunking and that embedded credentials never appear in it. The catalog and its provenance are in [`crates/credential-masking/tests/fixtures/credential-output`](../crates/credential-masking/tests/fixtures/credential-output).

## Finding spans

`Scanner` reads UTF-8 pieces with `push`, ends each authored string with
`block_boundary`, and ends the stream with `finish`. Each boundary returns
findings with a pattern name, byte start and end, and a SHA-256 fingerprint.
It returns no matched text. Offsets count source bytes across all pushes;
a boundary ends token matches but keeps an open private key. Private keys
spanning several blocks produce one finding per block.

The scanner observes the masker's regions, including its conservative masks
for an undecided long match. Overlapping regions produce one finding. It also
finds generic credential assignments with at least 20 ASCII value characters
and Shannon entropy of at least 3 bits per character. This additional scan
rule does not change normal output masking. It has no value length cutoff;
only a hash and character counts accumulate. `SCANNER_VERSION` identifies the
rules for clients tracking coverage. A finding is a suspected credential,
not confirmation or permission to remove anything.
