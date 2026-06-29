//! Human + `--json` output formatting for CLI responses.

use grove_ipc::protocol::{DiagnosticStatus, Response, ResponseData};
use grove_runtime::PhpRegistry;

pub fn print_response(resp: &Response, json: bool) {
    if json {
        println!("{}", serde_json::to_string_pretty(resp).unwrap_or_default());
        return;
    }

    if !resp.ok {
        eprintln!("✗ {}", resp.error.as_deref().unwrap_or("unknown error"));
        return;
    }

    match &resp.data {
        None => println!("✓ ok"),
        Some(ResponseData::Pong { version }) => println!("✓ groved {version}"),
        Some(ResponseData::Message(m)) => println!("✓ {m}"),
        Some(ResponseData::Status(s)) => {
            println!("Grove {ver}", ver = s.version);
            println!("  TLD          .{}", s.tld);
            println!("  HTTP         :{}", s.http_port);
            println!("  HTTPS        :{}", s.https_port);
            println!("  DNS          :{}", s.dns_port);
            println!("  Sites        {}", s.site_count);
            for svc in &s.services {
                let dot = if svc.running { "●" } else { "○" };
                println!("  {dot} {}", svc.name);
            }
        }
        Some(ResponseData::Sites(sites)) => {
            if sites.is_empty() {
                println!("No sites yet. Try `grove park ~/Code` or `grove link`.");
                return;
            }
            println!(
                "{:<24} {:<10} {:<7} {:<6} URL",
                "SITE", "DRIVER", "PHP", "HTTPS"
            );
            for s in sites {
                let site = &s.site;
                println!(
                    "{:<24} {:<10} {:<7} {:<6} {}",
                    site.hostname,
                    site.driver.as_str(),
                    site.php,
                    if site.secure { "yes" } else { "no" },
                    site.url(),
                );
            }
        }
        Some(ResponseData::Doctor(entries)) => {
            for e in entries {
                let mark = match e.status {
                    DiagnosticStatus::Pass => "✓",
                    DiagnosticStatus::Warn => "!",
                    DiagnosticStatus::Fail => "✗",
                };
                println!("{mark} {:<14} {}", e.check, e.detail);
            }
        }
    }
}

pub fn print_message(msg: &str, json: bool) {
    if json {
        println!("{}", serde_json::json!({ "ok": true, "message": msg }));
    } else {
        println!("✓ {msg}");
    }
}

pub fn print_php_list(registry: &PhpRegistry, json: bool) {
    if json {
        let builds: Vec<_> = registry
            .iter()
            .map(|b| {
                serde_json::json!({
                    "version": b.version,
                    "fpm_binary": b.fpm_binary,
                    "user_registered": b.user_registered,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&builds).unwrap_or_default());
        return;
    }
    let mut any = false;
    for b in registry.iter() {
        any = true;
        let tag = if b.user_registered { " (custom)" } else { "" };
        println!("php@{}{tag}  →  {}", b.version, b.fpm_binary.display());
    }
    if !any {
        println!("No PHP builds registered. Run `grove php discover`.");
    }
}
