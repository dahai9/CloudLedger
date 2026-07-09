use std::{
    fs,
    path::{Path, PathBuf},
};

use uuid::Uuid;

const DEFAULT_DATA_DIR: &str = ".cloudledger-server";
const SERVER_ID_FILE: &str = "server-id";

#[derive(Debug, Clone)]
pub struct ServerState {
    pub server_id: Uuid,
    pub data_dir: PathBuf,
}

impl ServerState {
    pub fn load_from_env() -> anyhow::Result<Self> {
        let data_dir = std::env::var("CLOUDLEDGER_SERVER_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(DEFAULT_DATA_DIR));
        Self::load(data_dir)
    }

    pub fn load(data_dir: PathBuf) -> anyhow::Result<Self> {
        fs::create_dir_all(&data_dir)?;
        let server_id_path = data_dir.join(SERVER_ID_FILE);
        let server_id = load_or_create_server_id(&server_id_path)?;

        Ok(Self {
            server_id,
            data_dir,
        })
    }
}

fn load_or_create_server_id(path: &Path) -> anyhow::Result<Uuid> {
    if path.exists() {
        let raw = fs::read_to_string(path)?;
        return Ok(Uuid::parse_str(raw.trim())?);
    }

    let server_id = Uuid::new_v4();
    fs::write(path, server_id.to_string())?;
    Ok(server_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_id_is_stable_in_data_dir() {
        let data_dir = std::env::temp_dir().join(format!("cloudledger-server-{}", Uuid::new_v4()));

        let first = ServerState::load(data_dir.clone()).expect("first load");
        let second = ServerState::load(data_dir.clone()).expect("second load");

        assert_eq!(first.server_id, second.server_id);

        fs::remove_dir_all(data_dir).expect("remove temp dir");
    }
}
