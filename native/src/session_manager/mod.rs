pub mod model;
pub mod providers;
pub use model::SessionMessage;
use serde::Serialize;
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMeta {
    pub provider_id: String,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_active_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume_command: Option<String>,
}

pub mod terminal {
    /// Turn an arbitrary string into a single shell argument for a resume command,
    /// passed through verbatim without any expansion.
    ///
    /// The original implementation wrapped it in double quotes: inside POSIX shell double
    /// quotes, `$(...)`, backticks, `$VAR` and backslashes are still interpreted, so a path
    /// containing those characters would make the copied command execute their content.
    /// A single quote has no special characters inside, only the `'` itself has to be
    /// escaped by ending and reopening the quoted string.
    #[cfg(not(windows))]
    pub fn shell_escape(value: &str) -> String {
        format!("'{}'", value.replace('\'', r"'\''"))
    }

    /// The Windows resume command is written for PowerShell (consistent with MCode's
    /// `$env:` style): a single-quoted string does not expand `$`, and a `'` inside it is
    /// written as `''`.
    #[cfg(windows)]
    pub fn shell_escape(value: &str) -> String {
        format!("'{}'", value.replace('\'', "''"))
    }

    #[cfg(test)]
    mod tests {
        use super::shell_escape;

        #[cfg(not(windows))]
        #[test]
        fn posix_escape_passes_arguments_literally() {
            for value in [
                "plain",
                "with space",
                "it's",
                "$(printf injected)",
                "`printf injected`",
                "$HOME and \\ backslash \"quote\"",
                "",
            ] {
                let output = std::process::Command::new("sh")
                    .arg("-c")
                    .arg(format!("printf '%s' {}", shell_escape(value)))
                    .output()
                    .unwrap();
                assert!(output.status.success(), "{value:?}");
                assert_eq!(String::from_utf8(output.stdout).unwrap(), value);
            }
        }

        #[cfg(windows)]
        #[test]
        fn powershell_escape_doubles_single_quotes() {
            assert_eq!(shell_escape("plain"), "'plain'");
            assert_eq!(shell_escape("C:\\it's $(x)"), "'C:\\it''s $(x)'");
        }
    }
}
