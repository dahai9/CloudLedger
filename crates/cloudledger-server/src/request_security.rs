use std::{net::IpAddr, sync::Arc};

use axum::{
    extract::{ConnectInfo, Request, State},
    http::{
        header::{HeaderName, FORWARDED},
        HeaderMap,
    },
    middleware::Next,
    response::Response,
};
use ipnet::IpNet;

use crate::config::NetworkSecurityConfig;

static X_FORWARDED_FOR: HeaderName = HeaderName::from_static("x-forwarded-for");
static X_FORWARDED_PROTO: HeaderName = HeaderName::from_static("x-forwarded-proto");

#[derive(Debug, Clone, Copy)]
pub struct RequestContext {
    pub client_ip: IpAddr,
    pub forwarded_https: bool,
}

#[derive(Debug, Clone)]
pub struct RequestSecurity {
    trusted_proxy_cidrs: Arc<[IpNet]>,
}

impl RequestSecurity {
    pub fn from_config(config: &NetworkSecurityConfig) -> Self {
        Self {
            trusted_proxy_cidrs: config.trusted_proxy_cidrs.clone().into(),
        }
    }

    fn trusts(&self, ip: IpAddr) -> bool {
        self.trusted_proxy_cidrs
            .iter()
            .any(|network| network.contains(&ip))
    }

    fn context(&self, peer_ip: IpAddr, headers: &HeaderMap) -> RequestContext {
        let trusted_peer = self.trusts(peer_ip);
        let client_ip = if trusted_peer {
            headers
                .get(&X_FORWARDED_FOR)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.split(',').next())
                .and_then(|value| value.trim().parse().ok())
                .unwrap_or(peer_ip)
        } else {
            peer_ip
        };
        let forwarded_https = trusted_peer
            && headers
                .get(&X_FORWARDED_PROTO)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.eq_ignore_ascii_case("https"));
        RequestContext {
            client_ip,
            forwarded_https,
        }
    }
}

pub async fn resolve_request_context(
    State(security): State<RequestSecurity>,
    mut request: Request,
    next: Next,
) -> Response {
    let peer_ip = request
        .extensions()
        .get::<ConnectInfo<std::net::SocketAddr>>()
        .map(|ConnectInfo(address)| address.ip())
        .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
    let context = security.context(peer_ip, request.headers());

    // RFC 7239 is deliberately ignored. Only the configured reverse proxy may
    // supply the single forwarding format CloudLedger understands.
    request.headers_mut().remove(FORWARDED);
    request.extensions_mut().insert(context);
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_only_configured_proxy_networks() {
        let security = RequestSecurity::from_config(&NetworkSecurityConfig {
            trusted_proxy_cidrs: vec!["127.0.0.1/32".parse().unwrap()],
            cors_allowed_origins: Vec::new(),
        });
        assert!(security.trusts("127.0.0.1".parse().unwrap()));
        assert!(!security.trusts("192.0.2.10".parse().unwrap()));

        let mut headers = HeaderMap::new();
        headers.insert(&X_FORWARDED_FOR, "198.51.100.7".parse().unwrap());
        headers.insert(&X_FORWARDED_PROTO, "https".parse().unwrap());
        let trusted = security.context("127.0.0.1".parse().unwrap(), &headers);
        assert_eq!(trusted.client_ip, "198.51.100.7".parse::<IpAddr>().unwrap());
        assert!(trusted.forwarded_https);
        let untrusted = security.context("192.0.2.10".parse().unwrap(), &headers);
        assert_eq!(untrusted.client_ip, "192.0.2.10".parse::<IpAddr>().unwrap());
        assert!(!untrusted.forwarded_https);
    }
}
