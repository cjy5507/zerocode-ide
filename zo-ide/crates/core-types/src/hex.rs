//! Lowercase hexadecimal encoding.
//!
//! Single source of truth for the `bytes -> lowercase hex string` encoding that
//! the OTLP exporter (trace/span ids) and the AWS `SigV4` signer (digests and
//! signatures) both need, without pulling in an external `hex` crate.

use std::fmt::Write;

/// Encode `bytes` as a lowercase hex string (two chars per byte, no separator).
#[must_use]
pub fn to_hex_lower(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut acc, b| {
            let _ = write!(acc, "{b:02x}");
            acc
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_bytes_as_lowercase_hex() {
        assert_eq!(to_hex_lower(&[]), "");
        assert_eq!(to_hex_lower(&[0x00, 0x0f, 0xff]), "000fff");
        assert_eq!(to_hex_lower(&[0xde, 0xad, 0xbe, 0xef]), "deadbeef");
    }
}
