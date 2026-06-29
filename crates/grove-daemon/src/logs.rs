//! Log discovery + parsing for the GUI/CLI log viewer.
//!
//! Two kinds of sources are surfaced:
//!   * **laravel** — each site's `storage/logs/*.log`, parsed into level/date/
//!     message/context entries.
//!   * **service** — Grove's own service logs (php-fpm, postgres, mysql, redis,
//!     daemon), shown line by line.

use std::path::Path;

use grove_core::paths::GrovePaths;
use grove_core::{Config, SiteRegistry};
use grove_ipc::protocol::{LogEntry, LogSource};

/// Discover every readable log file Grove knows about.
pub fn discover(config: &Config, registry: &SiteRegistry, paths: &GrovePaths) -> Vec<LogSource> {
    let mut sources = Vec::new();

    // Per-site Laravel logs.
    for site in registry.iter() {
        let logs_dir = site.path.join("storage/logs");
        let Ok(entries) = std::fs::read_dir(&logs_dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("log") {
                let file = p.file_name().and_then(|n| n.to_str()).unwrap_or("log");
                sources.push(LogSource {
                    name: format!("{} · {}", site.name, file),
                    path: p.to_string_lossy().into_owned(),
                    kind: "laravel".into(),
                });
            }
        }
    }

    // Grove's own service / runtime logs.
    if let Ok(entries) = std::fs::read_dir(paths.logs_dir()) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("log") {
                let file = p.file_name().and_then(|n| n.to_str()).unwrap_or("log");
                sources.push(LogSource {
                    name: format!("grove · {file}"),
                    path: p.to_string_lossy().into_owned(),
                    kind: "service".into(),
                });
            }
        }
    }

    let _ = config;
    sources.sort_by(|a, b| a.name.cmp(&b.name));
    sources
}

/// Read up to `limit` most-recent entries from a log file. `kind` selects the
/// parser. Returns newest-first.
pub fn read_entries(path: &Path, kind: &str, limit: usize) -> std::io::Result<Vec<LogEntry>> {
    let raw = std::fs::read_to_string(path)?;
    let mut entries = if kind == "laravel" {
        parse_laravel(&raw)
    } else {
        parse_plain(&raw)
    };
    entries.reverse(); // newest first
    entries.truncate(limit);
    Ok(entries)
}

/// Parse the Laravel/Monolog default format:
/// `[2026-06-29 05:27:49] local.ERROR: message {context}` plus following
/// stacktrace lines, which are folded into the entry's `context`.
fn parse_laravel(raw: &str) -> Vec<LogEntry> {
    let mut entries: Vec<LogEntry> = Vec::new();
    for line in raw.lines() {
        if let Some(parsed) = parse_laravel_header(line) {
            entries.push(parsed);
        } else if let Some(last) = entries.last_mut() {
            // Continuation (stacktrace / wrapped context).
            let ctx = last.context.get_or_insert_with(String::new);
            ctx.push_str(line);
            ctx.push('\n');
        }
    }
    entries
}

fn parse_laravel_header(line: &str) -> Option<LogEntry> {
    let rest = line.strip_prefix('[')?;
    let close = rest.find(']')?;
    let datetime = rest[..close].to_string();
    // Date must look like a timestamp.
    if !datetime.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let after = rest[close + 1..].trim_start();

    // `channel.LEVEL: message`
    let (level, message) = match after.find(": ") {
        Some(colon) => {
            let prefix = &after[..colon];
            let level = prefix.rsplit('.').next().unwrap_or(prefix).to_uppercase();
            (level, after[colon + 2..].to_string())
        }
        None => ("INFO".to_string(), after.to_string()),
    };

    Some(LogEntry {
        level,
        datetime,
        message,
        context: None,
    })
}

/// Plain log parser: one entry per line, with a best-effort level guess.
fn parse_plain(raw: &str) -> Vec<LogEntry> {
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| LogEntry {
            level: guess_level(line),
            datetime: String::new(),
            message: line.to_string(),
            context: None,
        })
        .collect()
}

fn guess_level(line: &str) -> String {
    let l = line.to_uppercase();
    if l.contains("ERROR") || l.contains("FATAL") {
        "ERROR".into()
    } else if l.contains("WARN") {
        "WARNING".into()
    } else if l.contains("NOTICE") {
        "NOTICE".into()
    } else {
        "INFO".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_laravel_line_and_stacktrace() {
        let raw = "[2026-06-29 05:27:49] local.ERROR: syntax error {\"exception\":\"x\"}\n#0 /app/foo.php\n#1 /app/bar.php\n[2026-06-29 05:28:00] local.INFO: done\n";
        let mut entries = parse_laravel(raw);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].level, "ERROR");
        assert_eq!(entries[0].datetime, "2026-06-29 05:27:49");
        assert!(entries[0].message.starts_with("syntax error"));
        assert!(entries[0]
            .context
            .as_ref()
            .unwrap()
            .contains("#0 /app/foo.php"));
        // newest-first happens in read_entries; here order is file order
        entries.reverse();
        assert_eq!(entries[0].level, "INFO");
    }

    #[test]
    fn plain_guesses_level() {
        let entries = parse_plain("WARNING: low disk\nall good\n");
        assert_eq!(entries[0].level, "WARNING");
        assert_eq!(entries[1].level, "INFO");
    }
}
