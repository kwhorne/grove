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

pub fn generate(d: &RequestDetail, fmt: TestFormat) -> String {
    match fmt {
        TestFormat::Curl => curl(d),
        TestFormat::Http => http_file(d),
        TestFormat::Pest => pest(d),
    }
}

fn curl(d: &RequestDetail) -> String {
    let mut out = format!("curl -X {} '{}'", d.method, url(d));
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
    let mut out = format!("{} {}\n", d.method, url(d));
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
    if helper == "json" {
        return format!(
            "<?php\n\nit('{title} responds', function () {{\n    $response = $this->json('{}', '{path}'{body_arg});\n\n    $response->assertStatus({});\n}});\n",
            d.method, d.status
        );
    }
    format!(
        "<?php\n\nit('{title} responds', function () {{\n    $response = $this->{helper}('{path}'{body_arg});\n\n    $response->assertStatus({});\n}});\n",
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
