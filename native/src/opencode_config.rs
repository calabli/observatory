use std::path::PathBuf;
pub fn get_opencode_db_path() -> PathBuf {
    // Honour the OPENCODE_DB environment variable override (empty strings are ignored)
    if let Ok(custom_path) = std::env::var("OPENCODE_DB") {
        if !custom_path.is_empty() {
            let path = PathBuf::from(&custom_path);
            if path.is_absolute() {
                return path;
            }
            // Relative paths are resolved against the data directory
            return get_opencode_data_dir().join(path);
        }
    }

    get_opencode_data_dir().join("opencode.db")
}

fn get_opencode_data_dir() -> PathBuf {
    // Honour XDG_DATA_HOME (per the XDG spec an empty string counts as unset)
    if let Ok(xdg_data) = std::env::var("XDG_DATA_HOME") {
        if !xdg_data.is_empty() {
            return PathBuf::from(xdg_data).join("opencode");
        }
    }

    // OpenCode uses xdg-basedir and does not follow the macOS/Windows platform conventions,
    // so the default lands in ~/.local/share/opencode on every platform
    crate::config::get_home_dir()
        .join(".local")
        .join("share")
        .join("opencode")
}
