# references

One grammar for anything Polylogue or Sinex can address: the reference
forms, the selectors that name a place inside a thing, the locators that name
things outside both products, the canonicalizer that makes locators
byte-identical in both products, and the route catalog that turns any
reference into a link to its owner's web view and back.

Each product supplies its own [catalog](#the-catalog): its kinds, their web
sub-views, its web address and its provider-URL patterns. What an identity or
a revision means stays the product's; this crate fixes where each part of a
reference begins and ends, how it is written, and how it travels.

## References

A reference names one object of one product, or one locator, and optionally
a place inside it.

| Form | Example | Where it is written |
|---|---|---|
| global | `sinex:ev/0190a1b2`, `polylogue:assertion/r1@3#exact=once` | stored, sent between products, read anywhere |
| local | `ev:0190a1b2` in Sinex, `session:claude-code:7f3a` in Polylogue | typed and printed inside the owning product |
| locator | `url:https://example.com/a`, `youtube:dQw4w9WgXcQ#t=30`, `file:laptop/r1/notes.md#L4-9` | the same everywhere; no product owns it |

The global form is `<product>:<kind>/<id>[@<revision>][#<selector>]`.

- **Product** is `polylogue` or `sinex`. Product names and locator kinds are
  reserved: no product declares a kind with one of them.
- **Kind** is a lowercase letter followed by lowercase letters, digits, `-`
  or `_`. The kind is the same word in both forms (`ev:` and `sinex:ev/`).
- **Identity** runs to the first `@` or `#`. It may hold `:` and `/`
  (`session:claude-code:abc`, `sinex:record/<material>/<offset>/<type>`).
  `@`, `#`, `%`, whitespace and control characters inside it are
  percent-encoded.
- **Revision** follows `@` and runs to `#`. Its meaning is the owner's: an
  exact revision of an authored record, for instance. Without one, a
  reference names the object as it currently is.
- **Selector** follows `#` ([selectors](#selectors)).

A local form is normalised to the global form at input
(`Catalog::parse`), so everything stored holds the global form and
`Reference::parse` reads it without a catalog. Printing for a person inside a
product (`Catalog::written`) gives the local form for that product's own
objects and the global form for everything else. Serde writes and reads the
global form.

A `%` that does not start an escape is a literal `%`; printing encodes it as
`%25`. Every input therefore has one printed form, and nothing is refused for
holding a `%`.

A kind can declare its local ordinal form with `Kind::with_local_form`:
`Ordinal` reads a literal identity followed by required `#<n>`;
`OptionalOrdinal` permits that suffix; `RevisionOrdinal` also permits
`@<revision>` before it. These local identity and revision fields are literal,
including `%`, `@` and `#` where they are not the form's final delimiters.
Ordinals are zero-based unsigned integers without leading zeros. For example,
`block:codex:m#2` becomes `polylogue:block/codex:m#ordinal=2`, and
`variant:root@v1#3` keeps root `root`, revision `v1` and ordinal `3` separately.
The default `Object` local form keeps the escaped grammar above.

An ordinal may accompany a text range. Human printing uses the explicitly
qualified global form when a declared local spelling cannot express the full
selection or would reinterpret an opaque identity. Products still validate
which kinds and member selections their domain supports; the catalog neither
looks up members nor invents identities.

### What a person types

`Catalog::read_input` classifies text where a reference may stand:

| Input | Becomes |
|---|---|
| `<sinex:ev/e1>`, `<file:host/root/path>` | an explicitly qualified global reference; the inner text is parsed only as a global object or locator |
| `self`, `here`, `last`, `latest` | a context word: the calling session; the caller's working-directory scope (repository, worktree, project); the product's most recent object of the kind asked for |
| `@3`, `@3.2`, `@2-5`, `@1,4`, `@*`, `@last` | a row pick from the last list printed for this terminal or session (rows and hits count from 1) |
| a reference in any written form | that reference, normalised to global |
| a local form of the other product's kind (`ev:e1` in Polylogue) | refused, with the global form (`sinex:ev/e1`) as the suggestion |
| an absolute URL | a product's own web address read back; else a product's provider-URL pattern (the reading product's first); else the canonical locator |
| anything else | bare text: a native identity or prefix the product resolves against its data |

`<global-reference>` is an input qualification. Whitespace outside the brackets
is ignored; the inner text keeps its ordinary escaping rules. Local forms,
context words, row picks, bare URLs and another layer of qualification are
refused inside it, without falling back to another input interpretation.
Use `url:` when qualifying a URL. Text beginning with `<` is reserved for
this construct; an opaque identity containing that text remains addressable
as the identity of its product-qualified object.

Canonical stored/global text and Serde output never add the qualification.
Human printing in Polylogue qualifies external file locators, because its
local `file:path@commit` spelling names a Git file at a commit. Human printing
in Sinex is unchanged. The qualification does not change a locator's
canonical identity or canonicalizer version, and reserves no object-kind name.

## Selectors

A selector names a place inside the referenced thing. One parser serves both
products. Parts are joined with `&` and always print in this order, so equal
selectors print equal text:

| Part | Form | Meaning |
|---|---|---|
| ordinal | `ordinal=0` | a recorded member ordinal, counted from zero; its owning product defines and validates membership |
| lines | `L40`, `L40-52` (`L40-L52` accepted) | lines counted from 1, both ends included |
| time | `t=10,20`, `t=90.5` (`npt:`, `h:mm:ss`, `mm:ss` and `t=,20` accepted) | W3C Media Fragments time in seconds; without an end, to the end |
| rectangle | `xywh=160,120,320,240`, `xywh=percent:0,0,50,50` (`pixel:` accepted) | W3C Media Fragments rectangle; width and height above 0 |
| text range | `b=4-9`, `c=4-9`, `u16=4-9` | a half-open text range in UTF-8 bytes, code points or UTF-16 code units |
| quote | `exact=…&prefix=…&suffix=…` | the W3C Web Annotation text quote: the exact text and the text around it, percent-encoded |
| named place | `id=section-2` | a place the document itself names, kept as written: a web page's element id from its URL fragment, a media chapter (the Media Fragments `id` dimension) |

- **Seconds are exact decimals.** Any number of fraction digits is kept;
  trailing zeros are dropped.
- **Out of range is invalid, never clamped.** `Selector::check` compares
  every part with the bounds a product knows (duration, size, line count,
  text length in each unit); an unknown bound is not checked.
- **A span is kept by its range and its quote.** A text range with a quote
  is found again in a container's current text by `Selector::span_in`:
  `Current` when the stored range still holds the quote, `Moved` when prefix, quote and suffix occur
  exactly once elsewhere, `ChangedSince` otherwise. Nothing is rewritten; the
  stored reference stays as written. `TextQuote::around` builds the quote
  with the context a writer chooses (Polylogue's assertions use 32
  characters).

## Locators

A locator is the canonical native address of a thing outside both products'
objects. Its canonical text is its identity: two things are one exactly when
their locators are equal under the current canonicalizer.

| Kind | Canonical form |
|---|---|
| `url:` | the URL with scheme and host lowercased, the default port dropped, dot segments removed, escapes of unreserved characters decoded and other escapes uppercased, the declared tracking parameters removed and the remaining query parameters sorted by name (values of one name keep their order); for `http` and `https` |
| `file:` | `<host>/<root id>/<relative path at first sight>`, host lowercased, `.` and empty segments dropped, `..` resolved inside the root |
| `part:` | `<source>/<native message id>/<part path>`: a message attachment |
| `youtube:` | the 11-character video id |
| `spotify:` | `<type>:<id>` for track, album, artist, playlist, episode, show, audiobook, chapter |
| `doi:` | the DOI, lowercased |
| `isbn:` | the ISBN-13; an ISBN-10 is converted; check digits are verified |
| `arxiv:` | the identifier, new or old style, with its version suffix when one was written |
| `booru:` | `<site>/<post number>` for danbooru, e621, gelbooru, safebooru, yandere, konachan |
| `github:` | `<owner>/<repository>`, lowercased, `.git` dropped |
| `wikipedia:` | `<language>/<title>` under MediaWiki's title rule: underscores for spaces, runs collapsed, the first letter upper case |

- **A URL that a catalog pattern names becomes the catalog locator.**
  `youtube.com/watch?v=X`, `youtu.be/X`, the embed and shorts forms, and a
  Takeout watch record's URL all give `youtube:X`. This is exact
  normalisation of one identifier: no time window, no similarity, no guessed
  equivalence. A pattern whose output is not a valid identifier does not
  match, and the URL stays a `url:` locator.
- **Nothing is guessed.** Only declared tracking parameters (`utm_*`,
  `fbclid`, `gclid`, `dclid`, `gbraid`, `wbraid`, `msclkid`, `yclid`,
  `twclid`, `ttclid`, `li_fat_id`, `igshid`, `mc_cid`, `mc_eid`, `_hsenc`,
  `_hsmi`, `mkt_tok`, `_gl`) are removed; `www.` stays; a trailing slash
  stays.
- **The fragment becomes the selector.** A URL's fragment that is selector
  text (`#t=30`) is parsed as one; any other fragment is kept as an `id=`
  part. A locator takes no revision, so `@` is plain inside it
  (`medium.com/@name`, `node_modules/@types`).
- **The original address is kept by the product** beside the locator; the
  canonical form decides identity only.
- **Catalog web addresses.** `Locator::web_url` gives the address a person
  opens (`https://www.youtube.com/watch?v=X`), with the selector as its
  fragment; files, attachments and ISBNs have none.
- **Versioned.** `locator::VERSION` names the canonicalizer. Any changed
  output for any input is a new version, and the table in
  `src/shared_cases/locator-cases.txt` pins the outputs of the current one. A product
  that keyed things with an older version recomputes them by replay; it never
  edits a key in place.

## URL patterns

`UrlPattern` declares a URL shape and the reference text it becomes. The
shared catalog patterns are declared with it, and so is each product's
provider-URL table:

```rust
UrlPattern::new("{language}.wikipedia.org", "/wiki/{title*}", &[], "wikipedia:{language}/{title}")
UrlPattern::new("chatgpt.com", "/c/{id}", &[], "session:chatgpt:{id}") // Polylogue's
```

`{name}` captures one host label, path segment or query value, with literal
text around it in the same piece (`intl-{locale}`, `{id}.pdf`); `{name*}`
as the last path piece captures the rest of the path. Required query
parameters are named; others are ignored. The scheme is not part of the
shape. Captures enter the output still percent-encoded, and the output is
parsed as ordinary reference text, which decodes and checks it. A malformed
pattern is refused when declared. Output templates use the escaped object
grammar (`result:{id}#ordinal={row}`), independently of a kind's human local
form. A pattern's selector and the URL fragment's distinct selector parts are
combined; a repeated part is refused rather than replacing either selection.

A provider that uses the URL segment's literal spelling as its native ID
declares `with_literal_capture("id")` on its pattern. The shared substitution
escapes that capture before the ordinary reference parse, preserving `%2F`,
`%25` and `@` as identity text. Other captures keep their decoded meaning.
The declaration is refused if the pattern has no capture by that name.
Patterns with a literal capture match the original URL, preserving escape
spelling before locator canonicalization; other patterns keep matching the
canonical URL. A URL fragment stays the reference's separate selector.

## The catalog

`ProductCatalog` is what one product declares: its kinds with their web
sub-views, its web base URL from deployment configuration, and its
provider-URL patterns. `Catalog` holds the declarations of both products as
read from one of them (`home`). A product whose declarations are absent is
not checked: its kinds are accepted as written, and its references render as
text.

### The route catalog

An object's web address follows from its reference:
`<base>/<kind>/<id>[@<revision>][/<sub-view>][#<selector>]`, with the
identity and revision percent-encoded once as one path segment. Read views
and toggles are query parameters, outside the reference.

- `Catalog::link` gives the owner's address, or text with the reason:
  `Sinex not configured` (no base URL here), no web view (a file, an
  attachment, an ISBN). A locator links to its catalog web address.
- `Catalog::url` adds a sub-view, refused when the kind does not declare it.
- `Catalog::route` reads an address back: a full URL under a configured
  base, or a path in the home product. Collection, status and other
  non-object routes are the product's own and read back as `None`.
