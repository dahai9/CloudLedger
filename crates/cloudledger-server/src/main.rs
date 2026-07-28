use std::{ffi::OsString, net::SocketAddr, path::PathBuf};

use cloudledger_server::config::{BackendConfig, DEFAULT_CONFIG_PATH};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config_path = parse_config_path(std::env::args_os().skip(1))?;
    let config = BackendConfig::load_or_create(&config_path)?;
    let addr = config.server.api_bind_addr;
    let admin_addr = config.server.admin_bind_addr;
    let state = cloudledger_server::ServerState::load_from_config(&config).await?;

    let listener = tokio::net::TcpListener::bind(addr).await?;
    let admin_listener = tokio::net::TcpListener::bind(admin_addr).await?;
    println!(
        "cloudledger-server API listening on http://{addr} with server id {}",
        state.server_id
    );
    println!(
        "cloudledger-server admin listening on http://{admin_addr}/{}; backend config: {}",
        state.admin_path,
        config_path.display()
    );
    let api_server = axum::serve(
        listener,
        cloudledger_server::router(state.clone())
            .into_make_service_with_connect_info::<SocketAddr>(),
    );
    let admin_server = axum::serve(
        admin_listener,
        cloudledger_server::admin_router(state).into_make_service_with_connect_info::<SocketAddr>(),
    );
    tokio::try_join!(api_server, admin_server)?;
    Ok(())
}

fn parse_config_path(args: impl IntoIterator<Item = OsString>) -> anyhow::Result<PathBuf> {
    let mut args = args.into_iter();
    let Some(argument) = args.next() else {
        return Ok(PathBuf::from(DEFAULT_CONFIG_PATH));
    };
    if argument != "--config" {
        anyhow::bail!(
            "unknown argument {}; usage: cloudledger-server [--config <path>]",
            argument.to_string_lossy()
        );
    }
    let path = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("--config requires a file path"))?;
    if args.next().is_some() {
        anyhow::bail!("usage: cloudledger-server [--config <path>]");
    }
    Ok(PathBuf::from(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_path_defaults_and_accepts_override() {
        assert_eq!(
            parse_config_path(Vec::<OsString>::new()).unwrap(),
            PathBuf::from(DEFAULT_CONFIG_PATH)
        );
        assert_eq!(
            parse_config_path([OsString::from("--config"), OsString::from("other.toml")]).unwrap(),
            PathBuf::from("other.toml")
        );
        assert!(parse_config_path([OsString::from("--unknown")]).is_err());
    }
}
