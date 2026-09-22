//! Serving for the prerendered special pages.
//!
//! The guide and the five API manuals are rendered at build time by
//! `generation::pages` and `generation::docs`, so this module only negotiates a
//! flavour, fills in `${host}`, and builds the response. One table, one code
//! path: before this the guide parsed markdown in-process while the manuals
//! were served verbatim with no substitution at all.

use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::Response;

use crate::http_cache::{self, Representation};

macro_rules! generated_page {
    ($name:literal) => {
        include_str!(concat!(env!("OUT_DIR"), "/", $name))
    };
}

/// The variables the pages may use.
///
/// Substitution is allow-listed rather than shell-like: an unknown `${...}` is
/// left exactly as written, so the `${SYMBOL_BASE}` in the manuals' curl
/// examples survives, and a typo stays visible instead of silently emptying.
///
/// `${host}` is the full origin; `${hostname}` is its bare authority, which is
/// what an HTTP `Host:` header needs.
const HOST: &str = "${host}";
const HOSTNAME: &str = "${hostname}";

/// Strips the scheme, and any path, from a validated public URL.
fn authority(host: &str) -> &str {
    let after_scheme = host.split_once("://").map_or(host, |(_, rest)| rest);
    after_scheme
        .split_once('/')
        .map_or(after_scheme, |(authority, _)| authority)
}

const HTML_TYPE: &str = "text/html; charset=utf-8";
const PLAIN_TYPE: &str = "text/plain; charset=utf-8";
const MARKDOWN_TYPE: &str = "text/markdown; charset=utf-8";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    Html,
    Plain,
    Man,
}

/// Every page served from a prerendered template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Special {
    Guide,
    ApiIndex,
    ApiJavaScript,
    ApiPython,
    ApiShell,
    ApiProtocol,
}

struct Template {
    html: &'static str,
    plain: &'static str,
    man: &'static str,
    /// Content type for the plain and man flavours.
    plain_type: &'static str,
    link_html: &'static str,
    link_plain: &'static str,
}

impl Special {
    pub const ALL: [Self; 6] = [
        Self::Guide,
        Self::ApiIndex,
        Self::ApiJavaScript,
        Self::ApiPython,
        Self::ApiShell,
        Self::ApiProtocol,
    ];

    const fn index(self) -> usize {
        match self {
            Self::Guide => 0,
            Self::ApiIndex => 1,
            Self::ApiJavaScript => 2,
            Self::ApiPython => 3,
            Self::ApiShell => 4,
            Self::ApiProtocol => 5,
        }
    }

    const fn template(self) -> Template {
        match self {
            Self::Guide => Template {
                html: generated_page!("docs.html"),
                plain: generated_page!("docs.plain"),
                man: generated_page!("docs.man"),
                plain_type: PLAIN_TYPE,
                link_html: "</>; rel=\"alternate\"; type=\"text/plain\"",
                link_plain: "</>; rel=\"alternate\"; type=\"text/html\"",
            },
            Self::ApiIndex => manual(
                generated_page!("api-doc-index.html"),
                generated_page!("api-doc-index.md"),
                "</API/>; rel=\"canonical\"",
            ),
            Self::ApiJavaScript => manual(
                generated_page!("api-doc-js.html"),
                generated_page!("api-doc-js.md"),
                "</API/JS>; rel=\"canonical\"",
            ),
            Self::ApiPython => manual(
                generated_page!("api-doc-python.html"),
                generated_page!("api-doc-python.md"),
                "</API/PY>; rel=\"canonical\"",
            ),
            Self::ApiShell => manual(
                generated_page!("api-doc-shell.html"),
                generated_page!("api-doc-shell.md"),
                "</API/SH>; rel=\"canonical\"",
            ),
            Self::ApiProtocol => manual(
                generated_page!("api-doc-protocol.html"),
                generated_page!("api-doc-protocol.md"),
                "</API/CURL>; rel=\"canonical\"",
            ),
        }
    }
}

/// A manual serves its markdown for both non-HTML flavours.
const fn manual(html: &'static str, markdown: &'static str, canonical: &'static str) -> Template {
    Template {
        html,
        plain: markdown,
        man: markdown,
        plain_type: MARKDOWN_TYPE,
        link_html: canonical,
        link_plain: canonical,
    }
}

struct Bodies {
    html: String,
    plain: String,
    man: String,
}

/// Every page with `${host}` resolved for this deployment.
///
/// Built once when the server starts rather than per request: the public URL
/// is fixed for the life of the process, and the protocol manual is large
/// enough that substituting it per request was pure waste.
pub struct Rendered(Vec<Bodies>);

impl Rendered {
    #[must_use]
    pub fn new(host: &str) -> Self {
        let name = authority(host);
        let escaped_host = maud::html! { (host) }.into_string();
        let escaped_name = maud::html! { (name) }.into_string();
        let fill = |template: &str, host: &str, name: &str| {
            template.replace(HOST, host).replace(HOSTNAME, name)
        };
        Self(
            Special::ALL
                .iter()
                .map(|page| {
                    let template = page.template();
                    Bodies {
                        html: fill(template.html, &escaped_host, &escaped_name),
                        plain: fill(template.plain, host, name),
                        man: fill(template.man, host, name),
                    }
                })
                .collect(),
        )
    }

    fn body(&self, page: Special, flavor: Flavor) -> &str {
        let bodies = &self.0[page.index()];
        match flavor {
            Flavor::Html => &bodies.html,
            Flavor::Plain => &bodies.plain,
            Flavor::Man => &bodies.man,
        }
    }

    /// The plain guide, used for the `/HASH` digest.
    #[must_use]
    pub fn guide_plain(&self) -> &str {
        self.body(Special::Guide, Flavor::Plain)
    }
}

/// Negotiates a flavour and returns the prerendered page.
pub fn respond(headers: &HeaderMap, rendered: &Rendered, page: Special) -> Response {
    let flavor = negotiate(headers);
    let template = page.template();
    let (content_type, link) = match flavor {
        Flavor::Html => (HTML_TYPE, template.link_html),
        Flavor::Plain | Flavor::Man => (template.plain_type, template.link_plain),
    };
    let mut representation =
        Representation::new(rendered.body(page, flavor).to_owned(), content_type);
    representation.vary = Some(HeaderValue::from_static("Accept, User-Agent"));
    representation.link = Some(HeaderValue::from_static(link));
    http_cache::respond(headers, representation)
}

pub fn negotiate(headers: &HeaderMap) -> Flavor {
    accept_flavor(header_str(headers, header::ACCEPT)).unwrap_or_else(|| {
        let ua = header_str(headers, header::USER_AGENT);
        if looks_like_browser(ua) {
            Flavor::Html
        } else if looks_like_cli(ua) {
            Flavor::Man
        } else {
            Flavor::Plain
        }
    })
}

fn header_str(headers: &HeaderMap, name: header::HeaderName) -> &str {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
}

fn looks_like_browser(ua: &str) -> bool {
    let ua = ua.to_ascii_lowercase();
    ua.contains("mozilla") || ua.contains("browser")
}

fn looks_like_cli(ua: &str) -> bool {
    let ua = ua.to_ascii_lowercase();
    ua.contains("curl/") || ua.contains("wget") || ua.contains("httpie")
}

fn accept_flavor(accept: &str) -> Option<Flavor> {
    if accept.is_empty() {
        return None;
    }
    let mut html_q: Option<f32> = None;
    let mut plain_q: Option<f32> = None;
    for part in accept.split(',') {
        let mut bits = part.split(';');
        let media = bits.next()?.trim();
        let mut q = 1.0f32;
        for param in bits {
            if let Some(v) = param.trim().strip_prefix("q=") {
                q = v.parse().unwrap_or(0.0);
            }
        }
        match media {
            "text/html" | "application/xhtml+xml" => html_q = Some(html_q.unwrap_or(0.0).max(q)),
            "text/plain" | "text/markdown" | "text/x-markdown" => {
                plain_q = Some(plain_q.unwrap_or(0.0).max(q));
            }
            _ => {}
        }
    }
    match (html_q, plain_q) {
        (Some(h), Some(p)) if p > h => Some(Flavor::Plain),
        (Some(_), _) => Some(Flavor::Html),
        (None, Some(_)) => Some(Flavor::Plain),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accept(value: &'static str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT, HeaderValue::from_static(value));
        headers
    }

    async fn body_of(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[test]
    fn accept_plain_beats_html_when_q_higher() {
        assert_eq!(
            accept_flavor("text/plain, text/html;q=0.1"),
            Some(Flavor::Plain)
        );
        assert_eq!(
            accept_flavor("text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"),
            Some(Flavor::Html)
        );
        assert_eq!(accept_flavor("*/*"), None);
        assert_eq!(accept_flavor("text/plain"), Some(Flavor::Plain));
    }

    #[test]
    fn curl_is_not_a_browser() {
        assert!(!looks_like_browser("curl/8.5.0"));
        assert!(looks_like_cli("curl/8.5.0"));
        assert!(!looks_like_cli(
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/120.0.0.0"
        ));
        assert!(looks_like_browser(
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/120.0.0.0"
        ));
    }

    #[test]
    fn authority_drops_scheme_and_path() {
        assert_eq!(authority("https://symbol.example"), "symbol.example");
        assert_eq!(authority("http://symbol"), "symbol");
        assert_eq!(
            authority("https://symbol.example:8443"),
            "symbol.example:8443"
        );
        assert_eq!(authority("symbol.example"), "symbol.example");
    }

    #[test]
    fn no_placeholder_survives_on_any_page_or_flavour() {
        let rendered = Rendered::new("https://symbol.test");
        for page in Special::ALL {
            for flavor in [Flavor::Html, Flavor::Plain, Flavor::Man] {
                let body = rendered.body(page, flavor);
                assert!(
                    !body.contains(HOST),
                    "{page:?}/{flavor:?} still contains ${{host}}"
                );
                assert!(
                    !body.contains(HOSTNAME),
                    "{page:?}/{flavor:?} still contains ${{hostname}}"
                );
                assert!(
                    !body.contains("symbol.example"),
                    "{page:?}/{flavor:?} still advertises the placeholder host"
                );
            }
        }
    }

    #[test]
    fn substitution_is_allow_listed() {
        let rendered = Rendered::new("https://symbol.test");
        // The Python and shell manuals document a variable the reader exports.
        // It must survive verbatim rather than being expanded or emptied,
        // while `${host}` beside it is still replaced.
        for page in [Special::ApiPython, Special::ApiShell] {
            let body = rendered.body(page, Flavor::Plain);
            assert!(
                body.contains("${SYMBOL_BASE}"),
                "{page:?} lost an unrelated shell variable"
            );
        }
        let python = rendered.body(Special::ApiPython, Flavor::Plain);
        assert!(python.contains("origin=\"https://symbol.test\""));

        // The protocol manual sets it from the real origin instead.
        let protocol = rendered.body(Special::ApiProtocol, Flavor::Plain);
        assert!(protocol.contains("SYMBOL_BASE=https://symbol.test"));
        assert!(protocol.contains("Host: symbol.test"));
    }

    #[test]
    fn guide_keeps_its_rendered_shape_after_substitution() {
        let rendered = Rendered::new("http://symbol");
        let plain = rendered.guide_plain();
        assert!(plain.starts_with("SYMBOL(1)"));
        assert!(plain.contains("symbol - tiny static web hosting on http://symbol."));
        assert!(plain.contains("http://symbol/API/JS"));
        assert!(!plain.contains('\u{8}'));

        let man = rendered.body(Special::Guide, Flavor::Man);
        assert!(man.contains('\u{8}'));
        assert!(man.contains("http://symbol/API/CURL"));

        let html = rendered.body(Special::Guide, Flavor::Html);
        assert!(html.contains("<a href=\"http://symbol/API/JS\">"));
        assert!(html.contains("class=\"cmd\">symbol</span>"));
        assert!(html.contains("<h2>NAME</h2>"));
    }

    #[test]
    fn manuals_carry_the_real_host_and_their_canonical_link() {
        let rendered = Rendered::new("https://symbol.test");
        let headers = accept("text/html");
        let response = respond(&headers, &rendered, Special::ApiProtocol);
        assert_eq!(
            response.headers()[header::LINK],
            "</API/CURL>; rel=\"canonical\""
        );
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
    }

    #[tokio::test]
    async fn manuals_still_negotiate_markdown_for_non_browsers() {
        let rendered = Rendered::new("https://symbol.test");
        let response = respond(&accept("text/markdown"), &rendered, Special::ApiPython);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/markdown; charset=utf-8"
        );
        assert_eq!(response.headers()[header::VARY], "Accept, User-Agent");
        let body = body_of(response).await;
        assert!(body.contains("https://symbol.test"));
        assert!(!body.contains("${host}"));
    }

    #[test]
    fn every_special_page_supports_conditional_requests() {
        let rendered = Rendered::new("https://symbol.test");
        for page in Special::ALL {
            let response = respond(&HeaderMap::new(), &rendered, page);
            let etag = response.headers()[header::ETAG].clone();
            let mut headers = HeaderMap::new();
            headers.insert(header::IF_NONE_MATCH, etag);
            let response = respond(&headers, &rendered, page);
            assert_eq!(
                response.status(),
                axum::http::StatusCode::NOT_MODIFIED,
                "{page:?} did not revalidate"
            );
        }
    }

    #[test]
    fn distinct_hosts_produce_distinct_bodies() {
        let a = Rendered::new("https://one.test");
        let b = Rendered::new("https://two.test");
        assert_ne!(a.guide_plain(), b.guide_plain());
        assert!(a.guide_plain().contains("one.test"));
        assert!(b.guide_plain().contains("two.test"));
    }
}
