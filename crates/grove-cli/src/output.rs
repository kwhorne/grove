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
        Some(ResponseData::PhpVersions(vers)) => {
            for v in vers {
                if v.installed {
                    println!("php@{}  installed", v.major);
                } else {
                    println!("php@{}  available", v.major);
                }
            }
        }
        Some(ResponseData::Nodes(nodes)) => {
            for n in nodes {
                if n.installed {
                    println!(
                        "node@{}  installed (v{})",
                        n.major,
                        n.version.as_deref().unwrap_or("?")
                    );
                } else {
                    println!("node@{}  available", n.major);
                }
            }
        }
        Some(ResponseData::LogSources(sources)) => {
            if sources.is_empty() {
                println!("No log files found yet.");
                return;
            }
            for s in sources {
                println!("{:<10} {}", s.kind, s.name);
            }
        }
        Some(ResponseData::LogEntries(entries)) => {
            for e in entries.iter().rev() {
                let date = if e.datetime.is_empty() {
                    "-"
                } else {
                    e.datetime.as_str()
                };
                println!("{:<8} {:<20} {}", e.level, date, truncate(&e.message, 90));
            }
        }
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
        Some(ResponseData::Tunnels(tunnels)) => {
            for t in tunnels {
                println!("✓ {} → {}", t.site, t.public_url);
            }
        }
        Some(ResponseData::TunnelRequests(reqs)) => {
            for r in reqs {
                println!(
                    "  {:<6} {:<40} {} ({}ms)",
                    r.method, r.path, r.status, r.duration_ms
                );
            }
        }
        Some(ResponseData::DevSites(sites)) => {
            if sites.is_empty() {
                println!("no dev processes running");
            } else {
                for s in sites {
                    println!("● dev running: {s}");
                }
            }
        }
        Some(ResponseData::License(license)) => match license {
            None => println!("No license active — Grove is running the free, open-source edition."),
            Some(c) => {
                let product = if c.is_teams() {
                    "Grove Teams"
                } else {
                    "Grove Pro"
                };
                println!("✓ {product} active");
                println!("  seats  : {}", c.seats);
                println!("  email  : {}", c.email);
                let days = (c.exp
                    - std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0))
                    / 86_400;
                println!("  renews : in {days} days");
            }
        },
        Some(ResponseData::Requests(reqs)) => {
            if reqs.is_empty() {
                println!("no requests recorded yet — open a site and reload");
            } else {
                for r in reqs {
                    println!(
                        "#{:<5} {}  {:>3}  {:<6} {:>5}ms  {:<16} {}",
                        r.id,
                        r.time,
                        r.status,
                        r.method,
                        r.duration_ms,
                        truncate(&r.site, 16),
                        r.path
                    );
                }
                println!("\nreplay any of these with: grove replay <id>");
            }
        }
        Some(ResponseData::RequestDetail(_)) => {} // GUI-only detail view
        Some(ResponseData::RequestChain(_)) => {} // surfaced via --json / MCP
        Some(ResponseData::SqlCapture(s)) => {
            println!(
                "SQL capture: {}\n{}",
                if s.enabled { "on" } else { "off" },
                s.note
            );
        }
        Some(ResponseData::Generated(code)) => print!("{code}"),
        Some(ResponseData::Hooks(hooks)) => {
            if hooks.is_empty() {
                println!("no webhooks captured yet — point a provider at https://<site>.test/__grove/hooks/<bucket>");
            } else {
                for r in hooks {
                    println!(
                        "#{:<5} {}  {:<6} {:<12} {}",
                        r.id,
                        r.time,
                        r.method,
                        truncate(&r.site, 12),
                        r.path
                    );
                }
                println!("\nre-deliver one with: grove hooks replay <id> --to https://<site>.test/<handler>");
            }
        }
        Some(ResponseData::Replayed {
            status,
            duration_ms,
        }) => {
            println!("replayed → {status} in {duration_ms}ms (see it in `grove requests`)");
        }
        Some(ResponseData::Snapshots(snaps)) => {
            if snaps.is_empty() {
                println!("no database snapshots yet — take one with `grove db snapshot`");
            } else {
                for s in snaps {
                    let kb = s.bytes / 1024;
                    let note = if s.note.is_empty() {
                        String::new()
                    } else {
                        format!("  — {}", s.note)
                    };
                    println!(
                        "{}  {:<9} {:<20} {:>6} KB  {}{}",
                        s.id, s.engine, s.database, kb, s.created, note
                    );
                }
            }
        }
        Some(ResponseData::Xdebug(x)) => {
            println!(
                "Xdebug {} (DBGp port {})",
                if x.enabled { "enabled" } else { "disabled" },
                x.port
            );
            for b in &x.builds {
                println!("  php@{:<5} {}", b.version, b.availability);
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
