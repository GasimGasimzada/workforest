use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const APP_NAME: &str = "workforest";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoConfig {
    pub name: String,
    pub path: PathBuf,
    pub tools: Vec<String>,
    pub default_tool: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct RepoConfigFile {
    #[serde(default)]
    pub repos: Vec<RepoConfig>,
}

pub fn config_dir() -> PathBuf {
    let project_dirs =
        directories::ProjectDirs::from("", "", APP_NAME).expect("project dirs available");
    project_dirs.config_dir().to_path_buf()
}

pub fn data_dir() -> PathBuf {
    let project_dirs =
        directories::ProjectDirs::from("", "", APP_NAME).expect("project dirs available");
    project_dirs.data_dir().to_path_buf()
}

pub fn repos_config_path() -> PathBuf {
    config_dir().join("repos.toml")
}
