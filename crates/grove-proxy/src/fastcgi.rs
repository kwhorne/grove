//! Minimal FastCGI client (PRD §8.2 — "egen minimal FastCGI-klient").
//!
//! Rather than depend on an unmaintained crate, Grove ships a tiny, focused
//! FastCGI responder client: enough to dispatch one request to a PHP-FPM pool
//! and stream the response back. It speaks FastCGI 1.0 over either a Unix
//! socket or TCP.

use std::collections::HashMap;

use bytes::{Buf, BufMut, BytesMut};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UnixStream};

const FCGI_VERSION: u8 = 1;
const FCGI_BEGIN_REQUEST: u8 = 1;
const FCGI_END_REQUEST: u8 = 3;
const FCGI_PARAMS: u8 = 4;
const FCGI_STDIN: u8 = 5;
const FCGI_STDOUT: u8 = 6;
const FCGI_STDERR: u8 = 7;
const FCGI_RESPONDER: u8 = 1;
const FCGI_KEEP_CONN: u8 = 0; // we open a fresh connection per request

const REQUEST_ID: u16 = 1;

#[derive(Debug, thiserror::Error)]
pub enum FastCgiError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("unexpected end of FastCGI stream")]
    UnexpectedEof,
    #[error("FastCGI protocol error: {0}")]
    Protocol(String),
}

/// The raw result of a FastCGI request: stdout (headers+body) and stderr.
#[derive(Debug, Default)]
pub struct FastCgiResponse {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Where the PHP-FPM pool is listening.
#[derive(Debug, Clone)]
pub enum FpmAddr {
    Unix(std::path::PathBuf),
    Tcp(std::net::SocketAddr),
}

/// Perform one FastCGI responder request.
pub async fn request(
    addr: &FpmAddr,
    params: &HashMap<String, String>,
    body: &[u8],
) -> Result<FastCgiResponse, FastCgiError> {
    match addr {
        FpmAddr::Unix(path) => {
            let stream = UnixStream::connect(path).await?;
            exchange(stream, params, body).await
        }
        FpmAddr::Tcp(sa) => {
            let stream = TcpStream::connect(sa).await?;
            exchange(stream, params, body).await
        }
    }
}

async fn exchange<S>(
    mut stream: S,
    params: &HashMap<String, String>,
    body: &[u8],
) -> Result<FastCgiResponse, FastCgiError>
where
    S: AsyncReadExt + AsyncWriteExt + Unpin,
{
    let mut out = BytesMut::new();

    // BEGIN_REQUEST
    let mut begin = BytesMut::new();
    begin.put_u16(FCGI_RESPONDER as u16);
    begin.put_u8(FCGI_KEEP_CONN);
    begin.put_bytes(0, 5); // reserved
    write_record(&mut out, FCGI_BEGIN_REQUEST, &begin);

    // PARAMS (name-value pairs), terminated by an empty PARAMS record.
    let mut param_buf = BytesMut::new();
    for (k, v) in params {
        encode_kv(&mut param_buf, k, v);
    }
    write_record(&mut out, FCGI_PARAMS, &param_buf);
    write_record(&mut out, FCGI_PARAMS, &[]); // empty = end of params

    // STDIN (request body), terminated by an empty STDIN record.
    if !body.is_empty() {
        // FastCGI content length per record is max 65535.
        for chunk in body.chunks(65535) {
            write_record(&mut out, FCGI_STDIN, chunk);
        }
    }
    write_record(&mut out, FCGI_STDIN, &[]);

    stream.write_all(&out).await?;
    stream.flush().await?;

    // Read response records until END_REQUEST.
    let mut resp = FastCgiResponse::default();
    loop {
        let header = read_exact(&mut stream, 8).await?;
        let _version = header[0];
        let rec_type = header[1];
        let _req_id = u16::from_be_bytes([header[2], header[3]]);
        let content_len = u16::from_be_bytes([header[4], header[5]]) as usize;
        let padding_len = header[6] as usize;

        let content = if content_len > 0 {
            read_exact(&mut stream, content_len).await?
        } else {
            Vec::new()
        };
        if padding_len > 0 {
            let _ = read_exact(&mut stream, padding_len).await?;
        }

        match rec_type {
            FCGI_STDOUT => resp.stdout.extend_from_slice(&content),
            FCGI_STDERR => resp.stderr.extend_from_slice(&content),
            FCGI_END_REQUEST => break,
            _ => {} // ignore unknown management records
        }
    }

    Ok(resp)
}

fn write_record(out: &mut BytesMut, rec_type: u8, content: &[u8]) {
    debug_assert!(content.len() <= u16::MAX as usize);
    out.put_u8(FCGI_VERSION);
    out.put_u8(rec_type);
    out.put_u16(REQUEST_ID);
    out.put_u16(content.len() as u16);
    out.put_u8(0); // padding length
    out.put_u8(0); // reserved
    out.put_slice(content);
}

/// Encode a FastCGI name-value pair with 1- or 4-byte length prefixes.
fn encode_kv(buf: &mut BytesMut, key: &str, val: &str) {
    encode_len(buf, key.len());
    encode_len(buf, val.len());
    buf.put_slice(key.as_bytes());
    buf.put_slice(val.as_bytes());
}

fn encode_len(buf: &mut BytesMut, len: usize) {
    if len < 128 {
        buf.put_u8(len as u8);
    } else {
        buf.put_u32(len as u32 | 0x8000_0000);
    }
}

async fn read_exact<S>(stream: &mut S, n: usize) -> Result<Vec<u8>, FastCgiError>
where
    S: AsyncReadExt + Unpin,
{
    let mut buf = vec![0u8; n];
    stream.read_exact(&mut buf).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            FastCgiError::UnexpectedEof
        } else {
            FastCgiError::Io(e)
        }
    })?;
    Ok(buf)
}

/// Split a raw FastCGI stdout payload into headers + body. PHP-FPM emits
/// CGI-style headers terminated by a blank line.
pub fn split_headers(stdout: &[u8]) -> (Vec<(String, String)>, Vec<u8>) {
    let mut headers = Vec::new();
    let mut bytes = BytesMut::from(stdout);

    // Find the header/body separator (\r\n\r\n or \n\n).
    let sep = find_subslice(stdout, b"\r\n\r\n")
        .map(|i| (i, 4))
        .or_else(|| find_subslice(stdout, b"\n\n").map(|i| (i, 2)));

    let Some((idx, sep_len)) = sep else {
        return (headers, stdout.to_vec());
    };

    let header_block = &stdout[..idx];
    for line in header_block.split(|&b| b == b'\n') {
        let line = trim_cr(line);
        if line.is_empty() {
            continue;
        }
        if let Some(colon) = line.iter().position(|&b| b == b':') {
            let name = String::from_utf8_lossy(&line[..colon]).trim().to_string();
            let value = String::from_utf8_lossy(&line[colon + 1..]).trim().to_string();
            headers.push((name, value));
        }
    }

    bytes.advance(idx + sep_len);
    (headers, bytes.to_vec())
}

fn trim_cr(line: &[u8]) -> &[u8] {
    line.strip_suffix(b"\r").unwrap_or(line)
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_short_length() {
        let mut buf = BytesMut::new();
        encode_len(&mut buf, 5);
        assert_eq!(&buf[..], &[5]);
    }

    #[test]
    fn encodes_long_length() {
        let mut buf = BytesMut::new();
        encode_len(&mut buf, 200);
        assert_eq!(buf.len(), 4);
        assert_eq!(buf[0] & 0x80, 0x80);
    }

    #[test]
    fn splits_cgi_headers() {
        let raw = b"Content-Type: text/html\r\nX-Powered-By: PHP\r\n\r\n<h1>hi</h1>";
        let (headers, body) = split_headers(raw);
        assert_eq!(headers.len(), 2);
        assert_eq!(headers[0].0, "Content-Type");
        assert_eq!(body, b"<h1>hi</h1>");
    }
}
