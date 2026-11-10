#![allow(dead_code)]
pub mod activity;
pub mod api;
pub mod codex_rollout_file;
pub mod config;
pub mod database;
pub mod error;
pub mod kimi_config;
pub mod kimi_session;
pub mod lan_usage;
pub mod mcode_config;
pub mod network_overview;
pub mod opencode_config;
pub mod proxy;
pub mod scan_jobs;
pub mod services;
pub mod session_manager;
pub mod subscriptions;

pub mod codex_config {
    pub fn get_codex_config_dir() -> std::path::PathBuf {
        std::env::var_os("CODEX_HOME")
            .filter(|v| !v.is_empty())
            .map(Into::into)
            .unwrap_or_else(|| crate::config::get_home_dir().join(".codex"))
    }
}
pub mod gemini_config {
    pub fn get_gemini_dir() -> std::path::PathBuf {
        std::env::var_os("GEMINI_CLI_HOME")
            .filter(|v| !v.is_empty())
            .map(Into::into)
            .unwrap_or_else(|| crate::config::get_home_dir().join(".gemini"))
    }
}
pub mod grok_config {
    pub fn get_grok_config_dir() -> std::path::PathBuf {
        std::env::var_os("GROK_HOME")
            .filter(|v| !v.is_empty())
            .map(Into::into)
            .unwrap_or_else(|| crate::config::get_home_dir().join(".grok"))
    }
}
pub mod claude_desktop_config {
    pub const ONE_M_CONTEXT_MARKER: &str = "[1m]";
}
pub mod usage_events {
    use std::sync::atomic::{AtomicU64, Ordering};
    static REVISION: AtomicU64 = AtomicU64::new(0);
    pub fn notify_log_recorded() {
        REVISION.fetch_add(1, Ordering::Relaxed);
        #[cfg(test)]
        TEST_NOTIFY_COUNT.with(|count| count.set(count.get() + 1));
    }
    pub fn revision() -> u64 {
        REVISION.load(Ordering::Relaxed)
    }
    // Counted per thread: every test runs on its own thread, so concurrent tests do not interfere.
    #[cfg(test)]
    thread_local! {
        static TEST_NOTIFY_COUNT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    }
    /// Take and reset the notification count for the current thread (tests only).
    #[cfg(test)]
    pub fn take_test_notify_count() -> u64 {
        TEST_NOTIFY_COUNT.with(|count| count.replace(0))
    }
}
pub mod agy_config {
    use std::path::{Path, PathBuf};
    pub fn agy_dir() -> PathBuf {
        crate::gemini_config::get_gemini_dir().join("antigravity-cli")
    }
    pub fn key_profiles_root() -> PathBuf {
        crate::config::get_home_dir().join(".cc-switch/agy")
    }
    pub fn resume_command(_gemini_dir: Option<&Path>, id: &str) -> String {
        format!("agy --resume {id}")
    }
}
pub mod pi_config {
    use crate::error::AppError;
    use std::path::PathBuf;
    #[derive(Default, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct PiNativeDefaults {
        pub default_provider: Option<String>,
        pub default_model: Option<String>,
        pub session_dir: Option<String>,
    }
    pub fn get_pi_agent_dir() -> Result<PathBuf, AppError> {
        let path = std::env::var_os("PI_CODING_AGENT_DIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| crate::config::get_home_dir().join(".pi/agent"));
        if !path.is_absolute() {
            return Err(AppError::InvalidInput(
                "PI_CODING_AGENT_DIR must be absolute".into(),
            ));
        }
        Ok(path)
    }
    pub fn read_pi_native_defaults() -> Result<PiNativeDefaults, AppError> {
        let path = get_pi_agent_dir()?.join("settings.json");
        match std::fs::read_to_string(&path) {
            Ok(text) => json5::from_str(&text)
                .map_err(|e| AppError::Config(format!("Invalid Pi settings: {e}"))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(PiNativeDefaults::default()),
            Err(e) => Err(AppError::io(path, e)),
        }
    }

    #[cfg(test)]
    pub mod test_support {
        use std::ffi::OsString;
        use std::path::Path;

        /// Point `PI_CODING_AGENT_DIR` at the given directory for the lifetime of the scope, restoring the previous value on exit.
        /// It mutates a process-wide environment variable, so tests using it must be marked `#[serial_test::serial]`.
        pub struct TestAgentDir {
            previous: Option<OsString>,
        }
        impl TestAgentDir {
            pub fn at(dir: &Path) -> Self {
                std::fs::create_dir_all(dir).expect("create Pi agent dir");
                let previous = std::env::var_os("PI_CODING_AGENT_DIR");
                std::env::set_var("PI_CODING_AGENT_DIR", dir);
                Self { previous }
            }
        }
        impl Drop for TestAgentDir {
            fn drop(&mut self) {
                match self.previous.take() {
                    Some(value) => std::env::set_var("PI_CODING_AGENT_DIR", value),
                    None => std::env::remove_var("PI_CODING_AGENT_DIR"),
                }
            }
        }
    }
}
