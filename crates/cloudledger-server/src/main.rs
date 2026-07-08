use std::net::SocketAddr;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let addr = SocketAddr::from(([127, 0, 0, 1], 8787));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("cloudledger-server listening on http://{addr}");
    axum::serve(listener, cloudledger_server::router()).await?;
    Ok(())
}
