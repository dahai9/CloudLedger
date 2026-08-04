use std::{ffi::OsString, net::SocketAddr, path::PathBuf};

use cloudledger_server::{
    audit::AuditSigner,
    config::{BackendConfig, RunMode, DEFAULT_CONFIG_PATH},
    storage::PostgresStore,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (command, config_path) = parse_command(std::env::args_os().skip(1))?;
    let config = BackendConfig::load_or_create(&config_path)?;
    if command == Command::Migrate {
        let migration_url = std::env::var("CLOUDLEDGER_MIGRATION_DATABASE_URL").map_err(|_| {
            anyhow::anyhow!(
                "CLOUDLEDGER_MIGRATION_DATABASE_URL is required for the one-time migrate command"
            )
        })?;
        config.validate_migration_database_url(&migration_url)?;
        let mut migration_database = config.database.clone();
        migration_database.url = migration_url;
        migration_database.auto_migrate = true;
        let store = PostgresStore::connect_with_audit(
            &migration_database,
            AuditSigner::from_config(&config.security.audit)?,
        )
        .await?;
        let report = store.verify_audit().await?;
        println!(
            "migration complete; audit chains: {}, events: {}",
            report.chains, report.events
        );
        return Ok(());
    }
    if command == Command::AuditVerify {
        let store = PostgresStore::connect_with_audit(
            &config.database,
            AuditSigner::from_config(&config.security.audit)?,
        )
        .await?;
        let report = store.verify_audit().await?;
        println!("{}", serde_json::to_string(&report)?);
        return Ok(());
    }
    let addr = config.server.api_bind_addr;
    let admin_addr = config.server.admin_bind_addr;
    let state = cloudledger_server::ServerState::load_from_config(&config).await?;

    if config.server.mode == RunMode::Development && config.server.allow_insecure_lan {
        eprintln!(
            "WARNING: INSECURE DEVELOPMENT LAN MODE ENABLED; credentials and data may cross plaintext HTTP"
        );
    }

    let listener = tokio::net::TcpListener::bind(addr).await?;
    let admin_listener = tokio::net::TcpListener::bind(admin_addr).await?;
    println!(
        "cloudledger-server API listening on http://{addr} (public {}) with server id {}",
        config.server.public_api_url, state.server_id
    );
    println!(
        "cloudledger-server admin listening on http://{admin_addr}/{} (public {}); backend config: {}",
        state.admin_path,
        config.server.public_admin_url,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    Serve,
    Migrate,
    AuditVerify,
}

fn parse_command(args: impl IntoIterator<Item = OsString>) -> anyhow::Result<(Command, PathBuf)> {
    let mut args = args.into_iter().peekable();
    let command = if args.peek().is_some_and(|value| value == "audit") {
        args.next();
        if args.next().as_deref() != Some(std::ffi::OsStr::new("verify")) {
            anyhow::bail!("usage: cloudledger-server audit verify [--config <path>]");
        }
        Command::AuditVerify
    } else if args.peek().is_some_and(|value| value == "migrate") {
        args.next();
        Command::Migrate
    } else {
        Command::Serve
    };
    let Some(argument) = args.next() else {
        return Ok((command, PathBuf::from(DEFAULT_CONFIG_PATH)));
    };
    if argument != "--config" {
        anyhow::bail!("usage: cloudledger-server [migrate|audit verify] [--config <path>]");
    }
    let path = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("--config requires a file path"))?;
    if args.next().is_some() {
        anyhow::bail!("usage: cloudledger-server [migrate|audit verify] [--config <path>]");
    }
    Ok((command, PathBuf::from(path)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_path_defaults_and_accepts_override() {
        assert_eq!(
            parse_command(Vec::<OsString>::new()).unwrap(),
            (Command::Serve, PathBuf::from(DEFAULT_CONFIG_PATH))
        );
        assert_eq!(
            parse_command([OsString::from("migrate")]).unwrap(),
            (Command::Migrate, PathBuf::from(DEFAULT_CONFIG_PATH))
        );
        assert_eq!(
            parse_command([OsString::from("--config"), OsString::from("other.toml")]).unwrap(),
            (Command::Serve, PathBuf::from("other.toml"))
        );
        assert_eq!(
            parse_command([
                OsString::from("audit"),
                OsString::from("verify"),
                OsString::from("--config"),
                OsString::from("other.toml")
            ])
            .unwrap(),
            (Command::AuditVerify, PathBuf::from("other.toml"))
        );
        assert!(parse_command([OsString::from("--unknown")]).is_err());
    }
}
