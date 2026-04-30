// Performance entry writer for E2E/integration tests.
//
// Emits JSONL records to TEST_PERF_OUTPUT, matching the npm writePerfEntry()
// schema so both runtimes feed a single unified performance stream.
//
// If TEST_PERF_OUTPUT is unset, write_perf_entry() is a no-op.

use std::fs::OpenOptions;
use std::io::Write as _;

use serde::Serialize;

/// Performance entry — schema must stay identical to npm's writePerfEntry().
#[derive(Debug, Clone, Serialize)]
pub struct PerfEntry {
    pub suite: String,
    pub transport: String,
    pub runtime: String,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rss_max: Option<u64>,
}

/// Write a single JSONL performance entry to TEST_PERF_OUTPUT.
///
/// No-ops silently if TEST_PERF_OUTPUT is unset or the file cannot be opened.
pub fn write_perf_entry(entry: &PerfEntry) -> anyhow::Result<()> {
    let path = match std::env::var("TEST_PERF_OUTPUT") {
        Ok(p) if !p.is_empty() => p,
        _ => return Ok(()),
    };

    let line = serde_json::to_string(entry)?;

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| anyhow::anyhow!("TEST_PERF_OUTPUT open error ({path}): {e}"))?;

    writeln!(file, "{line}")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead as _;

    #[test]
    fn writes_jsonl_to_output_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perf.jsonl");
        std::env::set_var("TEST_PERF_OUTPUT", path.to_str().unwrap());

        let entry = PerfEntry {
            suite: "test-suite".into(),
            transport: "same-hs".into(),
            runtime: "rust".into(),
            duration_ms: 123,
            rss_max: Some(65536),
        };
        write_perf_entry(&entry).unwrap();

        let file = std::fs::File::open(&path).unwrap();
        let lines: Vec<String> = std::io::BufReader::new(file)
            .lines()
            .map(|l| l.unwrap())
            .collect();
        assert_eq!(lines.len(), 1);
        let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(v["suite"], "test-suite");
        assert_eq!(v["transport"], "same-hs");
        assert_eq!(v["runtime"], "rust");
        assert_eq!(v["duration_ms"], 123);
        assert_eq!(v["rss_max"], 65536);

        std::env::remove_var("TEST_PERF_OUTPUT");
    }

    #[test]
    fn noop_when_env_unset() {
        std::env::remove_var("TEST_PERF_OUTPUT");
        let entry = PerfEntry {
            suite: "noop".into(),
            transport: "same-hs".into(),
            runtime: "rust".into(),
            duration_ms: 0,
            rss_max: None,
        };
        // Must not panic or error
        write_perf_entry(&entry).unwrap();
    }

    #[test]
    fn rss_max_omitted_when_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perf2.jsonl");
        std::env::set_var("TEST_PERF_OUTPUT", path.to_str().unwrap());

        let entry = PerfEntry {
            suite: "s".into(),
            transport: "t".into(),
            runtime: "rust".into(),
            duration_ms: 1,
            rss_max: None,
        };
        write_perf_entry(&entry).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
        assert!(
            v.get("rss_max").is_none(),
            "rss_max should be omitted when None"
        );

        std::env::remove_var("TEST_PERF_OUTPUT");
    }
}
