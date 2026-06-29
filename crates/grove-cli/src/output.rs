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
        Some(ResponseData::Mail(mails)) => {
            if mails.is_empty() {
                println!("No captured emails. Point your app's SMTP at 127.0.0.1:1025.");
                return;
            }
            println!("{:<5} {:<26} {:<26} SUBJECT", "ID", "FROM", "TO");
            for m in mails {
                println!(
                    "{:<5} {:<26} {:<26} {}",
                    m.id,
                    truncate(&m.from, 25),
                    truncate(&m.to.join(","), 25),
                    m.subject
                );
            }
        }
        Some(ResponseData::MailMessage(msg)) => match msg {
            None => eprintln!("✗ no such email"),
            Some(m) => {
                println!("From:    {}", m.from);
                println!("To:      {}", m.to.join(", "));
                println!("Subject: {}", m.subject);
                println!("Date:    {}", m.received_at);
                println!("Size:    {} bytes", m.size);
                let body = m
                    .text
                    .clone()
                    .or_else(|| m.html.clone())
                    .unwrap_or_else(|| m.raw.clone());
                println!("\n{body}");
            }
        },
        Some(ResponseData::Settings(_)) => println!("✓ ok"),
        Some(ResponseData::Services(svcs)) => {
            println!(
                "{:<12} {:<14} {:<10} {:<9} PORT",
                "SERVICE", "CATEGORY", "INSTALLED", "RUNNING"
            );
            for s in svcs {
                println!(
                    "{:<12} {:<14} {:<10} {:<9} {}",
                    s.name,
                    s.category,
                    if s.installed { "yes" } else { "no" },
                    if s.running { "yes" } else { "no" },
                    s.port
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

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
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
        println!(
            "{}",
            serde_json::to_string_pretty(&builds).unwrap_or_default()
        );
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
