use crate::config::{CapabilitiesConfig, CapabilityMode};
use std::fmt;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command as TokioCommand;

#[derive(Debug)]
pub struct ExecutorError(String);

impl fmt::Display for ExecutorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ExecutorError {}

#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedCommand {
    pub cmd: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
}

#[derive(Debug)]
pub struct CommandResult {
    pub exit_code: Option<i32>,
    pub stdout_lines: Vec<String>,
    pub stderr_lines: Vec<String>,
    pub total_seq: u64,
}

/// Execute a validated command, streaming stdout/stderr line by line.
/// Returns (exit_code, stdout_lines, stderr_lines) for now.
/// In full integration, this will send OutputEvents over Matrix.
pub async fn execute_command(
    validated: &ValidatedCommand,
) -> Result<CommandResult, ExecutorError> {
    let mut cmd = TokioCommand::new(&validated.cmd);
    cmd.args(&validated.args);
    if let Some(ref cwd) = validated.cwd {
        cmd.current_dir(cwd);
    }
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| ExecutorError(format!("spawn failed: {e}")))?;

    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();

    let mut stdout_reader = BufReader::new(stdout).lines();
    let mut stderr_reader = BufReader::new(stderr).lines();

    let mut stdout_lines: Vec<String> = Vec::new();
    let mut stderr_lines: Vec<String> = Vec::new();
    let mut seq: u64 = 0;

    loop {
        tokio::select! {
            line = stdout_reader.next_line() => {
                match line {
                    Ok(Some(l)) => {
                        stdout_lines.push(l);
                        seq += 1;
                    }
                    Ok(None) => {
                        while let Ok(Some(l)) = stderr_reader.next_line().await {
                            stderr_lines.push(l);
                            seq += 1;
                        }
                        break;
                    }
                    Err(e) => return Err(ExecutorError(format!("stdout read error: {e}"))),
                }
            }
            line = stderr_reader.next_line() => {
                match line {
                    Ok(Some(l)) => {
                        stderr_lines.push(l);
                        seq += 1;
                    }
                    Ok(None) => {
                        while let Ok(Some(l)) = stdout_reader.next_line().await {
                            stdout_lines.push(l);
                            seq += 1;
                        }
                        break;
                    }
                    Err(e) => return Err(ExecutorError(format!("stderr read error: {e}"))),
                }
            }
        }
    }

    let status = child
        .wait()
        .await
        .map_err(|e| ExecutorError(format!("wait failed: {e}")))?;

    Ok(CommandResult {
        exit_code: status.code(),
        stdout_lines,
        stderr_lines,
        total_seq: seq,
    })
}

/// Normalize a path by resolving `.` and `..` components without touching the filesystem.
fn normalize_path(path: &str) -> String {
    let mut components: Vec<&str> = Vec::new();
    let is_absolute = path.starts_with('/');

    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if is_absolute {
                    components.pop();
                } else if components.last().map_or(true, |c| *c == "..") {
                    components.push("..");
                } else {
                    components.pop();
                }
            }
            other => components.push(other),
        }
    }

    if is_absolute {
        format!("/{}", components.join("/"))
    } else {
        components.join("/")
    }
}

/// Validates dangerous argument patterns for specific commands.
fn validate_args(cmd: &str, args: &[&str]) -> Result<(), ExecutorError> {
    match cmd {
        "git" => {
            for (i, arg) in args.iter().enumerate() {
                if *arg == "-c" || *arg == "--config" {
                    return Err(ExecutorError(format!(
                        "argument not permitted: git {} is blocked",
                        arg
                    )));
                }
                if *arg == "submodule" {
                    if let Some(next) = args.get(i + 1) {
                        if *next == "foreach" {
                            return Err(ExecutorError(
                                "argument not permitted: git submodule foreach is blocked"
                                    .to_string(),
                            ));
                        }
                    }
                }
            }
        }
        "docker" => {
            for (i, arg) in args.iter().enumerate() {
                if *arg == "compose" {
                    for subsequent in &args[i + 1..] {
                        if *subsequent == "-f" || *subsequent == "--file" {
                            return Err(ExecutorError(
                                "argument not permitted: docker compose -f/--file is blocked"
                                    .to_string(),
                            ));
                        }
                    }
                }
            }
        }
        "env" => {
            return Err(ExecutorError(
                "argument not permitted: env command is blocked to prevent prefix injection"
                    .to_string(),
            ));
        }
        _ => {}
    }
    Ok(())
}

pub fn validate_command(
    config: &CapabilitiesConfig,
    cmd: &str,
    args: &[&str],
    cwd: Option<&str>,
) -> Result<ValidatedCommand, ExecutorError> {
    // 1. Allowlist check
    if config.mode == CapabilityMode::Allowlist && !config.allowed_commands.contains(&cmd.to_string())
    {
        return Err(ExecutorError(format!(
            "command '{}' not permitted",
            cmd
        )));
    }

    // 2. cwd validation
    let resolved_cwd = if let Some(cwd_str) = cwd {
        let normalized = normalize_path(cwd_str);
        let permitted = config
            .allowed_cwd_prefixes
            .iter()
            .any(|prefix| normalized.starts_with(prefix));
        if !permitted {
            return Err(ExecutorError(format!(
                "cwd not permitted: {}",
                normalized
            )));
        }
        Some(normalized)
    } else {
        None
    };

    // 3. Argument injection checks
    validate_args(cmd, args)?;

    Ok(ValidatedCommand {
        cmd: cmd.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
        cwd: resolved_cwd,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::*;

    fn test_config_with_allowlist(cmds: &[&str]) -> CapabilitiesConfig {
        CapabilitiesConfig {
            mode: CapabilityMode::Allowlist,
            allowed_commands: cmds.iter().map(|s| s.to_string()).collect(),
            allowed_cwd_prefixes: vec!["/workspace".to_string()],
            max_sessions: 10,
        }
    }

    fn test_config_with_cwd_prefixes(prefixes: &[&str]) -> CapabilitiesConfig {
        CapabilitiesConfig {
            mode: CapabilityMode::Allowlist,
            allowed_commands: vec!["cargo".to_string(), "git".to_string()],
            allowed_cwd_prefixes: prefixes.iter().map(|s| s.to_string()).collect(),
            max_sessions: 10,
        }
    }

    #[test]
    fn command_on_allowlist_is_permitted() {
        let config = test_config_with_allowlist(&["cargo", "git"]);
        let result = validate_command(&config, "cargo", &["build"], Some("/workspace"));
        assert!(result.is_ok());
    }

    #[test]
    fn command_not_on_allowlist_is_rejected() {
        let config = test_config_with_allowlist(&["cargo"]);
        let result = validate_command(&config, "rm", &["-rf", "/"], Some("/workspace"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not permitted"));
    }

    // mxdx-71v: cwd validation
    #[test]
    fn test_security_cwd_outside_prefix_is_rejected() {
        let config = test_config_with_cwd_prefixes(&["/workspace"]);
        let result = validate_command(&config, "cargo", &["build"], Some("/etc"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cwd not permitted"));
    }

    #[test]
    fn test_security_cwd_traversal_rejected() {
        let config = test_config_with_cwd_prefixes(&["/workspace"]);
        let result = validate_command(&config, "cargo", &["build"], Some("/workspace/../../etc"));
        assert!(result.is_err());
    }

    #[test]
    fn test_security_cwd_none_uses_default() {
        let config = test_config_with_cwd_prefixes(&["/workspace"]);
        let result = validate_command(&config, "cargo", &["build"], None);
        assert!(result.is_ok());
    }

    // mxdx-jjf: argument injection
    #[test]
    fn test_security_git_dash_c_blocked() {
        let config = test_config_with_allowlist(&["git"]);
        let result = validate_command(
            &config,
            "git",
            &["-c", "core.pager=evil", "log"],
            Some("/workspace"),
        );
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("argument not permitted"));
    }

    #[test]
    fn test_security_git_submodule_foreach_blocked() {
        let config = test_config_with_allowlist(&["git"]);
        let result = validate_command(
            &config,
            "git",
            &["submodule", "foreach", "evil"],
            Some("/workspace"),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_security_docker_compose_dash_f_blocked() {
        let config = test_config_with_allowlist(&["docker"]);
        let result = validate_command(
            &config,
            "docker",
            &["compose", "-f", "/tmp/evil.yml", "up"],
            Some("/workspace"),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_security_env_prefix_injection_blocked() {
        let config = test_config_with_allowlist(&["cargo"]);
        let result = validate_command(
            &config,
            "env",
            &["MALICIOUS=true", "cargo", "build"],
            Some("/workspace"),
        );
        assert!(result.is_err());
    }
}
