//! Turn a captured request into a ready-to-run artifact: a `curl` command, a
//! `.http` request file, or a Laravel Pest feature test. Because Grove already
//! captured the exact method, headers and body, "make a test from this request"
//! (or "replay it as curl") becomes a one-liner — great for turning a failing
//! request into a regression test.

use crate::reqlog::RequestDetail;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestFormat {
    Curl,
    Http,
    Pest,
}

impl TestFormat {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "curl" => Some(Self::Curl),
            "http" | "httpfile" | "rest" => Some(Self::Http),
            "pest" | "php" | "test" => Some(Self::Pest),
            _ => None,
        }
    }
}

/// Headers that don't belong in a generated artifact (recomputed by the client,
/// or noise).
fn skip_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "host" | "content-length" | "connection" | "transfer-encoding" | "accept-encoding"
    )
}

fn url(d: &RequestDetail) -> String {
    let scheme = if d.https { "https" } else { "http" };
    format!("{scheme}://{}{}", d.host, d.path)
}

/// Why a generated artifact may not reproduce the original request.
///
/// The timeline stores only the first [`crate::reqlog::MAX_BODY`] bytes of a
/// request body. Without saying so, an export of a large upload looks complete
/// and silently sends a partial body — which is worse than refusing to generate
/// one, because the failure surfaces as a puzzling response from the app.
fn truncation_lines() -> [String; 2] {
    let mib = crate::reqlog::MAX_BODY / (1024 * 1024);
    [
        format!("WARNING: Grove captured only the first {mib} MiB of this request body,"),
        "so the body below is incomplete and this will not reproduce the original request.".into(),
    ]
}

/// Prefix `out` with the truncation warning, one line per `comment` marker.
fn note_if_truncated(d: &RequestDetail, comment: &str) -> String {
    if !d.body_truncated {
        return String::new();
    }
    truncation_lines()
        .iter()
        .map(|line| format!("{comment} {line}\n"))
        .collect()
}

pub fn generate(d: &RequestDetail, fmt: TestFormat) -> String {
    match fmt {
        TestFormat::Curl => curl(d),
        TestFormat::Http => http_file(d),
        TestFormat::Pest => pest(d),
    }
}

fn curl(d: &RequestDetail) -> String {
    let mut out = note_if_truncated(d, "#");
    out.push_str(&format!("curl -X {} '{}'", d.method, url(d)));
    for (k, v) in &d.headers {
        if skip_header(k) {
            continue;
        }
        out.push_str(&format!(" \\\n  -H '{}: {}'", k, v.replace('\'', "'\\''")));
    }
    if !d.body.is_empty() {
        out.push_str(&format!(
            " \\\n  --data '{}'",
            d.body.replace('\'', "'\\''")
        ));
    }
    out.push('\n');
    out
}

fn http_file(d: &RequestDetail) -> String {
    let mut out = note_if_truncated(d, "#");
    out.push_str(&format!("{} {}\n", d.method, url(d)));
    for (k, v) in &d.headers {
        if skip_header(k) {
            continue;
        }
        out.push_str(&format!("{k}: {v}\n"));
    }
    if !d.body.is_empty() {
        out.push('\n');
        out.push_str(&d.body);
        out.push('\n');
    }
    out
}

fn pest(d: &RequestDetail) -> String {
    // path only (Laravel test helpers take a path, not a full URL).
    let path = &d.path;
    let method = d.method.to_ascii_lowercase();
    let is_json = d.headers.iter().any(|(k, v)| {
        k.eq_ignore_ascii_case("content-type") && v.to_ascii_lowercase().contains("json")
    });

    let helper = match method.as_str() {
        "get" => "getJson",
        "post" => "postJson",
        "put" => "putJson",
        "patch" => "patchJson",
        "delete" => "deleteJson",
        _ => "json",
    };

    let body_arg = if d.body.is_empty() || method == "get" {
        String::new()
    } else if d.body_truncated {
        // Don't try to parse a prefix: a truncated JSON body always fails to
        // parse, and reporting that as "not valid JSON" would blame the app.
        format!(
            ", [\n        // truncated body ({} bytes captured):\n        // {}\n    ]",
            d.body.len(),
            d.body
                .chars()
                .take(200)
                .collect::<String>()
                .replace('\n', " ")
        )
    } else if is_json {
        match serde_json::from_str::<serde_json::Value>(&d.body) {
            Ok(v) => format!(", {}", json_to_php(&v, 1)),
            Err(_) => format!(
                ", [\n        // body was not valid JSON:\n        // {}\n    ]",
                d.body.replace('\n', " ")
            ),
        }
    } else {
        format!(
            ", [\n        // raw body:\n        // {}\n    ]",
            d.body.replace('\n', " ")
        )
    };

    let title = format!("{} {}", d.method, path);
    let note = note_if_truncated(d, "//");
    if helper == "json" {
        return format!(
            "<?php\n\n{note}it('{title} responds', function () {{\n    $response = $this->json('{}', '{path}'{body_arg});\n\n    $response->assertStatus({});\n}});\n",
            d.method, d.status
        );
    }
    format!(
        "<?php\n\n{note}it('{title} responds', function () {{\n    $response = $this->{helper}('{path}'{body_arg});\n\n    $response->assertStatus({});\n}});\n",
        d.status
    )
}

/// Render a JSON value as a PHP array literal (indented).
fn json_to_php(v: &serde_json::Value, depth: usize) -> String {
    use serde_json::Value;
    let pad = "    ".repeat(depth);
    let pad_close = "    ".repeat(depth.saturating_sub(1));
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'")),
        Value::Array(items) => {
            if items.is_empty() {
                return "[]".into();
            }
            let mut out = String::from("[\n");
            for it in items {
                out.push_str(&format!("{pad}{},\n", json_to_php(it, depth + 1)));
            }
            out.push_str(&format!("{pad_close}]"));
            out
        }
        Value::Object(map) => {
            if map.is_empty() {
                return "[]".into();
            }
            let mut out = String::from("[\n");
            for (k, val) in map {
                out.push_str(&format!(
                    "{pad}'{}' => {},\n",
                    k.replace('\'', "\\'"),
                    json_to_php(val, depth + 1)
                ));
            }
            out.push_str(&format!("{pad_close}]"));
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detail() -> RequestDetail {
        RequestDetail {
            id: 1,
            method: "POST".into(),
            host: "myapp.test".into(),
            path: "/api/orders".into(),
            https: true,
            status: 201,
            headers: vec![
                ("content-type".into(), "application/json".into()),
                ("host".into(), "myapp.test".into()),
            ],
            body: r#"{"sku":"A1","qty":2}"#.into(),
            body_truncated: false,
        }
    }

    /// A truncated body must be flagged in every format. Silently emitting a
    /// partial body produces an artifact that looks right and reproduces nothing,
    /// and the failure then looks like the app's fault.
    #[test]
    fn every_format_flags_a_truncated_body() {
        let mut d = detail();
        d.body_truncated = true;

        let c = curl(&d);
        assert!(c.starts_with("# WARNING:"), "curl warns first: {c}");
        assert!(c.contains("will not reproduce"));

        let h = http_file(&d);
        assert!(h.starts_with("# WARNING:"), "http file warns first: {h}");

        let p = pest(&d);
        assert!(p.contains("// WARNING:"), "pest warns: {p}");
        // The warning must sit inside the PHP file, not before the opening tag.
        assert!(p.starts_with("<?php"));
        assert!(
            p.find("// WARNING:") < p.find("it('"),
            "warning belongs above the test: {p}"
        );
    }

    /// Nothing changes for a body captured in full.
    #[test]
    fn complete_bodies_carry_no_warning() {
        let d = detail();
        assert!(!curl(&d).contains("WARNING"));
        assert!(!http_file(&d).contains("WARNING"));
        assert!(!pest(&d).contains("WARNING"));
    }

    /// A truncated JSON body always fails to parse. Reporting that as invalid
    /// JSON would blame the application for Grove's own capture limit.
    #[test]
    fn truncated_json_is_not_called_invalid() {
        let mut d = detail();
        d.body = r#"{"sku":"A1","qty":"#.into(); // cut mid-value
        d.body_truncated = true;

        let p = pest(&d);
        assert!(p.contains("truncated body"), "{p}");
        assert!(!p.contains("not valid JSON"), "must not blame the app: {p}");
    }

    #[test]
    fn curl_has_method_url_and_body() {
        let c = curl(&detail());
        assert!(c.contains("curl -X POST 'https://myapp.test/api/orders'"));
        assert!(c.contains("-H 'content-type: application/json'"));
        assert!(!c.contains("-H 'host:")); // host is skipped
        assert!(c.contains("--data"));
    }

    #[test]
    fn pest_maps_json_body_to_php_array() {
        let p = pest(&detail());
        assert!(p.contains("postJson('/api/orders'"));
        assert!(p.contains("'sku' => 'A1'"));
        assert!(p.contains("'qty' => 2"));
        assert!(p.contains("assertStatus(201)"));
    }

    #[test]
    fn http_file_format() {
        let h = http_file(&detail());
        assert!(h.starts_with("POST https://myapp.test/api/orders"));
        assert!(h.contains("content-type: application/json"));
    }
}
