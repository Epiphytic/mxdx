//! mxdx-exec: thin process wrapper for the mxdx worker.
//!
//! Launched inside a tmux session by the worker. Spawns the actual command,
//! waits for it to exit, then immediately writes the exit code to a Unix
//! domain socket so the worker gets instant notification (no polling).
//!
//! Usage:
//!   mxdx-exec --notify <socket_path> -- <command> [args...]
//!
//! The worker creates a UDS listener at <socket_path> before launching tmux.
//! When the child process exits, mxdx-exec connects to the socket and sends
//! the exit code as a single line: "<exit_code>\n". If the child was killed
//! by a signal, the exit code is 128 + signal number (POSIX convention).

use std::os::unix::net::UnixStream;
use std::io::Write;
use std::process::Command;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // Parse: mxdx-exec --notify <socket> -- <cmd> [args...]
    let (notify_path, cmd_args) = parse_args(&args).unwrap_or_else(|e| {
        eprintln!("mxdx-exec: {e}");
        std::process::exit(126);
    });

    // Spawn the actual command
    let mut child = match Command::new(&cmd_args[0])
        .args(&cmd_args[1..])
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            eprintln!("mxdx-exec: failed to spawn '{}': {e}", cmd_args[0]);
            notify_exit(&notify_path, 127);
            std::process::exit(127);
        }
    };

    // Wait for the child to exit
    let exit_code = match child.wait() {
        Ok(status) => {
            if let Some(code) = status.code() {
                code
            } else {
                // Killed by signal — use 128 + signal (POSIX convention)
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt;
                    128 + status.signal().unwrap_or(0)
                }
                #[cfg(not(unix))]
                {
                    1
                }
            }
        }
        Err(e) => {
            eprintln!("mxdx-exec: wait failed: {e}");
            1
        }
    };

    // Notify the worker immediately via the Unix domain socket
    notify_exit(&notify_path, exit_code);

    std::process::exit(exit_code);
}

fn parse_args(args: &[String]) -> Result<(String, Vec<String>), String> {
    // Find --notify and -- separator
    let mut notify_path = None;
    let mut separator_idx = None;
    let mut i = 1; // skip argv[0]

    while i < args.len() {
        if args[i] == "--notify" {
            if i + 1 >= args.len() {
                return Err("--notify requires a socket path".into());
            }
            notify_path = Some(args[i + 1].clone());
            i += 2;
        } else if args[i] == "--" {
            separator_idx = Some(i);
            break;
        } else {
            i += 1;
        }
    }

    let notify = notify_path.ok_or("--notify <socket_path> is required")?;
    let sep = separator_idx.ok_or("-- separator before command is required")?;
    let cmd_args = args[sep + 1..].to_vec();

    if cmd_args.is_empty() {
        return Err("no command specified after --".into());
    }

    Ok((notify, cmd_args))
}

fn notify_exit(socket_path: &str, exit_code: i32) {
    match UnixStream::connect(socket_path) {
        Ok(mut stream) => {
            let _ = writeln!(stream, "{exit_code}");
        }
        Err(e) => {
            // Best-effort — the worker may have already cleaned up
            eprintln!("mxdx-exec: notify failed ({}): {e}", socket_path);
        }
    }
}
