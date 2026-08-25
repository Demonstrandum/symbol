use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;

#[derive(Clone, Copy)]
pub enum Policy {
    Revalidate,
    Immutable,
}

impl Policy {
    pub const fn value(self) -> &'static str {
        match self {
            Self::Revalidate => "no-cache",
            Self::Immutable => "public, max-age=31536000, immutable",
        }
    }
}

pub struct Representation {
    pub body: Bytes,
    pub content_type: HeaderValue,
    pub policy: Policy,
    pub vary: Option<HeaderValue>,
    pub link: Option<HeaderValue>,
    pub nosniff: bool,
    pub etag: Option<String>,
}

impl Representation {
    pub fn new(body: impl Into<Bytes>, content_type: &'static str) -> Self {
        Self {
            body: body.into(),
            content_type: HeaderValue::from_static(content_type),
            policy: Policy::Revalidate,
            vary: None,
            link: None,
            nosniff: false,
            etag: None,
        }
    }
}

pub fn strong_etag(bytes: &[u8]) -> String {
    format!("\"{}\"", blake3::hash(bytes).to_hex())
}

pub fn not_modified(
    request: &HeaderMap,
    etag: &str,
    policy: Policy,
    vary: Option<&HeaderValue>,
) -> Option<Response> {
    if !etag_matches(request, etag) {
        return None;
    }
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::NOT_MODIFIED;
    let headers = response.headers_mut();
    headers.insert(
        header::ETAG,
        HeaderValue::from_str(etag).expect("valid ETag"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(policy.value()),
    );
    if let Some(vary) = vary {
        headers.insert(header::VARY, vary.clone());
    }
    Some(response)
}

pub fn respond(request: &HeaderMap, representation: Representation) -> Response {
    let etag = representation
        .etag
        .unwrap_or_else(|| strong_etag(&representation.body));
    if let Some(response) = not_modified(
        request,
        &etag,
        representation.policy,
        representation.vary.as_ref(),
    ) {
        return response;
    }

    let mut response = Response::new(Body::from(representation.body));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, representation.content_type);
    headers.insert(
        header::ETAG,
        HeaderValue::from_str(&etag).expect("valid ETag"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(representation.policy.value()),
    );
    if let Some(vary) = representation.vary {
        headers.insert(header::VARY, vary);
    }
    if let Some(link) = representation.link {
        headers.insert(header::LINK, link);
    }
    if representation.nosniff {
        headers.insert(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        );
    }
    response
}

fn etag_matches(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|value| {
            value.split(',').any(|candidate| {
                let candidate = candidate.trim();
                candidate == "*" || candidate == etag || candidate.strip_prefix("W/") == Some(etag)
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn if_none_match_accepts_lists_wildcards_and_weak_tags() {
        let etag = "\"abc\"";
        for value in ["\"other\", \"abc\"", "*", "W/\"abc\""] {
            let mut headers = HeaderMap::new();
            headers.insert(header::IF_NONE_MATCH, HeaderValue::from_static(value));
            assert!(etag_matches(&headers, etag));
        }

        let mut headers = HeaderMap::new();
        headers.append(header::IF_NONE_MATCH, HeaderValue::from_static("\"other\""));
        headers.append(header::IF_NONE_MATCH, HeaderValue::from_static("\"abc\""));
        assert!(etag_matches(&headers, etag));
    }

    #[test]
    fn generated_response_returns_304_with_cache_headers() {
        let body = Bytes::from_static(b"hello");
        let etag = strong_etag(&body);
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, HeaderValue::from_str(&etag).unwrap());
        let mut representation = Representation::new(body, "text/plain");
        representation.vary = Some(HeaderValue::from_static("Accept"));
        let response = respond(&headers, representation);

        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(response.headers()[header::ETAG], etag);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
        assert_eq!(response.headers()[header::VARY], "Accept");
    }
}
