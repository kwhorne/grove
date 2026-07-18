//! A minimal SMTP server that captures mail instead of delivering it.
//!
//! It implements just enough of RFC 5321 to accept what application mailers
//! send (EHLO/HELO, MAIL FROM, RCPT TO, DATA, RSET, NOOP, QUIT). Every accepted
//! message is parsed and pushed into the [`MailStore`]; nothing is relayed.

use std::net::SocketAddr;

use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use crate::store::{CapturedEmail, MailStore};

#[derive(Debug, thiserror::Error)]
pub enum MailError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Bind and serve the SMTP catcher on `addr` until the task is dropped.
pub async fn serve_smtp(addr: SocketAddr, store: MailStore) -> Result<(), MailError> {
    let listener = TcpListener::bind(addr).await?;
    tracing::info!(%addr, "mail-catcher (SMTP) listening");

    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, "SMTP accept failed");
                continue;
            }
        };
        let store = store.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(stream, store).await {
                tracing::debug!(error = %e, %peer, "SMTP connection ended");
            }
        });
    }
}

#[derive(Default)]
struct Session {
    from: String,
    to: Vec<String>,
}

async fn handle(stream: TcpStream, store: MailStore) -> Result<(), MailError> {
    let (read_half, mut write) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    let mut session = Session::default();

    write.write_all(b"220 Grove mail-catcher ready\r\n").await?;

    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            break; // client disconnected
        }
        let trimmed = line.trim_end();
        let upper = trimmed.to_ascii_uppercase();

        if upper.starts_with("EHLO") || upper.starts_with("HELO") {
            // Advertise no extensions; a single 250 keeps clients happy.
            write.write_all(b"250 Grove\r\n").await?;
        } else if upper.starts_with("MAIL FROM") {
            session.from = extract_addr(trimmed);
            write.write_all(b"250 2.1.0 OK\r\n").await?;
        } else if upper.starts_with("RCPT TO") {
            session.to.push(extract_addr(trimmed));
            write.write_all(b"250 2.1.5 OK\r\n").await?;
        } else if upper.starts_with("DATA") {
            write
                .write_all(b"354 End data with <CR><LF>.<CR><LF>\r\n")
                .await?;
            let body = read_data(&mut reader).await?;
            let email = parse_email(&session, &body);
            let id = store.push(email);
            tracing::info!(id, from = %session.from, "captured email");
            write
                .write_all(format!("250 2.0.0 Ok: queued as {id}\r\n").as_bytes())
                .await?;
            session = Session::default();
        } else if upper.starts_with("RSET") {
            session = Session::default();
            write.write_all(b"250 2.0.0 OK\r\n").await?;
        } else if upper.starts_with("NOOP") {
            write.write_all(b"250 2.0.0 OK\r\n").await?;
        } else if upper.starts_with("QUIT") {
            write.write_all(b"221 2.0.0 Bye\r\n").await?;
            break;
        } else {
            write.write_all(b"250 2.0.0 OK\r\n").await?; // be lenient
        }
    }
    Ok(())
}

/// Read the DATA payload until the terminating `.` line, undoing dot-stuffing.
async fn read_data<R>(reader: &mut R) -> Result<String, MailError>
where
    R: AsyncBufReadExt + Unpin,
{
    let mut data = String::new();
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            break;
        }
        let content = line.trim_end_matches(['\r', '\n']);
        if content == "." {
            break;
        }
        // Dot-stuffing: a leading ".." represents a literal ".".
        let unstuffed = content.strip_prefix('.').unwrap_or(content);
        data.push_str(unstuffed);
        data.push('\n');
    }
    Ok(data)
}

/// Pull the `<addr>` out of `MAIL FROM:<addr>` / `RCPT TO:<addr>`.
fn extract_addr(line: &str) -> String {
    if let (Some(start), Some(end)) = (line.find('<'), line.rfind('>')) {
        if start < end {
            return line[start + 1..end].to_string();
        }
    }
    // Fallback: take whatever follows the colon.
    line.split_once(':')
        .map(|(_, rest)| rest.trim().to_string())
        .unwrap_or_default()
}

/// Parse captured DATA into headers + body parts.
fn parse_email(session: &Session, data: &str) -> CapturedEmail {
    let (headers, body) = split_headers(data);

    let header = |name: &str| {
        headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    };

    let subject = header("Subject").unwrap_or_default();
    let from = header("From").unwrap_or_else(|| session.from.clone());
    let to: Vec<String> = header("To")
        .map(|v| v.split(',').map(|s| s.trim().to_string()).collect())
        .unwrap_or_else(|| session.to.clone());

    let content_type = header("Content-Type").unwrap_or_default();
    let (text, html) = extract_bodies(&content_type, body);

    let now = OffsetDateTime::now_utc();
    let received_at = now.format(&Rfc3339).unwrap_or_default();
    let received_ms = (now.unix_timestamp_nanos() / 1_000_000).max(0) as u128;

    CapturedEmail {
        id: 0,
        from,
        to,
        subject,
        received_at,
        received_ms,
        size: data.len(),
        raw: data.to_string(),
        text,
        html,
    }
}

/// Split a raw message into folded headers and the remaining body.
fn split_headers(data: &str) -> (Vec<(String, String)>, &str) {
    let sep = data.find("\n\n");
    let (head, body) = match sep {
        Some(i) => (&data[..i], &data[i + 2..]),
        None => (data, ""),
    };

    let mut headers: Vec<(String, String)> = Vec::new();
    for raw_line in head.lines() {
        if raw_line.starts_with(' ') || raw_line.starts_with('\t') {
            // Continuation of the previous header (folding).
            if let Some(last) = headers.last_mut() {
                last.1.push(' ');
                last.1.push_str(raw_line.trim());
            }
            continue;
        }
        if let Some((k, v)) = raw_line.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    (headers, body)
}

/// Best-effort extraction of text/plain and text/html bodies.
fn extract_bodies(content_type: &str, body: &str) -> (Option<String>, Option<String>) {
    let ct = content_type.to_ascii_lowercase();

    if ct.contains("multipart/") {
        if let Some(boundary) = boundary_of(content_type) {
            return split_multipart(&boundary, body);
        }
    }
    if ct.contains("text/html") {
        return (None, Some(body.to_string()));
    }
    // Default: treat as plain text.
    if body.trim().is_empty() {
        (None, None)
    } else {
        (Some(body.to_string()), None)
    }
}

fn boundary_of(content_type: &str) -> Option<String> {
    let lower = content_type.to_ascii_lowercase();
    let idx = lower.find("boundary=")?;
    let rest = content_type[idx + "boundary=".len()..].trim();
    let rest = rest.trim_start_matches('"');
    let end = rest.find(['"', ';']).unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

fn split_multipart(boundary: &str, body: &str) -> (Option<String>, Option<String>) {
    let delim = format!("--{boundary}");
    let mut text = None;
    let mut html = None;
    for part in body.split(&delim) {
        let part = part.trim_start_matches(['\r', '\n']);
        if part.is_empty() || part.starts_with("--") {
            continue;
        }
        let (headers, content) = split_headers(part);
        let ct = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("Content-Type"))
            .map(|(_, v)| v.to_ascii_lowercase())
            .unwrap_or_default();
        if ct.contains("text/html") && html.is_none() {
            html = Some(content.to_string());
        } else if ct.contains("text/plain") && text.is_none() {
            text = Some(content.to_string());
        }
    }
    (text, html)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_address() {
        assert_eq!(extract_addr("MAIL FROM:<a@b.test>"), "a@b.test");
        assert_eq!(extract_addr("RCPT TO:<c@d.test> SIZE=10"), "c@d.test");
    }

    #[test]
    fn parses_simple_message() {
        let session = Session {
            from: "env@from.test".into(),
            to: vec!["env@to.test".into()],
        };
        let data = "From: Alice <alice@test>\nTo: Bob <bob@test>\nSubject: Hi\n\nHello world\n";
        let email = parse_email(&session, data);
        assert_eq!(email.subject, "Hi");
        assert_eq!(email.from, "Alice <alice@test>");
        assert_eq!(email.text.as_deref(), Some("Hello world\n"));
    }

    #[test]
    fn parses_multipart() {
        let ct = "multipart/alternative; boundary=\"XYZ\"";
        let body = "--XYZ\nContent-Type: text/plain\n\nplain part\n--XYZ\nContent-Type: text/html\n\n<b>html part</b>\n--XYZ--\n";
        let (text, html) = extract_bodies(ct, body);
        assert_eq!(text.as_deref(), Some("plain part\n"));
        assert_eq!(html.as_deref(), Some("<b>html part</b>\n"));
    }
}
