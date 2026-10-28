use crate::error::AppError;
use std::io::Write;
use std::path::{Path, PathBuf};

pub fn get_home_dir() -> PathBuf {
    std::env::var_os("OBSERVATORY_TEST_HOME")
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .expect("home directory unavailable")
}
pub fn get_app_config_dir() -> PathBuf {
    std::env::var_os("OBSERVATORY_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| get_home_dir().join(".observatory"))
}
pub fn get_claude_config_dir() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| get_home_dir().join(".claude"))
}
pub fn atomic_write(path: &Path, data: &[u8]) -> Result<(), AppError> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Config("Invalid data path".into()))?;
    std::fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|e| AppError::io(parent, e))?;
    temporary
        .write_all(data)
        .map_err(|e| AppError::io(path, e))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|e| AppError::io(path, e))?;
    temporary
        .persist(path)
        .map_err(|e| AppError::io(path, e.error))?;
    Ok(())
}
pub fn is_wsl_path(path: &Path) -> bool {
    path.to_string_lossy().to_lowercase().starts_with(r"\\wsl")
}
