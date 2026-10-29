//! Framework-neutral asset and security response layer. Wrap every route,
//! including errors, redirects, and not-found responses, with `secure`.
use http::{HeaderValue, Method, Request, Response, StatusCode, header};
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'none'; script-src 'self'; style-src 'self'; font-src 'self'; img-src 'self' data:; connect-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'";
pub fn secure<B>(mut response: Response<B>) -> Response<B> {
    let headers = response.headers_mut();
    headers.insert(
        "content-security-policy",
        HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    );
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    response
}
pub fn document(markup: maud::Markup) -> Response<Vec<u8>> {
    let mut response = Response::new(markup.into_string().into_bytes());
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    secure(response)
}
/// Exact route matching. Query strings do not affect asset identity; unknown paths
/// return 404 instead of falling through to a file system.
pub fn serve<B>(request: &Request<B>) -> Response<Vec<u8>> {
    serve_with_theme(request, None)
}
pub fn serve_with_theme<B>(
    request: &Request<B>,
    theme: Option<&crate::theme::Theme>,
) -> Response<Vec<u8>> {
    let theme_css = theme
        .and_then(|theme| theme.stylesheet.as_deref())
        .unwrap_or("");
    let asset: Option<(&str, &[u8])> = match request.uri().path() {
        "/assets/theme.css" => Some(("text/css; charset=utf-8", theme_css.as_bytes())),
        "/assets/tokens.css" => Some((
            "text/css; charset=utf-8",
            include_bytes!("../assets/tokens.css"),
        )),
        "/assets/shell.css" => Some((
            "text/css; charset=utf-8",
            include_bytes!("../assets/shell.css"),
        )),
        "/assets/shell.js" => Some((
            "text/javascript; charset=utf-8",
            include_bytes!("../assets/shell.js"),
        )),
        "/assets/fonts/IBMPlexSans-Regular.woff2" => Some((
            "font/woff2",
            include_bytes!("../assets/fonts/IBMPlexSans-Regular.woff2"),
        )),
        "/assets/fonts/IBMPlexMono-Regular.woff2" => Some((
            "font/woff2",
            include_bytes!("../assets/fonts/IBMPlexMono-Regular.woff2"),
        )),
        _ => None,
    };
    let mut response = Response::new(Vec::new());
    if request.method() != Method::GET && request.method() != Method::HEAD {
        *response.status_mut() = StatusCode::METHOD_NOT_ALLOWED;
        response
            .headers_mut()
            .insert(header::ALLOW, HeaderValue::from_static("GET, HEAD"));
    } else if let Some((content_type, bytes)) = asset {
        let etag = format!("\"{}-{:016x}\"", bytes.len(), checksum(bytes));
        response.headers_mut().insert(
            header::ETAG,
            HeaderValue::from_str(&etag).expect("hex is a header value"),
        );
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_str(content_type).expect("constant MIME type"),
        );
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=0, must-revalidate"),
        );
        if request
            .headers()
            .get(header::IF_NONE_MATCH)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value
                    .split(',')
                    .any(|tag| tag.trim() == etag || tag.trim() == "*")
            })
        {
            *response.status_mut() = StatusCode::NOT_MODIFIED;
        } else {
            response.headers_mut().insert(
                header::CONTENT_LENGTH,
                HeaderValue::from_str(&bytes.len().to_string()).expect("length is a header value"),
            );
            if request.method() == Method::GET {
                *response.body_mut() = bytes.to_vec();
            }
        }
    } else {
        *response.status_mut() = StatusCode::NOT_FOUND;
    }
    secure(response)
}
// A deterministic cache validator, not a security identity.
fn checksum(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}
