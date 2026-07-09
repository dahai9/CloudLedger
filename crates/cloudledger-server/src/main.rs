use std::net::SocketAddr;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let addr = std::env::var("CLOUDLEDGER_BIND_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:8787".to_string())
        .parse::<SocketAddr>()?;
    let state = cloudledger_server::ServerState::load_from_env()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!(
        "cloudledger-server listening on http://{addr} with server id {}",
        state.server_id
    );
    axum::serve(listener, cloudledger_server::router(state)).await?;
    Ok(())
}
