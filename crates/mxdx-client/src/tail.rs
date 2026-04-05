use mxdx_types::events::session::{SessionOutput, SessionResult, SessionStatus};
use anyhow::Result;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

/// Decode a SessionOutput event's data (base64) to bytes
pub fn decode_output(output: &SessionOutput) -> Result<Vec<u8>> {
    Ok(BASE64.decode(&output.data)?)
}

/// Format output for display (decode base64 data to string)
pub fn format_output(output: &SessionOutput) -> Result<String> {
    let bytes = decode_output(output)?;
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

/// Check if a session result indicates the tail should stop
pub fn is_terminal_result(_result: &SessionResult) -> bool {
    true // Any result ends the tail
}

/// Render a session result for display
pub fn format_result(result: &SessionResult) -> String {
    let status = match result.status {
        SessionStatus::Success => "success",
        SessionStatus::Failed => "failed",
        SessionStatus::Timeout => "timeout",
        SessionStatus::Cancelled => "cancelled",
    };
    format!(
        "Session {} {}: exit_code={:?}, duration={}s",
        result.session_uuid, status, result.exit_code, result.duration_seconds
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxdx_types::events::session::OutputStream;

    fn make_output(data: &str, seq: u64) -> SessionOutput {
        SessionOutput {
            session_uuid: "s-1".into(),
            worker_id: "@w:example.com".into(),
            stream: OutputStream::Stdout,
            data: BASE64.encode(data.as_bytes()),
            seq,
            timestamp: 1000 + seq,
        }
    }

    fn make_result(status: SessionStatus, exit_code: Option<i32>, duration: u64) -> SessionResult {
        SessionResult {
            session_uuid: "s-1".into(),
            worker_id: "@w:example.com".into(),
            status,
            exit_code,
            duration_seconds: duration,
            tail: None,
        }
    }

    #[test]
    fn decode_output_correctly_decodes_base64() {
        let output = make_output("hello world", 1);
        let decoded = decode_output(&output).unwrap();
        assert_eq!(decoded, b"hello world");
    }

    #[test]
    fn format_output_works() {
        let output = make_output("test output\n", 1);
        let formatted = format_output(&output).unwrap();
        assert_eq!(formatted, "test output\n");
    }

    #[test]
    fn format_output_handles_non_utf8() {
        let raw_bytes: Vec<u8> = vec![0xff, 0xfe, 0x48, 0x65, 0x6c, 0x6c, 0x6f];
        let output = SessionOutput {
            session_uuid: "s-1".into(),
            worker_id: "@w:example.com".into(),
            stream: OutputStream::Stdout,
            data: BASE64.encode(&raw_bytes),
            seq: 1,
            timestamp: 1000,
        };
        let formatted = format_output(&output).unwrap();
        assert!(formatted.contains("Hello"));
    }

    #[test]
    fn is_terminal_result_always_true() {
        let result = make_result(SessionStatus::Success, Some(0), 10);
        assert!(is_terminal_result(&result));

        let result = make_result(SessionStatus::Failed, Some(1), 5);
        assert!(is_terminal_result(&result));
    }

    #[test]
    fn format_result_success() {
        let result = make_result(SessionStatus::Success, Some(0), 120);
        let formatted = format_result(&result);
        assert!(formatted.contains("s-1"));
        assert!(formatted.contains("success"));
        assert!(formatted.contains("120s"));
        assert!(formatted.contains("Some(0)"));
    }

    #[test]
    fn format_result_failed() {
        let result = make_result(SessionStatus::Failed, Some(1), 5);
        let formatted = format_result(&result);
        assert!(formatted.contains("failed"));
        assert!(formatted.contains("Some(1)"));
    }

    #[test]
    fn format_result_timeout() {
        let result = make_result(SessionStatus::Timeout, None, 3600);
        let formatted = format_result(&result);
        assert!(formatted.contains("timeout"));
        assert!(formatted.contains("None"));
    }

    #[test]
    fn format_result_cancelled() {
        let result = make_result(SessionStatus::Cancelled, None, 0);
        let formatted = format_result(&result);
        assert!(formatted.contains("cancelled"));
    }
}
