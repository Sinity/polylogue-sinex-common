# credential-masking

`Masker` masks credentials from the fixed `credential-output-v2` catalog in streamed UTF-8 text. Feed pieces with `push`, end a block with `block_boundary` and end the string with `finish`. Use one masker per string. Masking never refuses input; text that stays undecided past a 512-byte window is masked. After a sink error, discard the response.

Each character and each marker reaches the sink in one `write_all` call, so a sink that fits output to a size limit can refuse a whole marker.

Behavior and limits: [credential masking](../../docs/credential-masking.md). The catalog keeps its own name, `credential-output-v2`, because it is a versioned data format.
