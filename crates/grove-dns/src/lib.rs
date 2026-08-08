//! Embedded authoritative resolver for `*.<tld>`.
//!
//! The resolver is deliberately *not* an open resolver: it only answers for the
//! configured TLD (e.g. `test`) and returns loopback. Anything else gets
//! REFUSED so a misconfigured system resolver can't accidentally route real
//! traffic through Grove.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use hickory_proto::op::{Header, HeaderCounts, MessageType, Metadata, OpCode, ResponseCode};
use hickory_proto::rr::rdata::{A, AAAA};
use hickory_proto::rr::{Name, RData, Record, RecordType};
use hickory_proto::ProtoError;
use hickory_server::net::runtime::Time;
use hickory_server::server::{Request, RequestHandler, ResponseHandler, ResponseInfo};
use hickory_server::zone_handler::MessageResponseBuilder;
use hickory_server::Server;
use tokio::net::{TcpListener, UdpSocket};

#[derive(Debug, thiserror::Error)]
pub enum DnsError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("dns protocol: {0}")]
    Proto(#[from] ProtoError),
}

/// How long a resolver may cache a `*.<tld>` answer.
///
/// A TTL of `0` forbids caching, so the system resolver asked Grove again for
/// every single connection — and on macOS `mDNSResponder` sits in that path,
/// adding a round trip to the first byte of every request. The answer is always
/// loopback and never changes, so there is nothing to keep fresh; sites
/// added or removed do not change what this returns.
const RECORD_TTL: u32 = 300;

/// Bytes of outgoing responses buffered per TCP connection.
///
/// Grove's answers are one small record, so this only has to cover a burst from
/// a single resolver; it is a bound on memory per connection, not a target.
const RESPONSE_BUFFER_SIZE: usize = 8 * 1024;

/// Handler that maps every name ending in `.<tld>` to loopback.
#[derive(Clone)]
pub struct GroveResolver {
    tld: String,
}

impl GroveResolver {
    pub fn new(tld: impl Into<String>) -> Self {
        Self {
            tld: into_label(tld),
        }
    }

    fn owns(&self, name: &Name) -> bool {
        let lower = name.to_lowercase().to_utf8();
        let host = lower.trim_end_matches('.');
        // No `format!` here: this runs on every DNS query, and the allocation was
        // only ever needed to prepend a dot.
        match host.strip_suffix(&self.tld) {
            Some("") => true,
            Some(rest) => rest.ends_with('.'),
            None => false,
        }
    }
}

fn into_label(tld: impl Into<String>) -> String {
    tld.into().trim_matches('.').to_lowercase()
}

/// Build a `ResponseInfo` for a response that could not be sent.
///
/// Only reached when writing to the socket failed, so nothing reads the record
/// counts; they exist because `ResponseInfo` is built from a full header.
fn info_for(metadata: Metadata) -> ResponseInfo {
    ResponseInfo::from(Header {
        metadata,
        counts: HeaderCounts::default(),
    })
}

#[async_trait::async_trait]
impl RequestHandler for GroveResolver {
    async fn handle_request<R: ResponseHandler, T: Time>(
        &self,
        request: &Request,
        mut response_handle: R,
    ) -> ResponseInfo {
        // `request_info` enforces exactly one question, which is the only shape
        // a resolver ever sends; anything else is refused rather than guessed at.
        let Ok(info) = request.request_info() else {
            return refuse(request, &mut response_handle).await;
        };
        let fqdn: Name = info.query.name().into();
        let query_type = info.query.query_type();

        // Only answer standard queries for our TLD.
        if request.metadata.op_code != OpCode::Query
            || request.metadata.message_type != MessageType::Query
            || !self.owns(&fqdn)
        {
            return refuse(request, &mut response_handle).await;
        }

        let records: Vec<Record> = match query_type {
            RecordType::A => vec![Record::from_rdata(
                fqdn.clone(),
                RECORD_TTL,
                RData::A(A(Ipv4Addr::LOCALHOST)),
            )],
            RecordType::AAAA => vec![Record::from_rdata(
                fqdn.clone(),
                RECORD_TTL,
                RData::AAAA(AAAA(Ipv6Addr::LOCALHOST)),
            )],
            // For everything else (e.g. MX, TXT) answer with an empty NOERROR so
            // resolvers don't keep retrying.
            _ => Vec::new(),
        };

        let builder = MessageResponseBuilder::from_message_request(request);
        let mut metadata = Metadata::response_from_request(&request.metadata);
        metadata.authoritative = true;
        let response = builder.build(metadata, records.iter(), &[], &[], &[]);

        match response_handle.send_response(response).await {
            Ok(info) => info,
            Err(e) => {
                tracing::error!(error = %e, "failed to send DNS response");
                info_for(metadata)
            }
        }
    }
}

async fn refuse<R: ResponseHandler>(request: &Request, handle: &mut R) -> ResponseInfo {
    let builder = MessageResponseBuilder::from_message_request(request);
    let response = builder.error_msg(&request.metadata, ResponseCode::Refused);
    match handle.send_response(response).await {
        Ok(info) => info,
        Err(_) => {
            let mut metadata = Metadata::response_from_request(&request.metadata);
            metadata.response_code = ResponseCode::Refused;
            info_for(metadata)
        }
    }
}

/// Bind UDP+TCP on `addr:port` and serve the resolver until the future is
/// dropped/aborted.
pub async fn serve(tld: &str, addr: SocketAddr) -> Result<Server<GroveResolver>, DnsError> {
    let handler = GroveResolver::new(tld);
    let mut server = Server::new(handler);

    let udp = UdpSocket::bind(addr).await?;
    server.register_socket(udp);

    let tcp = TcpListener::bind(addr).await?;
    server.register_listener(tcp, Duration::from_secs(5), RESPONSE_BUFFER_SIZE);

    tracing::info!(%addr, tld, "DNS resolver listening");
    Ok(server)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn owns_only_configured_tld() {
        let r = GroveResolver::new("test");
        assert!(r.owns(&Name::from_str("myapp.test.").unwrap()));
        assert!(r.owns(&Name::from_str("api.myapp.test.").unwrap()));
        assert!(!r.owns(&Name::from_str("example.com.").unwrap()));
        assert!(!r.owns(&Name::from_str("nottest.").unwrap()));
        // The allocation-free suffix check must not accept a name that merely
        // ends in the TLD's letters.
        assert!(!r.owns(&Name::from_str("mytest.").unwrap()));
        assert!(!r.owns(&Name::from_str("foo.latest.").unwrap()));
        assert!(r.owns(&Name::from_str("MyApp.TEST.").unwrap()));
    }
}
