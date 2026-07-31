//! Minimal FastCGI client ("egen minimal FastCGI-klient").
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

/// CGI response headers, as name/value pairs in the order PHP emitted them.
pub type CgiHeaders = Vec<(String, String)>;

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

/// A stream of response body chunks, one per FastCGI `STDOUT` record.
pub type BodyStream = tokio::sync::mpsc::Receiver<Result<bytes::Bytes, std::io::Error>>;

/// How many body chunks may sit in the channel before the reader task waits.
/// Small on purpose: it bounds memory for a fast producer and a slow client,
/// and back-pressure is the correct behaviour rather than unbounded buffering.
const BODY_CHANNEL_DEPTH: usize = 16;

/// Perform one FastCGI responder request, buffering the whole response.
///
/// Prefer [`request_streaming`] for anything user-facing: this holds the entire
/// response in memory and returns nothing until PHP closes the request.
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

/// Perform one FastCGI responder request and return as soon as PHP has flushed
/// its headers.
///
/// The response body arrives as a stream of chunks, one per `STDOUT` record, so
/// a Server-Sent Events endpoint reaches the client while PHP is still running
/// and a large download is never held in memory. The connection is owned by a
/// spawned task; if the receiver is dropped (the client went away) the task
/// stops reading and closes the connection, releasing the PHP-FPM worker.
pub async fn request_streaming(
    addr: &FpmAddr,
    params: &HashMap<String, String>,
    body: &[u8],
) -> Result<(CgiHeaders, BodyStream), FastCgiError> {
    match addr {
        FpmAddr::Unix(path) => {
            let stream = UnixStream::connect(path).await?;
            exchange_streaming(stream, params, body).await
        }
        FpmAddr::Tcp(sa) => {
            let stream = TcpStream::connect(sa).await?;
            exchange_streaming(stream, params, body).await
        }
    }
}

async fn exchange_streaming<S>(
    mut stream: S,
    params: &HashMap<String, String>,
    body: &[u8],
) -> Result<(CgiHeaders, BodyStream), FastCgiError>
where
    S: AsyncReadExt + AsyncWriteExt + Unpin + Send + 'static,
{
    write_request(&mut stream, params, body).await?;

    // Phase 1: read records until the CGI header block is complete. Only the
    // headers are buffered; the body never is.
    let mut head = Vec::new();
    let mut early_stderr = Vec::new();
    let mut ended = false;

    let (headers, leftover) = loop {
        match read_record(&mut stream).await? {
            Some((FCGI_STDOUT, content)) => {
                head.extend_from_slice(&content);
                if let Some(split) = try_split_headers(&head) {
                    break split;
                }
            }
            Some((FCGI_STDERR, content)) => early_stderr.extend_from_slice(&content),
            Some((FCGI_END_REQUEST, _)) | None => {
                // PHP finished without ever completing a header block. Hand back
                // whatever it wrote so the caller can surface it.
                ended = true;
                break (Vec::new(), std::mem::take(&mut head));
            }
            Some(_) => {} // management records
        }
    };

    if !early_stderr.is_empty() {
        tracing::warn!(stderr = %String::from_utf8_lossy(&early_stderr), "php stderr");
    }

    let (tx, rx) = tokio::sync::mpsc::channel(BODY_CHANNEL_DEPTH);

    // Bytes that followed the separator inside the same record have already been
    // read. They are the first part of the body and must not be dropped — losing
    // them silently swallows the first SSE event.
    if !leftover.is_empty() {
        let _ = tx.send(Ok(bytes::Bytes::from(leftover))).await;
    }
    if ended {
        return Ok((headers, rx));
    }

    // Phase 2: pump the rest of the body without buffering it.
    tokio::spawn(async move {
        loop {
            match read_record(&mut stream).await {
                Ok(Some((FCGI_STDOUT, content))) => {
                    if content.is_empty() {
                        continue;
                    }
                    if tx.send(Ok(bytes::Bytes::from(content))).await.is_err() {
                        // Receiver dropped: the client disconnected. Stop reading
                        // so `stream` drops and the FPM worker is freed instead of
                        // being held for the life of an abandoned stream.
                        break;
                    }
                }
                Ok(Some((FCGI_STDERR, content))) => {
                    // Headers are already on the wire, so this can only be logged.
                    tracing::warn!(stderr = %String::from_utf8_lossy(&content), "php stderr");
                }
                Ok(Some((FCGI_END_REQUEST, _))) | Ok(None) => break,
                Ok(Some(_)) => {}
                Err(e) => {
                    let _ = tx.send(Err(std::io::Error::other(e))).await;
                    break;
                }
            }
        }
    });

    Ok((headers, rx))
}

async fn exchange<S>(
    mut stream: S,
    params: &HashMap<String, String>,
    body: &[u8],
) -> Result<FastCgiResponse, FastCgiError>
where
    S: AsyncReadExt + AsyncWriteExt + Unpin,
{
    write_request(&mut stream, params, body).await?;

    let mut resp = FastCgiResponse::default();
    loop {
        match read_record(&mut stream).await? {
            Some((FCGI_STDOUT, content)) => resp.stdout.extend_from_slice(&content),
            Some((FCGI_STDERR, content)) => resp.stderr.extend_from_slice(&content),
            Some((FCGI_END_REQUEST, _)) | None => break,
            Some(_) => {} // ignore unknown management records
        }
    }

    Ok(resp)
}

/// Write BEGIN_REQUEST + PARAMS + STDIN for one responder request.
async fn write_request<S>(
    stream: &mut S,
    params: &HashMap<String, String>,
    body: &[u8],
) -> Result<(), FastCgiError>
where
    S: AsyncWriteExt + Unpin,
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
    Ok(())
}

/// Read one FastCGI record. `Ok(None)` means the peer closed cleanly between
/// records, which some FPM builds do instead of sending END_REQUEST.
async fn read_record<S>(stream: &mut S) -> Result<Option<(u8, Vec<u8>)>, FastCgiError>
where
    S: AsyncReadExt + Unpin,
{
    let header = match read_exact(&mut *stream, 8).await {
        Ok(h) => h,
        Err(FastCgiError::UnexpectedEof) => return Ok(None),
        Err(e) => return Err(e),
    };
    let rec_type = header[1];
    let content_len = u16::from_be_bytes([header[4], header[5]]) as usize;
    let padding_len = header[6] as usize;

    let content = if content_len > 0 {
        read_exact(&mut *stream, content_len).await?
    } else {
        Vec::new()
    };
    if padding_len > 0 {
        let _ = read_exact(&mut *stream, padding_len).await?;
    }
    Ok(Some((rec_type, content)))
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
pub fn split_headers(stdout: &[u8]) -> (CgiHeaders, Vec<u8>) {
    try_split_headers(stdout).unwrap_or_else(|| (Vec::new(), stdout.to_vec()))
}

/// Like [`split_headers`], but `None` when the header block is not complete yet.
///
/// This is what makes streaming possible: the caller feeds in whatever has
/// arrived so far and only commits once the separator is present. The returned
/// body bytes are the remainder of the buffer *after* the separator, which for a
/// streamed response is the first chunk of the body.
pub fn try_split_headers(stdout: &[u8]) -> Option<(CgiHeaders, Vec<u8>)> {
    let mut headers = Vec::new();
    let mut bytes = BytesMut::from(stdout);

    // Find the header/body separator (\r\n\r\n or \n\n).
    let (idx, sep_len) = find_subslice(stdout, b"\r\n\r\n")
        .map(|i| (i, 4))
        .or_else(|| find_subslice(stdout, b"\n\n").map(|i| (i, 2)))?;

    let header_block = &stdout[..idx];
    for line in header_block.split(|&b| b == b'\n') {
        let line = trim_cr(line);
        if line.is_empty() {
            continue;
        }
        if let Some(colon) = line.iter().position(|&b| b == b':') {
            let name = String::from_utf8_lossy(&line[..colon]).trim().to_string();
            let value = String::from_utf8_lossy(&line[colon + 1..])
                .trim()
                .to_string();
            headers.push((name, value));
        }
    }

    bytes.advance(idx + sep_len);
    Some((headers, bytes.to_vec()))
}

fn trim_cr(line: &[u8]) -> &[u8] {
    line.strip_suffix(b"\r").unwrap_or(line)
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
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

    #[test]
    fn try_split_waits_for_the_complete_header_block() {
        // Streaming feeds bytes in as they arrive; committing early would treat
        // half a header block as headers.
        assert!(try_split_headers(b"Content-Type: text/event-stream").is_none());
        assert!(try_split_headers(b"Content-Type: text/event-stream\r\n").is_none());
        assert!(try_split_headers(b"Content-Type: text/event-stream\r\n\r").is_none());
        assert!(try_split_headers(b"Content-Type: text/event-stream\r\n\r\n").is_some());
    }

    #[test]
    fn try_split_keeps_body_bytes_that_share_the_header_record() {
        // The regression this guards: PHP flushes headers and the first SSE event
        // in one STDOUT record. The bytes after the separator are already read, so
        // if they are not returned as the first body chunk the first event is
        // silently lost and it looks like PHP sent nothing.
        let raw = b"Content-Type: text/event-stream\r\n\r\ndata: first\n\n";
        let (headers, body) = try_split_headers(raw).expect("header block is complete");
        assert_eq!(headers.len(), 1);
        assert_eq!(body, b"data: first\n\n");
    }

    #[test]
    fn try_split_handles_bare_lf_separator() {
        let raw = b"Status: 302 Found\nLocation: /login\n\nredirecting";
        let (headers, body) = try_split_headers(raw).expect("complete");
        assert_eq!(headers.len(), 2);
        assert_eq!(body, b"redirecting");
    }

    /// Write one FastCGI record straight to a socket (test-side FPM).
    async fn send_record<S>(sock: &mut S, rec_type: u8, content: &[u8])
    where
        S: AsyncWriteExt + Unpin,
    {
        let mut buf = BytesMut::new();
        write_record(&mut buf, rec_type, content);
        sock.write_all(&buf).await.unwrap();
        sock.flush().await.unwrap();
    }

    /// The behaviour this whole change exists for: a chunk must reach the caller
    /// while the FastCGI request is still open.
    ///
    /// The fake FPM waits for a signal from the test before writing the second
    /// event, and the test only sends that signal after it has received the
    /// first. So the assertions cannot pass unless the first chunk was delivered
    /// before the second was even written — which a buffering client could never
    /// do. No sleeps, so no timing flake.
    #[tokio::test]
    async fn delivers_chunks_before_the_request_ends() {
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (go_tx, go_rx) = tokio::sync::oneshot::channel::<()>();

        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            // Drain the request; its contents don't matter here.
            let mut scratch = vec![0u8; 8192];
            let _ = sock.read(&mut scratch).await;

            // Headers and the first event share one record, the awkward case.
            send_record(
                &mut sock,
                FCGI_STDOUT,
                b"Content-Type: text/event-stream\r\n\r\ndata: 1\n\n",
            )
            .await;

            go_rx.await.expect("test signals after first chunk");

            send_record(&mut sock, FCGI_STDOUT, b"data: 2\n\n").await;
            send_record(&mut sock, FCGI_END_REQUEST, &[0u8; 8]).await;
        });

        let (headers, mut rx) = request_streaming(&FpmAddr::Tcp(addr), &HashMap::new(), b"")
            .await
            .expect("request starts");

        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].0, "Content-Type");
        assert_eq!(headers[0].1, "text/event-stream");

        let first = recv_chunk(&mut rx).await.expect("first chunk").unwrap();
        assert_eq!(
            &first[..],
            b"data: 1\n\n",
            "body bytes from the header record"
        );

        go_tx.send(()).unwrap();

        let second = recv_chunk(&mut rx).await.expect("second chunk").unwrap();
        assert_eq!(&second[..], b"data: 2\n\n");

        assert!(
            recv_chunk(&mut rx).await.is_none(),
            "END_REQUEST must close the stream"
        );
    }

    /// Receive one chunk, failing instead of hanging if the client buffers.
    async fn recv_chunk(rx: &mut BodyStream) -> Option<Result<bytes::Bytes, std::io::Error>> {
        tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("a buffering client would hang here")
    }

    /// A vanished client must not leave the FPM worker occupied.
    #[tokio::test]
    async fn dropping_the_receiver_stops_reading() {
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (closed_tx, closed_rx) = tokio::sync::oneshot::channel::<()>();

        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut scratch = vec![0u8; 8192];
            let _ = sock.read(&mut scratch).await;

            send_record(&mut sock, FCGI_STDOUT, b"Content-Type: text/plain\r\n\r\n").await;

            // Keep producing until the client's side goes away. Writes fail once
            // the reader task drops the connection.
            loop {
                let mut buf = BytesMut::new();
                write_record(&mut buf, FCGI_STDOUT, b"x");
                if sock.write_all(&buf).await.is_err() {
                    break;
                }
                tokio::task::yield_now().await;
            }
            let _ = closed_tx.send(());
        });

        let (_headers, rx) = request_streaming(&FpmAddr::Tcp(addr), &HashMap::new(), b"")
            .await
            .expect("request starts");

        // The client disconnects: hyper drops the body, so the receiver drops.
        drop(rx);

        tokio::time::timeout(std::time::Duration::from_secs(5), closed_rx)
            .await
            .expect("reader task should close the connection, freeing the FPM worker")
            .expect("server signals");
    }

    #[test]
    fn try_split_returns_empty_body_when_headers_end_the_record() {
        let raw = b"Content-Type: text/plain\r\n\r\n";
        let (headers, body) = try_split_headers(raw).expect("complete");
        assert_eq!(headers.len(), 1);
        assert!(body.is_empty(), "no body bytes yet, not a lost chunk");
    }
}
