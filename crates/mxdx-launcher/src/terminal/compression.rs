use base64::{engine::general_purpose::STANDARD, Engine};
use flate2::{read::ZlibDecoder, write::ZlibEncoder, Compression};
use std::io::{Read, Write};

const COMPRESSION_THRESHOLD: usize = 32;
const CHUNK_SIZE: usize = 8192;

pub fn compress_encode(data: &[u8]) -> (String, String) {
    if data.len() < COMPRESSION_THRESHOLD {
        let encoded = STANDARD.encode(data);
        (encoded, "raw+base64".to_string())
    } else {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data).expect("zlib compression failed");
        let compressed = encoder.finish().expect("zlib finish failed");
        let encoded = STANDARD.encode(&compressed);
        (encoded, "zlib+base64".to_string())
    }
}

pub fn decode_decompress_bounded(
    encoded: &str,
    encoding: &str,
    max_bytes: usize,
) -> Result<Vec<u8>, anyhow::Error> {
    match encoding {
        "raw+base64" => {
            let decoded = STANDARD.decode(encoded)?;
            if decoded.len() > max_bytes {
                anyhow::bail!(
                    "decoded size {} exceeds max_bytes {}",
                    decoded.len(),
                    max_bytes
                );
            }
            Ok(decoded)
        }
        "zlib+base64" => {
            let compressed = STANDARD.decode(encoded)?;
            let mut decoder = ZlibDecoder::new(&compressed[..]);
            let mut output = Vec::new();
            let mut chunk = [0u8; CHUNK_SIZE];
            let mut total = 0usize;

            loop {
                let n = decoder.read(&mut chunk)?;
                if n == 0 {
                    break;
                }
                total += n;
                if total > max_bytes {
                    anyhow::bail!("decompressed size exceeds max_bytes {}", max_bytes);
                }
                output.extend_from_slice(&chunk[..n]);
            }

            Ok(output)
        }
        _ => anyhow::bail!("unknown encoding: {}", encoding),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn small_payload_uses_raw_base64() {
        let data = b"hi";
        let (encoded, encoding) = compress_encode(data);
        assert_eq!(encoding, "raw+base64");
        let decoded = decode_decompress_bounded(&encoded, &encoding, 1_048_576).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn large_payload_uses_zlib_base64() {
        let data = vec![b'x'; 100];
        let (encoded, encoding) = compress_encode(&data);
        assert_eq!(encoding, "zlib+base64");
        let decoded = decode_decompress_bounded(&encoded, &encoding, 1_048_576).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn boundary_at_32_bytes_uses_zlib() {
        let data = vec![b'a'; 32];
        let (_, encoding) = compress_encode(&data);
        assert_eq!(encoding, "zlib+base64");
    }

    #[test]
    fn test_security_zlib_bomb_rejected_before_pty_write() {
        let bomb_data = vec![b'a'; 2 * 1024 * 1024];
        let (encoded, encoding) = compress_encode(&bomb_data);
        let result = decode_decompress_bounded(&encoded, &encoding, 1_048_576);
        assert!(result.is_err(), "zlib bomb should be rejected");
    }

    #[test]
    fn test_security_decompression_streams_and_fails_fast() {
        let bomb_data = vec![b'a'; 5 * 1024 * 1024];
        let (encoded, encoding) = compress_encode(&bomb_data);
        let start = std::time::Instant::now();
        let _ = decode_decompress_bounded(&encoded, &encoding, 1_048_576);
        assert!(
            start.elapsed() < Duration::from_millis(100),
            "Should fail fast"
        );
    }
}
