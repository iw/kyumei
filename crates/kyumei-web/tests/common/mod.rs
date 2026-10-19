//! Sending requests to the router as a browser would.

use topcoat::router::{Body, Method, Router, StatusCode, header, request::Request, to_bytes};

pub struct Reply {
    pub status: StatusCode,
    pub location: String,
    pub content_type: String,
    pub body: String,
}

pub async fn send(
    router: &Router,
    method: Method,
    path: &str,
    form: Option<&[(&str, &str)]>,
) -> Reply {
    let mut b = Request::<()>::builder().method(method).uri(path);
    let body = match form {
        Some(fields) => {
            b = b.header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
            Body::from(encode(fields))
        }
        None => Body::default(),
    };
    let response = router.handle(b.body(body).expect("request")).await;
    let (parts, body) = response.into_parts();
    let get = |h| {
        parts
            .headers
            .get(h)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned()
    };
    Reply {
        status: parts.status,
        location: get(header::LOCATION),
        content_type: get(header::CONTENT_TYPE),
        body: String::from_utf8_lossy(&to_bytes(body, usize::MAX).await.expect("body"))
            .into_owned(),
    }
}

fn encode(fields: &[(&str, &str)]) -> String {
    fields
        .iter()
        .map(|(k, v)| format!("{}={}", enc(k), enc(v)))
        .collect::<Vec<_>>()
        .join("&")
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}
