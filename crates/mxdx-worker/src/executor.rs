use std::collections::HashMap;
use std::path::Path;

use anyhow::{bail, Result};

/// A command that has passed all validation checks and is safe to execute.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedCommand {
    pub bin: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub cwd: Option<String>,
}

/// Characters that could enable shell injection if passed unsanitized.
const SHELL_METACHARACTERS: &[char] = &[
    ';', '|', '&', '$', '`', '(', ')', '{', '}', '<', '>', '\n', '\r',
];

/// Validate a binary name/path. Must be non-empty, contain no shell
/// metacharacters, and be a single token (no spaces unless it resolves
/// to an existing filesystem path).
pub fn validate_bin(bin: &str) -> Result<()> {
    if bin.is_empty() {
        bail!("bin must not be empty");
    }
    if bin.contains(SHELL_METACHARACTERS) {
        bail!("bin contains shell metacharacters: {}", bin);
    }
    // Must be a single token (no spaces unless it's a valid path on disk)
    if bin.contains(' ') && !Path::new(bin).exists() {
        bail!("bin contains spaces and is not a valid path: {}", bin);
    }
    Ok(())
}

/// Validate command arguments. No argument may contain a null byte.
pub fn validate_args(args: &[String]) -> Result<()> {
    for (i, arg) in args.iter().enumerate() {
        if arg.contains('\0') {
            bail!("arg[{}] contains null byte", i);
        }
    }
    Ok(())
}

/// Validate a working directory. Must be an absolute path, must not
/// contain `..` traversal components, and must exist as a directory.
pub fn validate_cwd(cwd: &str) -> Result<()> {
    if !cwd.starts_with('/') {
        bail!("cwd must be an absolute path: {}", cwd);
    }
    if cwd.contains("..") {
        bail!("cwd must not contain path traversal (..): {}", cwd);
    }
    if !Path::new(cwd).is_dir() {
        bail!("cwd does not exist or is not a directory: {}", cwd);
    }
    Ok(())
}

/// Check whether a string is a valid environment variable key.
/// Must match the pattern `[A-Z_][A-Z0-9_]*`.
fn is_valid_env_key(key: &str) -> bool {
    if key.is_empty() {
        return false;
    }
    let mut chars = key.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_uppercase() && first != '_' {
        return false;
    }
    chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// Validate all environment variable keys. Each key must match
/// `[A-Z_][A-Z0-9_]*`.
pub fn validate_env(env: &HashMap<String, String>) -> Result<()> {
    for key in env.keys() {
        if !is_valid_env_key(key) {
            bail!("invalid env key (must match [A-Z_][A-Z0-9_]*): {}", key);
        }
    }
    Ok(())
}

/// Validate a binary name against the command allowlist.
/// Empty list = deny all (strict security default).
/// Non-empty list = exact match required.
pub fn validate_allowlist(bin: &str, allowed: &[String]) -> Result<()> {
    if allowed.is_empty() {
        bail!("command allowlist is empty — all commands are denied");
    }
    if !allowed.iter().any(|a| a == bin) {
        bail!(
            "command '{}' is not in the allowlist: {:?}",
            bin,
            allowed
        );
    }
    Ok(())
}

/// Validate a working directory against the CWD allowlist.
/// Uses prefix matching: cwd must start with one of the allowed prefixes.
pub fn validate_cwd_allowlist(cwd: &str, allowed: &[String]) -> Result<()> {
    if allowed.is_empty() {
        bail!("cwd allowlist is empty — all directories are denied");
    }
    if !allowed.iter().any(|a| cwd.starts_with(a.as_str())) {
        bail!(
            "cwd '{}' is not under any allowed directory: {:?}",
            cwd,
            allowed
        );
    }
    Ok(())
}

/// Validate all parts of a command and return a `ValidatedCommand` if
/// everything passes. This is the primary entry point for callers.
///
/// `allowed_commands` and `allowed_cwd` are security gates checked before
/// any other validation. Pass empty slices to use the strict deny-all default.
pub fn validate_command(
    bin: &str,
    args: &[String],
    env: Option<&HashMap<String, String>>,
    cwd: Option<&str>,
    allowed_commands: &[String],
    allowed_cwd: &[String],
) -> Result<ValidatedCommand> {
    // Security gates first
    validate_allowlist(bin, allowed_commands)?;
    if let Some(cwd) = cwd {
        validate_cwd_allowlist(cwd, allowed_cwd)?;
    }

    validate_bin(bin)?;
    validate_args(args)?;
    if let Some(env) = env {
        validate_env(env)?;
    }
    if let Some(cwd) = cwd {
        validate_cwd(cwd)?;
    }
    Ok(ValidatedCommand {
        bin: bin.to_string(),
        args: args.to_vec(),
        env: env.cloned().unwrap_or_default(),
        cwd: cwd.map(|s| s.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- validate_bin ---

    #[test]
    fn validate_bin_simple_command() {
        assert!(validate_bin("echo").is_ok());
    }

    #[test]
    fn validate_bin_rejects_shell_injection() {
        let result = validate_bin("echo; rm -rf /");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("shell metacharacters"));
    }

    #[test]
    fn validate_bin_rejects_empty() {
        let result = validate_bin("");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty"));
    }

    #[test]
    fn validate_bin_rejects_pipe() {
        let result = validate_bin("echo|cat");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("shell metacharacters"));
    }

    #[test]
    fn validate_bin_rejects_ampersand() {
        assert!(validate_bin("sleep 5 &").is_err());
    }

    #[test]
    fn validate_bin_rejects_dollar_sign() {
        assert!(validate_bin("$HOME/bin/evil").is_err());
    }

    #[test]
    fn validate_bin_rejects_backtick() {
        assert!(validate_bin("`whoami`").is_err());
    }

    #[test]
    fn validate_bin_rejects_parens() {
        assert!(validate_bin("(evil)").is_err());
    }

    #[test]
    fn validate_bin_rejects_angle_brackets() {
        assert!(validate_bin("cat</etc/passwd").is_err());
        assert!(validate_bin("echo>evil").is_err());
    }

    #[test]
    fn validate_bin_rejects_newlines() {
        assert!(validate_bin("echo\nrm -rf /").is_err());
        assert!(validate_bin("echo\rrm -rf /").is_err());
    }

    #[test]
    fn validate_bin_accepts_absolute_path() {
        // /usr/bin/env should exist on any Unix system
        assert!(validate_bin("/usr/bin/env").is_ok());
    }

    // --- validate_args ---

    #[test]
    fn validate_args_simple() {
        assert!(validate_args(&["hello".into()]).is_ok());
    }

    #[test]
    fn validate_args_rejects_null_byte() {
        let result = validate_args(&["hello\0world".into()]);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("null byte"));
    }

    #[test]
    fn validate_args_accepts_empty_list() {
        assert!(validate_args(&[]).is_ok());
    }

    #[test]
    fn validate_args_accepts_special_chars() {
        // Arguments are allowed to have special chars — it's the bin that
        // must be sanitized. Args go through execvp, not a shell.
        assert!(validate_args(&["--flag=value".into(), "hello world".into()]).is_ok());
    }

    // --- validate_cwd ---

    #[test]
    fn validate_cwd_tmp() {
        assert!(validate_cwd("/tmp").is_ok());
    }

    #[test]
    fn validate_cwd_rejects_relative_path() {
        let result = validate_cwd("relative/path");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("absolute path"));
    }

    #[test]
    fn validate_cwd_rejects_traversal() {
        let result = validate_cwd("/tmp/../etc");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("path traversal"));
    }

    #[test]
    fn validate_cwd_rejects_nonexistent() {
        let result = validate_cwd("/nonexistent_dir_abc123");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("does not exist or is not a directory"));
    }

    // --- validate_env ---

    #[test]
    fn validate_env_valid_keys() {
        let mut env = HashMap::new();
        env.insert("PATH".into(), "/usr/bin".into());
        env.insert("HOME".into(), "/root".into());
        env.insert("MY_VAR_123".into(), "value".into());
        env.insert("_PRIVATE".into(), "value".into());
        assert!(validate_env(&env).is_ok());
    }

    #[test]
    fn validate_env_rejects_lowercase() {
        let mut env = HashMap::new();
        env.insert("lowercase".into(), "val".into());
        let result = validate_env(&env);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid env key"));
    }

    #[test]
    fn validate_env_rejects_leading_digit() {
        let mut env = HashMap::new();
        env.insert("123ABC".into(), "val".into());
        let result = validate_env(&env);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid env key"));
    }

    #[test]
    fn validate_env_rejects_empty_key() {
        let mut env = HashMap::new();
        env.insert("".into(), "val".into());
        assert!(validate_env(&env).is_err());
    }

    #[test]
    fn validate_env_accepts_empty_map() {
        let env = HashMap::new();
        assert!(validate_env(&env).is_ok());
    }

    // --- is_valid_env_key ---

    #[test]
    fn env_key_validation_edge_cases() {
        assert!(is_valid_env_key("A"));
        assert!(is_valid_env_key("_"));
        assert!(is_valid_env_key("_A1"));
        assert!(!is_valid_env_key(""));
        assert!(!is_valid_env_key("a"));
        assert!(!is_valid_env_key("1A"));
        assert!(!is_valid_env_key("A-B"));
        assert!(!is_valid_env_key("A.B"));
        assert!(!is_valid_env_key("A B"));
    }

    // --- validate_command (full integration) ---

    // Helper: create a permissive allowlist for tests that don't focus on allowlists
    fn allow_all() -> Vec<String> {
        vec!["echo".into(), "ls".into(), "cat".into(), "sleep".into()]
    }

    fn allow_cwd_all() -> Vec<String> {
        vec!["/".into()]
    }

    #[test]
    fn validate_command_happy_path() {
        let mut env = HashMap::new();
        env.insert("PATH".into(), "/usr/bin".into());
        let allowed = vec!["echo".into()];
        let allowed_cwd = vec!["/tmp".into()];

        let result = validate_command(
            "echo",
            &["hello".into()],
            Some(&env),
            Some("/tmp"),
            &allowed,
            &allowed_cwd,
        );
        assert!(result.is_ok());

        let cmd = result.unwrap();
        assert_eq!(cmd.bin, "echo");
        assert_eq!(cmd.args, vec!["hello"]);
        assert_eq!(cmd.env.get("PATH"), Some(&"/usr/bin".to_string()));
        assert_eq!(cmd.cwd, Some("/tmp".to_string()));
    }

    #[test]
    fn validate_command_no_env_no_cwd() {
        let allowed = vec!["ls".into()];
        let result = validate_command("ls", &["-la".into()], None, None, &allowed, &[]);
        assert!(result.is_ok());

        let cmd = result.unwrap();
        assert!(cmd.env.is_empty());
        assert!(cmd.cwd.is_none());
    }

    #[test]
    fn validate_command_invalid_bin() {
        let result = validate_command(
            "echo; rm -rf /",
            &[],
            None,
            None,
            &allow_all(),
            &allow_cwd_all(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn validate_command_invalid_args() {
        let result = validate_command(
            "echo",
            &["hello\0world".into()],
            None,
            None,
            &allow_all(),
            &allow_cwd_all(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn validate_command_invalid_env() {
        let mut env = HashMap::new();
        env.insert("bad-key".into(), "val".into());
        let result = validate_command(
            "echo",
            &[],
            Some(&env),
            None,
            &allow_all(),
            &allow_cwd_all(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn validate_command_invalid_cwd() {
        let result = validate_command(
            "echo",
            &[],
            None,
            Some("relative"),
            &allow_all(),
            &allow_cwd_all(),
        );
        assert!(result.is_err());
    }

    // --- validate_allowlist ---

    #[test]
    fn allowlist_empty_denies_all() {
        let result = validate_allowlist("echo", &[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty"));
    }

    #[test]
    fn allowlist_permits_listed_command() {
        let allowed = vec!["echo".into(), "ls".into()];
        assert!(validate_allowlist("echo", &allowed).is_ok());
        assert!(validate_allowlist("ls", &allowed).is_ok());
    }

    #[test]
    fn allowlist_rejects_unlisted_command() {
        let allowed = vec!["echo".into()];
        let result = validate_allowlist("rm", &allowed);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not in the allowlist"));
    }

    #[test]
    fn allowlist_exact_match_only() {
        let allowed = vec!["echo".into()];
        // "echo2" should not match "echo"
        assert!(validate_allowlist("echo2", &allowed).is_err());
    }

    // --- validate_cwd_allowlist ---

    #[test]
    fn cwd_allowlist_empty_denies_all() {
        let result = validate_cwd_allowlist("/tmp", &[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty"));
    }

    #[test]
    fn cwd_allowlist_prefix_match() {
        let allowed = vec!["/tmp".into(), "/home/worker".into()];
        assert!(validate_cwd_allowlist("/tmp", &allowed).is_ok());
        assert!(validate_cwd_allowlist("/tmp/subdir", &allowed).is_ok());
        assert!(validate_cwd_allowlist("/home/worker/projects", &allowed).is_ok());
    }

    #[test]
    fn cwd_allowlist_rejects_outside_prefix() {
        let allowed = vec!["/tmp".into()];
        let result = validate_cwd_allowlist("/etc", &allowed);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not under any allowed"));
    }

    #[test]
    fn validate_command_rejects_unlisted_bin() {
        let allowed_cmds = vec!["ls".into()];
        let allowed_cwd = vec!["/tmp".into()];
        let result = validate_command("rm", &[], None, None, &allowed_cmds, &allowed_cwd);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not in the allowlist"));
    }

    #[test]
    fn validate_command_rejects_unlisted_cwd() {
        let allowed_cmds = vec!["echo".into()];
        let allowed_cwd = vec!["/tmp".into()];
        let result = validate_command(
            "echo",
            &[],
            None,
            Some("/etc"),
            &allowed_cmds,
            &allowed_cwd,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not under any allowed"));
    }
}
