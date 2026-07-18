//! Parsing the bundled databases' query logs into correlatable events.
//!
//! Grove owns the database service, so it can turn on MySQL's general query log
//! (writing to a Grove-owned file) and read it back to correlate the SQL a
//! request issued with the request timeline — no per-app instrumentation. The
//! parser here is the tested core; enabling the log and reading the file lives
//! in the daemon.

use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// One SQL statement observed in a database's query log.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QueryEvent {
    /// Epoch milliseconds the statement was logged.
    pub epoch_ms: u128,
    /// `mysql` today; other engines plug into the same shape.
    pub engine: String,
    /// The SQL text.
    pub sql: String,
}

/// Parse MySQL 8's general query log (FILE output) into query events.
///
/// Each entry starts with an ISO-8601 timestamp, a tab, `<id> <Command>`, a tab,
/// then the argument. Lines without a leading timestamp are continuations of the
/// previous entry's argument (multi-line SQL). Only `Query` commands are kept,
/// and Grove's own bookkeeping (toggling the log) is filtered out.
pub fn parse_mysql_general(text: &str) -> Vec<QueryEvent> {
    let mut events = Vec::new();
    // (epoch_ms, command, argument)
    let mut cur: Option<(u128, String, String)> = None;

    for line in text.lines() {
        if let Some((ms, rest)) = split_timestamp(line) {
            flush(&mut events, cur.take());
            let (meta, arg) = match rest.split_once('\t') {
                Some((m, a)) => (m, a.to_string()),
                None => (rest, String::new()),
            };
            let command = meta.split_whitespace().nth(1).unwrap_or("").to_string();
            cur = Some((ms, command, arg));
        } else if let Some((_, _, arg)) = cur.as_mut() {
            arg.push('\n');
            arg.push_str(line);
        }
        // Lines before the first entry (the log header) are ignored.
    }
    flush(&mut events, cur.take());
    events
}

/// Keep only the events whose timestamp falls within `[start_ms, end_ms]`
/// (inclusive) — used to attribute captured SQL to a request or sandboxed op.
pub fn in_window(events: Vec<QueryEvent>, start_ms: u128, end_ms: u128) -> Vec<QueryEvent> {
    events
        .into_iter()
        .filter(|q| q.epoch_ms >= start_ms && q.epoch_ms <= end_ms)
        .collect()
}

/// If `line` begins with an ISO-8601 timestamp followed by a tab, return the
/// epoch-ms and the remainder after the tab.
fn split_timestamp(line: &str) -> Option<(u128, &str)> {
    let (head, rest) = line.split_once('\t')?;
    let head = head.trim();
    // Cheap gate before the (relatively costly) full parse.
    if head.len() < 19 || !head.as_bytes()[0].is_ascii_digit() {
        return None;
    }
    let ts = OffsetDateTime::parse(head, &Rfc3339).ok()?;
    let ms = (ts.unix_timestamp_nanos() / 1_000_000).max(0) as u128;
    Some((ms, rest))
}

fn flush(events: &mut Vec<QueryEvent>, cur: Option<(u128, String, String)>) {
    let Some((epoch_ms, command, arg)) = cur else {
        return;
    };
    if command != "Query" {
        return;
    }
    let sql = arg.trim().to_string();
    if sql.is_empty() || is_bookkeeping(&sql) {
        return;
    }
    events.push(QueryEvent {
        epoch_ms,
        engine: "mysql".into(),
        sql,
    });
}

/// Grove's own log-toggling statements should never show up as app queries.
fn is_bookkeeping(sql: &str) -> bool {
    let l = sql.to_ascii_lowercase();
    l.contains("general_log") || l.contains("log_output")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_line_queries_with_timestamps() {
        let log = "\
/usr/sbin/mysqld, Version: 8.0.36
Tcp port: 3306  Unix socket: /tmp/mysql.sock
Time                 Id Command    Argument
2026-07-18T10:11:12.100000Z\t   12 Connect\troot@localhost on app
2026-07-18T10:11:12.200000Z\t   12 Query\tSELECT * FROM users WHERE id = 1
2026-07-18T10:11:12.300000Z\t   12 Query\tUPDATE users SET name = 'x' WHERE id = 1
";
        let events = parse_mysql_general(log);
        assert_eq!(events.len(), 2); // Connect is dropped
        assert_eq!(events[0].sql, "SELECT * FROM users WHERE id = 1");
        assert_eq!(events[0].engine, "mysql");
        assert_eq!(events[1].sql, "UPDATE users SET name = 'x' WHERE id = 1");
        // 2026-07-18T10:11:12.200Z
        assert_eq!(events[0].epoch_ms % 1000, 200);
    }

    #[test]
    fn joins_multi_line_statements() {
        let log = "\
2026-07-18T10:11:12.200000Z\t   12 Query\tSELECT id,
name
FROM users
2026-07-18T10:11:12.300000Z\t   12 Query\tSELECT 2
";
        let events = parse_mysql_general(log);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].sql, "SELECT id,\nname\nFROM users");
        assert_eq!(events[1].sql, "SELECT 2");
    }

    #[test]
    fn filters_grove_bookkeeping() {
        let log = "\
2026-07-18T10:11:12.200000Z\t   12 Query\tSET GLOBAL general_log = 1
2026-07-18T10:11:12.300000Z\t   12 Query\tSET GLOBAL log_output = 'FILE'
2026-07-18T10:11:12.400000Z\t   12 Query\tSELECT 1
";
        let events = parse_mysql_general(log);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].sql, "SELECT 1");
    }

    #[test]
    fn in_window_filters_inclusively() {
        let ev = |ms: u128| QueryEvent {
            epoch_ms: ms,
            engine: "mysql".into(),
            sql: format!("SELECT {ms}"),
        };
        let events = vec![ev(100), ev(150), ev(200), ev(500)];
        let hit = in_window(events, 150, 200);
        assert_eq!(hit.len(), 2);
        assert_eq!(hit[0].sql, "SELECT 150");
        assert_eq!(hit[1].sql, "SELECT 200");
    }

    #[test]
    fn empty_or_header_only_yields_nothing() {
        assert!(parse_mysql_general("").is_empty());
        assert!(parse_mysql_general("Time                 Id Command    Argument\n").is_empty());
    }
}
