//! Keeping credentials out of the request log's readers.
//!
//! Grove is the proxy, so it sees every request in full — including the
//! `Authorization` header, the session `Cookie`, and the `password` field of the
//! login form that just went past. It kept all of it verbatim and handed it to
//! anyone who asked: `grove requests`, the GUI's detail view, the curl/`.http`/
//! Pest snippets people paste into issues and test files, and — most of all —
//! the MCP tools, which hand a captured request straight to an AI assistant.
//!
//! That last one is the reason this is not merely untidy. `grove_request`,
//! `grove_request_chain` and `grove_explain` exist to send a failing request
//! somewhere else to be explained. A live session cookie should not go with it.
//!
//! # Where this applies, and where it deliberately does not
//!
//! Redaction happens when a [`crate::reqlog::RequestDetail`] is built — the
//! struct that leaves the daemon. `CapturedRequest`, which `grove replay` uses,
//! is built separately and stays in-process, so replaying a request still sends
//! the real credentials and still works. That split already existed; this module
//! only takes advantage of it.
//!
//! The consequence worth knowing: `grove request <id> --as curl` now emits
//! `Authorization: [redacted]`, so the snippet will not reproduce an
//! authenticated request until you put your own credential back. That is the
//! right default for something whose purpose is to be pasted somewhere.
//!
//! # What it cannot promise
//!
//! Bodies are redacted for JSON and form-encoded shapes, by key. A secret in a
//! body Grove cannot parse — multipart, protobuf, something truncated mid-token
//! — is left alone, because mangling a body a developer is trying to read is its
//! own kind of damage. This narrows the exposure; it does not eliminate it.

use std::borrow::Cow;

/// What a redacted value is replaced with.
///
/// The key or header name is kept: knowing that a request *carried* an
/// `Authorization` header is most of its diagnostic value, and dropping the line
/// entirely would make a redacted request look like an unauthenticated one.
pub const PLACEHOLDER: &str = "[redacted]";

/// Header names whose value is a credential, matched case-insensitively.
const SECRET_HEADERS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "cookie",
    "set-cookie",
    "x-api-key",
    "x-auth-token",
    "x-amz-security-token",
];

/// Substrings that make a header or field name credential-shaped.
///
/// Deliberately not `auth`: `authorization` is already matched exactly, and the
/// bare substring starts catching names that are about authentication without
/// carrying a secret.
const SECRET_FRAGMENTS: &[&str] = &[
    "password",
    "passwd",
    "pwd",
    "secret",
    "token",
    "api_key",
    "api-key",
    "apikey",
    "credential",
    "private_key",
    "private-key",
];

/// Whether a header name identifies a credential.
pub fn is_secret_header(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    SECRET_HEADERS.contains(&lower.as_str()) || contains_secret_fragment(&lower)
}

/// Whether a query parameter or body field name identifies a credential.
pub fn is_secret_field(name: &str) -> bool {
    contains_secret_fragment(&name.to_ascii_lowercase())
}

fn contains_secret_fragment(lower: &str) -> bool {
    SECRET_FRAGMENTS.iter().any(|f| lower.contains(f))
}

/// Redact credential headers in place, keeping their names and order.
pub fn headers(headers: &mut [(String, String)]) {
    for (name, value) in headers.iter_mut() {
        if is_secret_header(name) {
            PLACEHOLDER.clone_into(value);
        }
    }
}

/// Redact credential parameters in a path's query string.
///
/// The recorded path comes from `path_and_query`, so `?api_key=…` is in it.
/// Returns the path unchanged when there is no query to touch.
pub fn path_with_query(path: &str) -> Cow<'_, str> {
    let Some((base, query)) = path.split_once('?') else {
        return Cow::Borrowed(path);
    };
    let redacted = form_encoded(query);
    if redacted == query {
        return Cow::Borrowed(path);
    }
    Cow::Owned(format!("{base}?{redacted}"))
}

/// Redact a request body, as far as its shape can be understood.
///
/// JSON first, then form-encoding; anything else is returned untouched. An
/// unparseable body is left as it is rather than blanked, because a body a
/// developer is reading is worth more intact than uniformly hidden — and
/// blanking every unrecognised body would make the log useless for exactly the
/// requests people reach for it with.
pub fn body(body: &str) -> Cow<'_, str> {
    if body.trim().is_empty() {
        return Cow::Borrowed(body);
    }
    if let Some(json) = json_body(body) {
        return Cow::Owned(json);
    }
    if looks_form_encoded(body) {
        let redacted = form_encoded(body);
        if redacted != body {
            return Cow::Owned(redacted);
        }
    }
    Cow::Borrowed(body)
}

/// Redact a JSON body, returning `None` when it is not JSON.
///
/// Re-serialising rather than patching the text guarantees the result is still
/// valid JSON — a redaction that produced a body no tool could parse would
/// trade one debugging problem for another.
fn json_body(body: &str) -> Option<String> {
    let mut value: serde_json::Value = serde_json::from_str(body).ok()?;
    if !redact_json(&mut value) {
        return None; // nothing secret; keep the original formatting
    }
    serde_json::to_string_pretty(&value).ok()
}

/// Walk a JSON value, replacing the values of credential-named keys.
/// Returns whether anything was replaced.
fn redact_json(value: &mut serde_json::Value) -> bool {
    let mut changed = false;
    match value {
        serde_json::Value::Object(map) => {
            for (key, v) in map.iter_mut() {
                if is_secret_field(key) {
                    // Replace whatever it was — string, number, nested object.
                    // A secret hidden one level down in a `credentials: {...}`
                    // is still a secret.
                    if !matches!(v, serde_json::Value::Null) {
                        *v = serde_json::Value::String(PLACEHOLDER.to_string());
                        changed = true;
                    }
                } else if redact_json(v) {
                    changed = true;
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items.iter_mut() {
                if redact_json(item) {
                    changed = true;
                }
            }
        }
        _ => {}
    }
    changed
}

/// A body that is plausibly `a=1&b=2`, and not JSON or free text.
fn looks_form_encoded(body: &str) -> bool {
    let first = body.split('&').next().unwrap_or_default();
    match first.split_once('=') {
        // A key must be non-empty and free of the whitespace that would mark
        // this as prose rather than a form.
        Some((key, _)) => !key.is_empty() && !key.contains(char::is_whitespace),
        None => false,
    }
}

/// Redact credential values in `a=1&b=2`, preserving order and everything else.
fn form_encoded(input: &str) -> String {
    input
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some((key, _)) if is_secret_field(key) => format!("{key}={PLACEHOLDER}"),
            _ => pair.to_string(),
        })
        .collect::<Vec<_>>()
        .join("&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_headers_are_recognised() {
        for name in [
            "Authorization",
            "authorization",
            "AUTHORIZATION",
            "Cookie",
            "Proxy-Authorization",
            "X-Api-Key",
            "X-Auth-Token",
            "x-refresh-token",
            "X-Client-Secret",
        ] {
            assert!(is_secret_header(name), "{name} should be redacted");
        }
    }

    /// The log has to stay useful: redacting the ordinary headers people debug
    /// with would be its own failure.
    #[test]
    fn ordinary_headers_are_left_alone() {
        for name in [
            "Accept",
            "Content-Type",
            "User-Agent",
            "Host",
            "Referer",
            "X-Requested-With",
            "Accept-Language",
        ] {
            assert!(!is_secret_header(name), "{name} should not be redacted");
        }
    }

    #[test]
    fn header_values_are_replaced_and_names_kept() {
        let mut h = vec![
            ("Authorization".into(), "Bearer sk-live-abcdef".into()),
            ("Accept".into(), "application/json".into()),
            ("Cookie".into(), "laravel_session=eyJpdiI6...".into()),
        ];
        headers(&mut h);
        assert_eq!(h[0], ("Authorization".into(), PLACEHOLDER.to_string()));
        assert_eq!(h[1], ("Accept".into(), "application/json".to_string()));
        assert_eq!(h[2], ("Cookie".into(), PLACEHOLDER.to_string()));
        assert!(
            !h.iter().any(|(_, v)| v.contains("sk-live")),
            "the secret survived: {h:?}"
        );
    }

    #[test]
    fn query_parameters_are_redacted_in_place() {
        let p = path_with_query("/api/users?api_key=sk-live-123&page=2");
        assert_eq!(p, "/api/users?api_key=[redacted]&page=2");
        assert!(!p.contains("sk-live"));
    }

    #[test]
    fn a_path_without_secrets_is_returned_untouched() {
        assert!(matches!(
            path_with_query("/api/users?page=2&sort=name"),
            Cow::Borrowed(_)
        ));
        assert!(matches!(path_with_query("/plain/path"), Cow::Borrowed(_)));
    }

    #[test]
    fn a_login_form_body_loses_only_the_password() {
        let b = body("email=me%40example.com&password=hunter2&remember=on");
        assert_eq!(
            b, "email=me%40example.com&password=[redacted]&remember=on",
            "the other fields are what make the entry worth keeping"
        );
        assert!(!b.contains("hunter2"));
    }

    #[test]
    fn a_json_body_stays_valid_json() {
        let b = body(r#"{"email":"me@example.com","password":"hunter2"}"#);
        assert!(!b.contains("hunter2"));
        let parsed: serde_json::Value =
            serde_json::from_str(&b).expect("redaction must not break the JSON");
        assert_eq!(parsed["email"], "me@example.com");
        assert_eq!(parsed["password"], PLACEHOLDER);
    }

    /// A secret nested inside an object, or inside an array of objects, is still
    /// a secret.
    #[test]
    fn nested_json_secrets_are_reached() {
        let b = body(
            r#"{"user":{"name":"kh","api_token":"sk-1"},"clients":[{"client_secret":"sk-2"}]}"#,
        );
        assert!(!b.contains("sk-1"), "{b}");
        assert!(!b.contains("sk-2"), "{b}");
        let parsed: serde_json::Value = serde_json::from_str(&b).unwrap();
        assert_eq!(parsed["user"]["name"], "kh");
    }

    /// A whole object under a credential-shaped key goes, not just its strings.
    #[test]
    fn an_object_under_a_secret_key_is_replaced_wholesale() {
        let b = body(r#"{"credentials":{"user":"kh","pass":"hunter2"}}"#);
        assert!(!b.contains("hunter2"), "{b}");
        assert!(!b.contains("\"user\""), "the whole object should go: {b}");
    }

    #[test]
    fn a_clean_json_body_keeps_its_original_formatting() {
        let original = r#"{"email":"me@example.com","page":2}"#;
        assert_eq!(
            body(original),
            original,
            "an untouched body should not be reformatted"
        );
    }

    /// A body Grove cannot parse is left intact rather than blanked. This is the
    /// documented limit of the feature, so it is pinned as behaviour.
    #[test]
    fn an_unparseable_body_is_left_alone() {
        for raw in [
            "\u{1}\u{2}\u{3}binary junk",
            "------WebKitFormBoundary\r\nContent-Disposition: form-data\r\n",
            "just some prose with a password in it",
            "{\"truncated\": \"val",
        ] {
            assert_eq!(body(raw), raw, "should not have been altered: {raw:?}");
        }
    }

    #[test]
    fn an_empty_body_is_not_touched() {
        assert_eq!(body(""), "");
        assert_eq!(body("   "), "   ");
    }
}
