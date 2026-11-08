//! Locators: the canonical native address of a thing outside both products'
//! own objects (a file, a web page, a catalog object, a message attachment),
//! and the canonicalizer that produces them.
//!
//! The canonicalizer is data (the tracking parameters, the catalog patterns)
//! plus small pure functions, versioned by [`VERSION`]. Both products key
//! things by its output, so they compute byte-identical keys. Changing any
//! output for any input is a new version; a product recomputes what it keyed
//! with the old version and never edits keys in place.

use crate::escape;
use crate::pattern::UrlPattern;
use crate::selector::{Selector, SelectorError};
use std::fmt;
use std::sync::LazyLock;
use url::Url;

/// The canonicalizer's version. Pinned by the shared case table
/// ([`crate::shared_cases::shared_locator_cases`]): changing an output without
/// raising this number fails that table's test.
pub const VERSION: u32 = 1;

/// The kinds of locator. Their names are reserved: no product declares an
/// object kind with one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LocatorKind {
    /// `url:<canonical absolute URL>`
    Url,
    /// `file:<host>/<root id>/<relative path at first sight>`
    File,
    /// `part:<source>/<native message id>/<part path>`, a message attachment
    Part,
    /// `youtube:<video id>`
    Youtube,
    /// `spotify:<type>:<id>`
    Spotify,
    /// `doi:<lowercased DOI>`
    Doi,
    /// `isbn:<ISBN-13>`
    Isbn,
    /// `arxiv:<identifier>`, with its version suffix when one was written
    Arxiv,
    /// `booru:<site>/<post id>`
    Booru,
    /// `github:<owner>/<repository>`
    Github,
    /// `wikipedia:<language>/<title>`
    Wikipedia,
}

impl LocatorKind {
    pub const ALL: [Self; 11] = [
        Self::Url,
        Self::File,
        Self::Part,
        Self::Youtube,
        Self::Spotify,
        Self::Doi,
        Self::Isbn,
        Self::Arxiv,
        Self::Booru,
        Self::Github,
        Self::Wikipedia,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Url => "url",
            Self::File => "file",
            Self::Part => "part",
            Self::Youtube => "youtube",
            Self::Spotify => "spotify",
            Self::Doi => "doi",
            Self::Isbn => "isbn",
            Self::Arxiv => "arxiv",
            Self::Booru => "booru",
            Self::Github => "github",
            Self::Wikipedia => "wikipedia",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }

    /// Whether this is a catalog form: an identifier some outside catalog
    /// assigns, which URLs of that catalog fold into.
    pub fn is_catalog(self) -> bool {
        !matches!(self, Self::Url | Self::File | Self::Part)
    }
}

/// A canonical locator. Two locators are equal exactly when they name the
/// same thing under the current canonicalizer.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Locator {
    kind: LocatorKind,
    /// The canonical value after `<kind>:`, decoded.
    value: String,
}

/// A locator with the selector its address carried (a URL's fragment, or
/// the text after `#`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Located {
    pub locator: Locator,
    pub selector: Option<Selector>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LocatorError {
    #[error("`{text}` is not an absolute URL: {reason}")]
    NotAbsoluteUrl { text: String, reason: String },
    #[error("`{value}` is not a {kind} locator; expected {expected}")]
    Invalid {
        kind: &'static str,
        value: String,
        expected: &'static str,
    },
    #[error("the {0} locator's escapes do not decode to UTF-8 text")]
    NotUtf8(&'static str),
    #[error(transparent)]
    Selector(#[from] SelectorError),
}

impl Locator {
    pub fn kind(&self) -> LocatorKind {
        self.kind
    }

    /// The canonical value after `<kind>:`, decoded.
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Parses an address into its canonical locator: locator text,
    /// `<kind>:<value>[#<selector>]`, or an absolute URL. A URL, bare or as a
    /// `url:` value, is canonicalized by [`Locator::from_url`], so it may
    /// become a catalog locator.
    pub fn parse(text: &str) -> Result<Located, LocatorError> {
        let Some((kind, value)) = text
            .split_once(':')
            .and_then(|(name, value)| Some((LocatorKind::from_name(name)?, value)))
        else {
            return Self::from_url(text);
        };
        if kind == LocatorKind::Url {
            return Self::from_url(value);
        }
        let (value, selector) = match value.split_once('#') {
            Some((value, selector)) => (value, Some(Selector::parse(selector)?)),
            None => (value, None),
        };
        let value = escape::decode(value).map_err(|_| LocatorError::NotUtf8(kind.name()))?;
        Ok(Located {
            locator: Self::new(kind, &value)?,
            selector,
        })
    }

    /// A locator from its kind and decoded value, canonicalized. A `url`
    /// value that a catalog pattern names becomes that catalog locator.
    pub fn new(kind: LocatorKind, value: &str) -> Result<Self, LocatorError> {
        if kind == LocatorKind::Url {
            return Self::from_url(value).map(|located| located.locator);
        }
        let value = canonical_value(kind, value).ok_or_else(|| LocatorError::Invalid {
            kind: kind.name(),
            value: value.to_owned(),
            expected: expected(kind),
        })?;
        Ok(Self { kind, value })
    }

    /// Canonicalizes an absolute URL. For `http` and `https`: scheme and host
    /// lowercased, the default port dropped, dot segments removed, escapes of
    /// unreserved characters decoded and other escapes uppercased, the
    /// declared tracking parameters removed, the remaining query parameters
    /// sorted by name (values of one name keep their order). A URL that a
    /// catalog pattern names becomes that catalog locator. The fragment
    /// becomes the selector: parsed as one when it is selector text, kept as
    /// an `id=` part otherwise.
    pub fn from_url(text: &str) -> Result<Located, LocatorError> {
        let mut url = Url::parse(text).map_err(|error| LocatorError::NotAbsoluteUrl {
            text: text.to_owned(),
            reason: error.to_string(),
        })?;
        let selector = url
            .fragment()
            .filter(|fragment| !fragment.is_empty())
            .map(|fragment| {
                Selector::parse(fragment).unwrap_or_else(|_| Selector {
                    id: Some(escape::decode(fragment).unwrap_or_else(|_| fragment.to_owned())),
                    ..Selector::default()
                })
            });
        url.set_fragment(None);
        if matches!(url.scheme(), "http" | "https") {
            let path = normalize_escapes(url.path());
            url.set_path(&path);
            let query = url
                .query()
                .map(canonical_query)
                .filter(|query| !query.is_empty());
            url.set_query(query.as_deref());
            if let Some(locator) = CATALOG_PATTERNS
                .iter()
                .filter_map(|pattern| pattern.apply(&url))
                .find_map(|text| Self::parse(&text).ok())
            {
                return Ok(Located {
                    locator: locator.locator,
                    selector,
                });
            }
        }
        Ok(Located {
            locator: Self {
                kind: LocatorKind::Url,
                value: url.into(),
            },
            selector,
        })
    }

    /// A `file:` locator from a host, a root's identity and a path relative
    /// to that root.
    pub fn file(host: &str, root: &str, path: &str) -> Result<Self, LocatorError> {
        Self::new(LocatorKind::File, &format!("{host}/{root}/{path}"))
    }

    /// A `part:` locator for an attachment of a message.
    pub fn part(source: &str, message: &str, path: &str) -> Result<Self, LocatorError> {
        Self::new(LocatorKind::Part, &format!("{source}/{message}/{path}"))
    }

    /// The web address a person opens for this locator, with the selector as
    /// its fragment. `None` for files, attachments and ISBNs, which have no
    /// one web address.
    pub fn web_url(&self, selector: Option<&Selector>) -> Option<String> {
        let value = &self.value;
        let address = match self.kind {
            LocatorKind::Url => value.clone(),
            LocatorKind::File | LocatorKind::Part | LocatorKind::Isbn => return None,
            LocatorKind::Youtube => format!("https://www.youtube.com/watch?v={value}"),
            LocatorKind::Spotify => {
                let (kind, id) = value.split_once(':')?;
                format!("https://open.spotify.com/{kind}/{id}")
            }
            LocatorKind::Doi => format!("https://doi.org/{}", encode_path(value)),
            LocatorKind::Arxiv => format!("https://arxiv.org/abs/{value}"),
            LocatorKind::Booru => {
                let (site, id) = value.split_once('/')?;
                let site = BOORU_SITES.iter().find(|known| known.name == site)?;
                site.post_url.replace("{id}", id)
            }
            LocatorKind::Github => format!("https://github.com/{value}"),
            LocatorKind::Wikipedia => {
                let (language, title) = value.split_once('/')?;
                format!(
                    "https://{language}.wikipedia.org/wiki/{}",
                    encode_path(title)
                )
            }
        };
        Some(match selector {
            None => address,
            Some(selector) => format!("{address}#{}", web_fragment(selector)),
        })
    }
}

impl fmt::Display for Locator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            // A canonical URL holds no `#` and its `%` are escapes already.
            LocatorKind::Url => write!(f, "url:{}", self.value),
            kind => write!(
                f,
                "{}:{}",
                kind.name(),
                escape::encode(&self.value, escape::plain_in_locator)
            ),
        }
    }
}

impl fmt::Display for Located {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.locator)?;
        if let Some(selector) = &self.selector {
            write!(f, "#{selector}")?;
        }
        Ok(())
    }
}

/// A selector as a web address's fragment: a kept element id as the
/// page wrote it, otherwise the selector text (browsers read `t=` and
/// `xywh=` as media fragments).
fn web_fragment(selector: &Selector) -> String {
    match selector {
        Selector {
            id: Some(id),
            lines: None,
            time: None,
            rectangle: None,
            text_range: None,
            quote: None,
        } => escape::encode(id, |character| {
            escape::plain_in_path_segment(character) || matches!(character, '/' | '?' | '@')
        }),
        _ => selector.to_string(),
    }
}

fn encode_path(text: &str) -> String {
    escape::encode(text, |character| {
        escape::plain_in_path_segment(character) || character == '/'
    })
}

/// Query parameters that only record how a link was shared. Removing them
/// never changes what the URL names; nothing else is removed.
const TRACKING_PARAMETERS: &[&str] = &[
    "fbclid",
    "gclid",
    "dclid",
    "gbraid",
    "wbraid",
    "msclkid",
    "yclid",
    "twclid",
    "ttclid",
    "li_fat_id",
    "igshid",
    "mc_cid",
    "mc_eid",
    "_hsenc",
    "_hsmi",
    "mkt_tok",
    "_gl",
];

fn is_tracking(name: &str) -> bool {
    name.starts_with("utm_") || TRACKING_PARAMETERS.contains(&name)
}

fn canonical_query(query: &str) -> String {
    let mut pairs: Vec<String> = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(normalize_escapes)
        .filter(|pair| !is_tracking(pair.split_once('=').map_or(pair.as_str(), |(name, _)| name)))
        .collect();
    pairs.sort_by(|left, right| {
        let name = |pair: &str| {
            pair.split_once('=')
                .map_or(pair, |(name, _)| name)
                .to_owned()
        };
        name(left).cmp(&name(right))
    });
    pairs.join("&")
}

/// Decodes escapes of unreserved characters and uppercases the rest
/// (RFC 3986 §6.2.2.1–2).
fn normalize_escapes(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && bytes[index + 1].is_ascii_hexdigit()
            && bytes[index + 2].is_ascii_hexdigit()
        {
            let value =
                u8::from_str_radix(&text[index + 1..index + 3], 16).expect("two hex digits");
            if escape::unreserved(value as char) {
                out.push(value as char);
            } else {
                out.push('%');
                out.push_str(&text[index + 1..index + 3].to_ascii_uppercase());
            }
            index += 3;
        } else {
            let c = text[index..]
                .chars()
                .next()
                .expect("index is on a character");
            out.push(c);
            index += c.len_utf8();
        }
    }
    out
}

fn expected(kind: LocatorKind) -> &'static str {
    match kind {
        LocatorKind::Url => "an absolute URL",
        LocatorKind::File => "<host>/<root id>/<path> with a path that stays inside the root",
        LocatorKind::Part => "<source>/<native message id>/<part path>",
        LocatorKind::Youtube => "an 11-character video id of letters, digits, - and _",
        LocatorKind::Spotify => {
            "<track|album|artist|playlist|episode|show|audiobook|chapter>:<id of letters and digits>"
        }
        LocatorKind::Doi => "10.<registrant>/<suffix>",
        LocatorKind::Isbn => "an ISBN-10 or ISBN-13 with a valid check digit",
        LocatorKind::Arxiv => "<yymm>.<number>[v<n>] or <archive>[.<class>]/<7 digits>[v<n>]",
        LocatorKind::Booru => "<site>/<post number> for a declared site",
        LocatorKind::Github => "<owner>/<repository>",
        LocatorKind::Wikipedia => "<language>/<title>",
    }
}

/// The canonical value of a non-URL locator, or `None` when the value is not
/// one of its kind.
fn canonical_value(kind: LocatorKind, value: &str) -> Option<String> {
    match kind {
        LocatorKind::Url => None,
        LocatorKind::File => {
            let mut parts = value.splitn(3, '/');
            let host = parts.next().filter(|host| !host.is_empty())?.to_lowercase();
            let root = parts.next().filter(|root| !root.is_empty())?;
            let mut path: Vec<&str> = Vec::new();
            for segment in parts.next()?.split('/') {
                match segment {
                    "" | "." => {}
                    ".." => {
                        path.pop()?;
                    }
                    segment => path.push(segment),
                }
            }
            (!path.is_empty()).then(|| format!("{host}/{root}/{}", path.join("/")))
        }
        LocatorKind::Part => {
            let mut parts = value.splitn(3, '/');
            let source = parts.next().filter(|source| !source.is_empty())?;
            let message = parts.next().filter(|message| !message.is_empty())?;
            let path = parts.next().filter(|path| !path.is_empty())?;
            Some(format!("{source}/{message}/{path}"))
        }
        LocatorKind::Youtube => (value.len() == 11
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'))
        .then(|| value.to_owned()),
        LocatorKind::Spotify => {
            let (kind, id) = value.split_once(':')?;
            let known = [
                "track",
                "album",
                "artist",
                "playlist",
                "episode",
                "show",
                "audiobook",
                "chapter",
            ];
            (known.contains(&kind)
                && !id.is_empty()
                && id.bytes().all(|byte| byte.is_ascii_alphanumeric()))
            .then(|| value.to_owned())
        }
        LocatorKind::Doi => {
            let doi = value.to_lowercase();
            let (prefix, suffix) = doi.split_once('/')?;
            let registrant = prefix.strip_prefix("10.")?;
            (!suffix.is_empty()
                && !registrant.is_empty()
                && registrant
                    .split('.')
                    .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())))
            .then_some(doi)
        }
        LocatorKind::Isbn => isbn13(value),
        LocatorKind::Arxiv => arxiv_identifier(value).then(|| value.to_owned()),
        LocatorKind::Booru => {
            let (site, post) = value.split_once('/')?;
            BOORU_SITES.iter().find(|known| known.name == site)?;
            let post: u64 = post
                .parse()
                .ok()
                .filter(|_| post.bytes().all(|byte| byte.is_ascii_digit()))?;
            Some(format!("{site}/{post}"))
        }
        LocatorKind::Github => {
            let value = value.to_lowercase();
            let (owner, repository) = value.split_once('/')?;
            let repository = repository.strip_suffix(".git").unwrap_or(repository);
            let owner_ok = !owner.is_empty()
                && owner
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                && !GITHUB_RESERVED.contains(&owner);
            let repository_ok = !repository.is_empty()
                && !matches!(repository, "." | "..")
                && repository
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
            (owner_ok && repository_ok).then(|| format!("{owner}/{repository}"))
        }
        LocatorKind::Wikipedia => {
            let (language, title) = value.split_once('/')?;
            let language = language.to_lowercase();
            if language.is_empty()
                || matches!(language.as_str(), "www" | "m")
                || !language
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            {
                return None;
            }
            let title = wikipedia_title(title)?;
            Some(format!("{language}/{title}"))
        }
    }
}

/// MediaWiki's title rule: spaces are underscores, runs of underscores are
/// one, none at either end, and the first letter is upper case.
fn wikipedia_title(title: &str) -> Option<String> {
    let joined = title
        .replace(' ', "_")
        .split('_')
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    let mut chars = joined.chars();
    let first = chars.next()?;
    Some(first.to_uppercase().chain(chars).collect())
}

fn arxiv_identifier(value: &str) -> bool {
    let identifier = match value.rsplit_once('v') {
        Some((identifier, version))
            if !version.is_empty() && version.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            identifier
        }
        _ => value,
    };
    let all_digits =
        |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    if let Some((month, number)) = identifier.split_once('.')
        && !identifier.contains('/')
    {
        return month.len() == 4
            && all_digits(month)
            && matches!(number.len(), 4 | 5)
            && all_digits(number);
    }
    let Some((archive, number)) = identifier.split_once('/') else {
        return false;
    };
    let (archive, class) = match archive.split_once('.') {
        Some((archive, class)) => (archive, Some(class)),
        None => (archive, None),
    };
    !archive.is_empty()
        && archive
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
        && class.is_none_or(|class| {
            class.len() == 2 && class.bytes().all(|byte| byte.is_ascii_uppercase())
        })
        && number.len() == 7
        && all_digits(number)
}

/// The ISBN-13 of an ISBN-10 or ISBN-13 written with or without hyphens and
/// spaces, when its check digit is right.
fn isbn13(value: &str) -> Option<String> {
    let digits: Vec<u8> = value
        .chars()
        .filter(|character| !matches!(character, '-' | ' '))
        .map(|character| match character {
            '0'..='9' => Some(character as u8 - b'0'),
            'X' | 'x' => Some(10),
            _ => None,
        })
        .collect::<Option<_>>()?;
    let isbn13_check_digit = |first_twelve: &[u8]| {
        let sum: u32 = first_twelve
            .iter()
            .enumerate()
            .map(|(index, &digit)| u32::from(digit) * if index % 2 == 0 { 1 } else { 3 })
            .sum();
        ((10 - sum % 10) % 10) as u8
    };
    let thirteen: Vec<u8> = match digits.len() {
        10 => {
            if digits[..9].contains(&10) {
                return None;
            }
            let sum: u32 = digits
                .iter()
                .enumerate()
                .map(|(index, &digit)| u32::from(digit) * (10 - index as u32))
                .sum();
            if !sum.is_multiple_of(11) {
                return None;
            }
            let mut thirteen = vec![9, 7, 8];
            thirteen.extend_from_slice(&digits[..9]);
            let check = isbn13_check_digit(&thirteen);
            thirteen.push(check);
            thirteen
        }
        13 => {
            if digits.contains(&10)
                || !(digits.starts_with(&[9, 7, 8]) || digits.starts_with(&[9, 7, 9]))
            {
                return None;
            }
            if isbn13_check_digit(&digits[..12]) != digits[12] {
                return None;
            }
            digits
        }
        _ => return None,
    };
    Some(
        thirteen
            .iter()
            .map(|digit| char::from(b'0' + digit))
            .collect(),
    )
}

/// First path names on github.com that are site pages, not owners.
const GITHUB_RESERVED: &[&str] = &[
    "about",
    "apps",
    "collections",
    "contact",
    "customer-stories",
    "enterprise",
    "events",
    "explore",
    "features",
    "issues",
    "login",
    "marketplace",
    "new",
    "notifications",
    "orgs",
    "organizations",
    "pricing",
    "pulls",
    "search",
    "security",
    "settings",
    "site",
    "sponsors",
    "topics",
    "trending",
];

struct BooruSite {
    name: &'static str,
    post_url: &'static str,
}

const BOORU_SITES: &[BooruSite] = &[
    BooruSite {
        name: "danbooru",
        post_url: "https://danbooru.donmai.us/posts/{id}",
    },
    BooruSite {
        name: "e621",
        post_url: "https://e621.net/posts/{id}",
    },
    BooruSite {
        name: "gelbooru",
        post_url: "https://gelbooru.com/index.php?id={id}&page=post&s=view",
    },
    BooruSite {
        name: "safebooru",
        post_url: "https://safebooru.org/index.php?id={id}&page=post&s=view",
    },
    BooruSite {
        name: "yandere",
        post_url: "https://yande.re/post/show/{id}",
    },
    BooruSite {
        name: "konachan",
        post_url: "https://konachan.com/post/show/{id}",
    },
];

/// The shared catalog patterns, tried in order on a canonical `http(s)` URL.
pub static CATALOG_PATTERNS: LazyLock<Vec<UrlPattern>> = LazyLock::new(|| {
    let youtube_hosts = [
        "youtube.com",
        "www.youtube.com",
        "m.youtube.com",
        "music.youtube.com",
    ];
    let mut patterns = Vec::new();
    let mut add = |host: &str, path: &str, query: &[(&str, &str)], output: &str| {
        patterns.push(
            UrlPattern::new(host, path, query, output).expect("a shared pattern is well formed"),
        );
    };
    for host in youtube_hosts {
        add(host, "/watch", &[("v", "{id}")], "youtube:{id}");
        for kind in ["embed", "shorts", "live", "v"] {
            add(host, &format!("/{kind}/{{id}}"), &[], "youtube:{id}");
        }
    }
    add("youtu.be", "/{id}", &[], "youtube:{id}");
    for host in ["youtube-nocookie.com", "www.youtube-nocookie.com"] {
        add(host, "/embed/{id}", &[], "youtube:{id}");
    }
    add(
        "open.spotify.com",
        "/{type}/{id}",
        &[],
        "spotify:{type}:{id}",
    );
    add(
        "open.spotify.com",
        "/intl-{locale}/{type}/{id}",
        &[],
        "spotify:{type}:{id}",
    );
    add(
        "open.spotify.com",
        "/embed/{type}/{id}",
        &[],
        "spotify:{type}:{id}",
    );
    for host in ["doi.org", "dx.doi.org", "www.doi.org"] {
        add(host, "/{doi*}", &[], "doi:{doi}");
    }
    for host in ["arxiv.org", "www.arxiv.org", "export.arxiv.org"] {
        add(host, "/abs/{id*}", &[], "arxiv:{id}");
        add(host, "/pdf/{id*}.pdf", &[], "arxiv:{id}");
        add(host, "/pdf/{id*}", &[], "arxiv:{id}");
    }
    add(
        "danbooru.donmai.us",
        "/posts/{id}",
        &[],
        "booru:danbooru/{id}",
    );
    add("e621.net", "/posts/{id}", &[], "booru:e621/{id}");
    for (host, site) in [("gelbooru.com", "gelbooru"), ("safebooru.org", "safebooru")] {
        add(
            host,
            "/index.php",
            &[("page", "post"), ("s", "view"), ("id", "{id}")],
            &format!("booru:{site}/{{id}}"),
        );
    }
    add("yande.re", "/post/show/{id}", &[], "booru:yandere/{id}");
    add(
        "konachan.com",
        "/post/show/{id}",
        &[],
        "booru:konachan/{id}",
    );
    for host in ["github.com", "www.github.com"] {
        add(
            host,
            "/{owner}/{repository}",
            &[],
            "github:{owner}/{repository}",
        );
        add(
            host,
            "/{owner}/{repository}/",
            &[],
            "github:{owner}/{repository}",
        );
    }
    add(
        "{language}.wikipedia.org",
        "/wiki/{title*}",
        &[],
        "wikipedia:{language}/{title}",
    );
    add(
        "{language}.m.wikipedia.org",
        "/wiki/{title*}",
        &[],
        "wikipedia:{language}/{title}",
    );
    patterns
});
