use crate::config::get_home_dir;
use std::path::PathBuf;
pub(crate) fn data_dir() -> PathBuf {
    std::env::var_os("KIMI_CODE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| get_home_dir().join(".kimi-code"))
}
