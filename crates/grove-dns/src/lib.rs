//! Embedded authoritative resolver for `*.<tld>` (PRD §6.1).
//!
//! The resolver is deliberately *not* an open resolver: it only answers for the
//! configured TLD (e.g. `test`) and returns loopback. Anything else gets
//! REFUSED so a misconfigured system resolver can't accidentally route real
//! traffic through Grove.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use hickory_proto::op::{Header, MessageType, OpCode, ResponseCode};
use hickory_proto::rr::rdata::{A, AAAA};
use hickory_proto::rr::{Name, RData, Record, RecordType};
use hickory_server::authority::MessageResponseBuilder;
use hickory_server::server::{Request, RequestHandler, ResponseHandler, ResponseInfo};
use hickory_server::ServerFuture;
use tokio::net::{TcpListener, UdpSocket};

#[derive(Debug, thiserror::Error)]
pub enum DnsError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("dns protocol: {0}")]
    Proto(#[from] hickory_proto::error::ProtoError),
}

/// Handler that maps every name ending in `.<tld>` to loopback.
#[derive(Clone)]
pub struct GroveResolver {
    tld: String,
}

impl GroveResolver {
    pub fn new(tld: impl Into<String>) -> Self {
        Self { tld: into_label(tld) }
    }

    fn owns(&self, name: &Name) -> bool {
        let lower = name.to_lowercase().to_utf8();
        let host = lower.trim_end_matches('.');
        host == self.tld || host.ends_with(&format!(".{}", self.tld))
    }
}

fn into_label(tld: impl Into<String>) -> String {
    tld.into().trim_matches('.').to_lowercase()
}

#[async_trait::async_trait]
impl RequestHandler for GroveResolver {
    async fn handle_request<R: ResponseHandler>(
        &self,
        request: &Request,
        mut response_handle: R,
    ) -> ResponseInfo {
        let query = request.query();
        let name = query.name();
        let fqdn: Name = name.into();

        // Only answer standard queries for our TLD.
        if request.op_code() != OpCode::Query
            || request.message_type() != MessageType::Query
            || !self.owns(&fqdn)
        {
            return refuse(request, &mut response_handle).await;
        }

        let records: Vec<Record> = match query.query_type() {
            RecordType::A => vec![Record::from_rdata(
                fqdn.clone(),
                0,
                RData::A(A(Ipv4Addr::LOCALHOST)),
            )],
            RecordType::AAAA => vec![Record::from_rdata(
                fqdn.clone(),
                0,
                RData::AAAA(AAAA(Ipv6Addr::LOCALHOST)),
            )],
            // For everything else (e.g. MX, TXT) answer with an empty NOERROR so
            // resolvers don't keep retrying.
            _ => Vec::new(),
        };

        let builder = MessageResponseBuilder::from_message_request(request);
        let mut header = Header::response_from_request(request.header());
        header.set_authoritative(true);
        let response = builder.build(header, records.iter(), &[], &[], &[]);

        match response_handle.send_response(response).await {
            Ok(info) => info,
            Err(e) => {
                tracing::error!(error = %e, "failed to send DNS response");
                ResponseInfo::from(header)
            }
        }
    }
}

async fn refuse<R: ResponseHandler>(request: &Request, handle: &mut R) -> ResponseInfo {
    let builder = MessageResponseBuilder::from_message_request(request);
    let response = builder.error_msg(request.header(), ResponseCode::Refused);
    match handle.send_response(response).await {
        Ok(info) => info,
        Err(_) => {
            let mut header = Header::response_from_request(request.header());
            header.set_response_code(ResponseCode::Refused);
            ResponseInfo::from(header)
        }
    }
}

/// Bind UDP+TCP on `addr:port` and serve the resolver until the future is
/// dropped/aborted.
pub async fn serve(tld: &str, addr: SocketAddr) -> Result<ServerFuture<GroveResolver>, DnsError> {
    let handler = GroveResolver::new(tld);
    let mut server = ServerFuture::new(handler);

    let udp = UdpSocket::bind(addr).await?;
    server.register_socket(udp);

    let tcp = TcpListener::bind(addr).await?;
    server.register_listener(tcp, Duration::from_secs(5));

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
    }
}
