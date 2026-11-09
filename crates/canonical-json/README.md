# canonical-json

The one JSON encoding that identity hashes in Polylogue and Sinex read.

An identity hash (a message version, a document version, an event ID, a
record hash, an equivalence key) must give the same bytes for the same value
in every build of both products. serde_json does not promise that: its
`preserve_order` feature changes the order a map keeps, its
`arbitrary_precision` feature changes how a number is held, and Cargo turns a
feature on for every crate in a binary once any one crate asks for it. Each
product used to sort maps by hand at some hash sites and not at others, so a
test build and a production build could hash the same value differently.

## The encoding

`canonical_bytes(tag, &value)` returns:

1. the tag, such as `polylogue-message`, which names the kind of identity, so
   two kinds never share bytes;
2. one zero byte;
3. the value as JSON, with
   - the members of every object, at every depth, sorted by key compared as
     UTF-8 bytes; structs are objects, so their fields sort too;
   - numbers written as the value holds them: a number parsed with
     `arbitrary_precision` keeps its spelling, so `1.50` stays `1.50` and is a
     different value from `1.5`; integer and float fields are written as
     serde_json writes them;
   - no whitespace, and strings escaped as serde_json escapes them.

`canonical_hash(tag, &value)` is BLAKE3 of those bytes. Both accept any
`Serialize` value, including `serde_json::Value`; encoding a typed value and
encoding `serde_json::to_value` of it give the same bytes. The encoder
serializes the value directly and buffers only each object's members while it
sorts them.

Number spelling survives only where the value was parsed with
`arbitrary_precision`. A product that parses JSON without it rounds `1.50` to
`1.5` before any hash sees it, so both products parse with it.

`canonical_json(&value)` is the same JSON without a tag. It exists for the
question identity in `query-language`, whose bytes stored records already
cite; a new identity always takes a tag.
