use std::path::PathBuf;

pub const APP_NAME: &str = "workforest";

pub fn config_dir() -> PathBuf {
    let project_dirs = directories::ProjectDirs::from("", "", APP_NAME)
        .expect("project dirs available");
    project_dirs.config_dir().to_path_buf()
}
