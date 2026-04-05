use mxdx_types::events::session::SessionOutput;
use anyhow::Result;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

/// Reassemble output from a list of SessionOutput events, ordered by seq number
pub fn reassemble_output(mut outputs: Vec<SessionOutput>) -> Result<Vec<u8>> {
    outputs.sort_by_key(|o| o.seq);
    let mut data = Vec::new();
    for output in &outputs {
        let decoded = BASE64.decode(&output.data)?;
        data.extend_from_slice(&decoded);
    }
    Ok(data)
}

/// Reassemble and convert to string (lossy)
pub fn reassemble_output_string(outputs: Vec<SessionOutput>) -> Result<String> {
    let data = reassemble_output(outputs)?;
    Ok(String::from_utf8_lossy(&data).to_string())
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

    #[test]
    fn reassemble_output_orders_by_seq() {
        // Provide out of order
        let outputs = vec![
            make_output("world", 2),
            make_output("hello ", 1),
        ];
        let result = reassemble_output(outputs).unwrap();
        assert_eq!(result, b"hello world");
    }

    #[test]
    fn reassemble_output_handles_empty() {
        let result = reassemble_output(vec![]).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn reassemble_output_concatenates_correctly() {
        let outputs = vec![
            make_output("line1\n", 1),
            make_output("line2\n", 2),
            make_output("line3\n", 3),
        ];
        let result = reassemble_output_string(outputs).unwrap();
        assert_eq!(result, "line1\nline2\nline3\n");
    }

    #[test]
    fn reassemble_output_string_works() {
        let outputs = vec![make_output("hello", 1)];
        let result = reassemble_output_string(outputs).unwrap();
        assert_eq!(result, "hello");
    }
}
