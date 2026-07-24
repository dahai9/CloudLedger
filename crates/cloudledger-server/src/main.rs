use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let addr = std::env::var("CLOUDLEDGER_BIND_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:8787".to_string())
        .parse::<SocketAddr>()?;
    let admin_addr = std::env::var("CLOUDLEDGER_ADMIN_BIND_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8788".to_string())
        .parse::<SocketAddr>()?;
    validate_admin_bind_addr(&admin_addr)?;

    let state = cloudledger_server::ServerState::load_from_env()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let admin_listener = tokio::net::TcpListener::bind(admin_addr).await?;
    println!(
        "cloudledger-server API listening on http://{addr} with server id {}",
        state.server_id
    );
    println!(
        "cloudledger-server admin listening on http://{admin_addr}/admin; token file: {}",
        state.data_dir.join("admin-token").display()
    );
    let api_server = axum::serve(listener, cloudledger_server::router(state.clone()));
    let admin_server = axum::serve(admin_listener, cloudledger_server::admin_router(state));
    tokio::try_join!(api_server, admin_server)?;
    Ok(())
}

fn validate_admin_bind_addr(addr: &SocketAddr) -> anyhow::Result<()> {
    if is_private_or_loopback(addr.ip()) {
        Ok(())
    } else {
        anyhow::bail!("CLOUDLEDGER_ADMIN_BIND_ADDR must be loopback or private LAN, got {addr}")
    }
}

fn is_private_or_loopback(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_loopback() || ip.is_private() || is_ipv4_link_local(ip),
        IpAddr::V6(ip) => ip.is_loopback() || is_ipv6_unique_local(ip) || is_ipv6_link_local(ip),
    }
}

fn is_ipv4_link_local(ip: Ipv4Addr) -> bool {
    let [first, second, _, _] = ip.octets();
    first == 169 && second == 254
}

fn is_ipv6_unique_local(ip: Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xfe00) == 0xfc00
}

fn is_ipv6_link_local(ip: Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xffc0) == 0xfe80
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_bind_allows_loopback_and_lan_only() {
        assert!("127.0.0.1:8788"
            .parse::<SocketAddr>()
            .map(|addr| validate_admin_bind_addr(&addr).is_ok())
            .unwrap());
        assert!("192.168.1.229:8788"
            .parse::<SocketAddr>()
            .map(|addr| validate_admin_bind_addr(&addr).is_ok())
            .unwrap());
        assert!("10.64.1.212:8788"
            .parse::<SocketAddr>()
            .map(|addr| validate_admin_bind_addr(&addr).is_ok())
            .unwrap());

        assert!("0.0.0.0:8788"
            .parse::<SocketAddr>()
            .map(|addr| validate_admin_bind_addr(&addr).is_err())
            .unwrap());
        assert!("8.8.8.8:8788"
            .parse::<SocketAddr>()
            .map(|addr| validate_admin_bind_addr(&addr).is_err())
            .unwrap());
    }
}
