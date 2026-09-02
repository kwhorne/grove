//! Human + `--json` output formatting for CLI responses.

use grove_ipc::protocol::{DiagnosticStatus, Response, ResponseData};
use grove_runtime::{PhpBuild, PhpRegistry, Tier};

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
                match &svc.detail {
                    Some(detail) => println!("  {dot} {:<12} {detail}", svc.name),
                    None => println!("  {dot} {}", svc.name),
                }
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
        Some(ResponseData::RequestChain(_)) => {}  // surfaced via --json / MCP
        Some(ResponseData::WindowChain(_)) => {}   // folded into sandbox tool results
        Some(ResponseData::Explain(None)) => println!("no request with that id"),
        Some(ResponseData::Explain(Some(b))) => {
            println!("{}", b.summary);
            if !b.chain.queries.is_empty() {
                println!("\nQueries ({}):", b.chain.queries.len());
                for q in &b.chain.queries {
                    println!("  {}", q.sql);
                }
            }
            if !b.chain.emails.is_empty() {
                println!("\nMail ({}):", b.chain.emails.len());
                for m in &b.chain.emails {
                    println!("  ✉ {} → {}", m.subject, m.to.join(", "));
                }
            }
            if b.logs.is_empty() {
                if b.is_error {
                    println!("\nNo matching error-log entries found.");
                }
            } else {
                println!("\nError log:");
                for e in &b.logs {
                    println!("  [{}] {}: {}", e.datetime, e.level, e.message);
                    if let Some(ctx) = &e.context {
                        for line in ctx.lines().take(8) {
                            println!("    {line}");
                        }
                    }
                }
            }
            println!(
                "\nTip: `grove explain {} --json` for the full bundle (pipe to your AI).",
                b.request.id
            );
        }
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
                let audit = grove_runtime::audit_extensions(b);
                serde_json::json!({
                    "version": b.version,
                    "fpm_binary": b.fpm_binary,
                    "cli_binary": b.cli_binary,
                    "variant": b.variant,
                    "user_registered": b.user_registered,
                    "extensions": audit.loaded,
                    "missing_required": audit
                        .missing_at(Tier::Required)
                        .iter()
                        .map(|e| e.name)
                        .collect::<Vec<_>>(),
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
        let tag = if b.user_registered {
            " (custom)".to_string()
        } else {
            b.variant
                .as_deref()
                .map(|v| format!(" ({v})"))
                .unwrap_or_default()
        };
        println!("php@{}{tag}  →  {}", b.version, b.fpm_binary.display());
        println!("    {}", grove_runtime::audit_extensions(b).summary());
    }
    if !any {
        println!("No PHP builds registered. Run `grove php discover`.");
    }
}

/// Per-build extension audit — the detail behind `grove php list`'s one-liner.
///
/// The point of the report is the *why* column: "intl is missing" means nothing
/// until you know it's what Laravel's `Number` helpers and Filament need.
pub fn print_php_extensions(builds: &[PhpBuild], show_present: bool, json: bool) {
    if json {
        let out: Vec<_> = builds
            .iter()
            .map(|b| {
                let audit = grove_runtime::audit_extensions(b);
                let entries = |tier: Tier| {
                    audit
                        .missing_at(tier)
                        .iter()
                        .map(|e| serde_json::json!({ "name": e.name, "why": e.why }))
                        .collect::<Vec<_>>()
                };
                serde_json::json!({
                    "version": b.version,
                    "variant": b.variant,
                    "loaded": audit.loaded,
                    "healthy": audit.is_healthy(),
                    "missing": {
                        "required": entries(Tier::Required),
                        "recommended": entries(Tier::Recommended),
                        "optional": entries(Tier::Optional),
                    },
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
        return;
    }

    for (i, b) in builds.iter().enumerate() {
        if i > 0 {
            println!();
        }
        let audit = grove_runtime::audit_extensions(b);
        let variant = b
            .variant
            .as_deref()
            .map(|v| format!(" ({v})"))
            .unwrap_or_else(|| {
                if b.user_registered {
                    " (custom)".into()
                } else {
                    String::new()
                }
            });
        println!("php@{}{variant} — {}", b.version, audit.summary());

        if audit.loaded.is_empty() {
            println!("  Could not run `php -m` for this build.");
            continue;
        }

        // Optional gaps are a long tail (Swoole, MongoDB, tidy…) that would bury
        // the two or three lines actually worth acting on, so they wait for
        // `--all`.
        let tiers: &[Tier] = if show_present {
            &[Tier::Required, Tier::Recommended, Tier::Optional]
        } else {
            &[Tier::Required, Tier::Recommended]
        };
        for &tier in tiers {
            let missing = audit.missing_at(tier);
            if missing.is_empty() {
                continue;
            }
            println!("\n  Missing ({}):", tier.label());
            for e in &missing {
                println!("    ✗ {:<14} {}", e.name, e.why);
            }
        }

        if audit.missing.is_empty() {
            println!("  Every extension Grove looks for is present.");
        } else if !show_present {
            let optional = audit.missing_at(Tier::Optional).len();
            if optional > 0 {
                println!(
                    "\n  {optional} optional extension(s) also missing — `--all` to list them."
                );
            }
        }

        if show_present {
            println!("\n  Loaded ({}):", audit.loaded.len());
            for chunk in audit.loaded.chunks(6) {
                println!("    {}", chunk.join("  "));
            }
        }
    }

    // Close with the way out, phrased for the variant they're actually on: the
    // two prebuilt sets trade one gap for the other, so telling someone already
    // on `bulk` to switch to `bulk` is worse than saying nothing.
    let unhealthy: Vec<&PhpBuild> = builds
        .iter()
        .filter(|b| !grove_runtime::audit_extensions(b).is_healthy())
        .collect();
    if unhealthy.is_empty() {
        return;
    }
    println!(
        "\nThe prebuilt static-PHP sets are not supersets of each other: `common` has the PDO\n\
         SQLite/PostgreSQL drivers but no intl or mysqli; `bulk` has intl and mysqli but not\n\
         those PDO drivers."
    );
    let variants: std::collections::BTreeSet<&str> = unhealthy
        .iter()
        .filter_map(|b| b.variant.as_deref())
        .collect();
    if let [only] = variants.iter().copied().collect::<Vec<_>>()[..] {
        let other = if only == "bulk" { "common" } else { "bulk" };
        println!(
            "Trade one gap for the other with `grove php install <version> --variant {other}`,"
        );
    } else {
        println!("Pick a set with `grove php install <version> --variant common|bulk`,");
    }
    println!(
        "or get everything by pointing Grove at your own PHP:\n  \
         grove php register <version> <path-to-php-fpm>"
    );
}
