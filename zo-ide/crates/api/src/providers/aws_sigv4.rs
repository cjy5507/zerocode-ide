//! Minimal AWS Signature Version 4 (`SigV4`) signing — exactly what the Bedrock
//! Messages endpoint needs (`POST`, JSON body, no query string), hand-rolled
//! on `sha2` instead of pulling the `aws-sigv4` crate tree. The HMAC chain
//! and canonical-request format follow the `SigV4` specification; the unit
//! tests pin signatures cross-computed with an independent implementation
//! (python `hmac`/`hashlib`).
//!
//! Credentials resolve from the environment (`AWS_ACCESS_KEY_ID` /
//! `AWS_SECRET_ACCESS_KEY` / `AWS_SESSION_TOKEN`) and, failing that, from the
//! `~/.aws/credentials` INI (profile from `AWS_PROFILE`, default `default`).
//! Fancier chain members (SSO, IMDS, process providers) are intentionally
//! out of scope — the gateway's error message points at the supported paths.

use std::time::{SystemTime, UNIX_EPOCH};

use core_types::hex::to_hex_lower;

use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AwsCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
}

/// Env first, then the shared credentials file. `None` when neither yields a
/// complete key pair.
pub(crate) fn resolve_credentials(
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Option<AwsCredentials> {
    let non_empty = |key: &str| {
        lookup(key)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    if let (Some(access_key_id), Some(secret_access_key)) = (
        non_empty("AWS_ACCESS_KEY_ID"),
        non_empty("AWS_SECRET_ACCESS_KEY"),
    ) {
        return Some(AwsCredentials {
            access_key_id,
            secret_access_key,
            session_token: non_empty("AWS_SESSION_TOKEN"),
        });
    }
    let path = non_empty("AWS_SHARED_CREDENTIALS_FILE")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            lookup("HOME").map(|home| {
                std::path::PathBuf::from(home)
                    .join(".aws")
                    .join("credentials")
            })
        })?;
    let profile = non_empty("AWS_PROFILE").unwrap_or_else(|| "default".to_string());
    let contents = std::fs::read_to_string(path).ok()?;
    credentials_from_ini(&contents, &profile)
}

/// Tiny INI reader for the shared credentials file: `[profile]` sections,
/// `key = value` lines, `#`/`;` comments.
pub(crate) fn credentials_from_ini(contents: &str, profile: &str) -> Option<AwsCredentials> {
    let mut in_profile = false;
    let mut access_key_id = None;
    let mut secret_access_key = None;
    let mut session_token = None;
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(section) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            in_profile = section.trim() == profile;
            continue;
        }
        if !in_profile {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().to_string();
        match key.trim() {
            "aws_access_key_id" => access_key_id = Some(value),
            "aws_secret_access_key" => secret_access_key = Some(value),
            "aws_session_token" => session_token = Some(value).filter(|v| !v.is_empty()),
            _ => {}
        }
    }
    Some(AwsCredentials {
        access_key_id: access_key_id?,
        secret_access_key: secret_access_key?,
        session_token,
    })
}

/// Sign one request; returns the headers to attach: `x-amz-date`,
/// `authorization`, and `x-amz-security-token` when present. `content-type:
/// application/json` is part of the signed set, so the caller must send
/// exactly that header (it already does).
pub(crate) fn sign_request(
    credentials: &AwsCredentials,
    region: &str,
    service: &str,
    host: &str,
    path: &str,
    payload: &[u8],
    now: SystemTime,
) -> Vec<(String, String)> {
    let (amz_date, date) = amz_timestamp(now);
    let payload_hash = to_hex_lower(&Sha256::digest(payload));

    let mut canonical_headers =
        format!("content-type:application/json\nhost:{host}\nx-amz-date:{amz_date}\n");
    let mut signed_headers = "content-type;host;x-amz-date".to_string();
    if let Some(token) = &credentials.session_token {
        use std::fmt::Write;
        let _ = writeln!(canonical_headers, "x-amz-security-token:{token}");
        signed_headers.push_str(";x-amz-security-token");
    }
    let canonical_request =
        format!("POST\n{path}\n\n{canonical_headers}\n{signed_headers}\n{payload_hash}");

    let scope = format!("{date}/{region}/{service}/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        to_hex_lower(&Sha256::digest(canonical_request.as_bytes()))
    );

    let mut key = hmac_sha256(
        format!("AWS4{}", credentials.secret_access_key).as_bytes(),
        date.as_bytes(),
    );
    for part in [region, service, "aws4_request"] {
        key = hmac_sha256(&key, part.as_bytes());
    }
    let signature = to_hex_lower(&hmac_sha256(&key, string_to_sign.as_bytes()));

    let mut headers = vec![
        ("x-amz-date".to_string(), amz_date),
        (
            "authorization".to_string(),
            format!(
                "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
                credentials.access_key_id
            ),
        ),
    ];
    if let Some(token) = &credentials.session_token {
        headers.push(("x-amz-security-token".to_string(), token.clone()));
    }
    headers
}

/// RFC 2104 `HMAC-SHA256` on top of `sha2` — the only primitive `SigV4` needs
/// beyond plain `SHA-256`, so it is not worth a crate.
fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut key_block = [0u8; BLOCK];
    if key.len() > BLOCK {
        key_block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(key_block.map(|b| b ^ 0x36));
    inner.update(data);
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(key_block.map(|b| b ^ 0x5c));
    outer.update(inner);
    outer.finalize().into()
}

/// `(YYYYMMDDTHHMMSSZ, YYYYMMDD)` in UTC, no chrono dependency — civil-date
/// conversion via the standard days-from-epoch algorithm.
#[allow(
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    reason = "seconds-since-epoch fits i64; field values are 0..=9999, well within range"
)]
fn amz_timestamp(now: SystemTime) -> (String, String) {
    let secs = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let days = (secs / 86_400) as i64;
    let (year, month, day) = civil_from_days(days);
    let rem = secs % 86_400;
    let (hour, minute, second) = (rem / 3_600, rem % 3_600 / 60, rem % 60);
    let date = format!("{year:04}{month:02}{day:02}");
    (format!("{date}T{hour:02}{minute:02}{second:02}Z"), date)
}

/// Howard Hinnant's `civil_from_days`: days since 1970-01-01 → (y, m, d).
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    reason = "intermediate day-of-era values are bounded to 0..146097; month/day to 1..=31"
)]
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn at(unix_secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(unix_secs)
    }

    /// python `datetime`과 교차 검증한 UTC 타임스탬프 포맷.
    #[test]
    fn amz_timestamp_matches_python_datetime() {
        assert_eq!(amz_timestamp(at(0)).0, "19700101T000000Z");
        assert_eq!(amz_timestamp(at(1_762_000_000)).0, "20251101T122640Z");
        assert_eq!(amz_timestamp(at(4_102_444_800)).0, "21000101T000000Z");
    }

    /// 독립 구현(python hmac/hashlib)으로 계산한 서명과 일치해야 한다 —
    /// 세션 토큰 없는 경우와 있는 경우 모두.
    #[test]
    fn signature_matches_independent_python_implementation() {
        let payload = br#"{"max_tokens":1,"model":"anthropic.claude-opus-4-8"}"#;
        // 20260611T120000Z
        let now = at(1_781_179_200);
        let credentials = AwsCredentials {
            access_key_id: "AKIDEXAMPLE".to_string(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string(),
            session_token: None,
        };
        let headers = sign_request(
            &credentials,
            "us-east-1",
            "bedrock-mantle",
            "bedrock-mantle.us-east-1.api.aws",
            "/anthropic/v1/messages",
            payload,
            now,
        );
        assert_eq!(
            headers[0],
            ("x-amz-date".to_string(), "20260611T120000Z".to_string())
        );
        let authorization = &headers[1].1;
        assert!(
            authorization.ends_with(
                "Signature=f632c245932fc5f77988c5f50539b2620fe610555101a3903945727a85c409a3"
            ),
            "{authorization}"
        );
        assert!(
            authorization
                .contains("Credential=AKIDEXAMPLE/20260611/us-east-1/bedrock-mantle/aws4_request")
        );
        assert!(authorization.contains("SignedHeaders=content-type;host;x-amz-date,"));

        let with_token = AwsCredentials {
            session_token: Some("THETOKEN".to_string()),
            ..credentials
        };
        let headers = sign_request(
            &with_token,
            "us-east-1",
            "bedrock-mantle",
            "bedrock-mantle.us-east-1.api.aws",
            "/anthropic/v1/messages",
            payload,
            now,
        );
        assert!(
            headers[1].1.ends_with(
                "Signature=2aa9402fca44b66c48ac73a58c817f5ef3e0e8b0656d89bf9dfd672f55dc18bc"
            ),
            "{}",
            headers[1].1
        );
        assert!(
            headers
                .iter()
                .any(|(k, v)| k == "x-amz-security-token" && v == "THETOKEN")
        );
    }

    #[test]
    fn ini_credentials_parse_profiles_and_comments() {
        let ini = "
# shared credentials
[default]
aws_access_key_id = AKIA_DEFAULT
aws_secret_access_key = secret_default

[work]
aws_access_key_id=AKIA_WORK
aws_secret_access_key = secret_work
aws_session_token = tok_work
";
        let default = credentials_from_ini(ini, "default").expect("default profile");
        assert_eq!(default.access_key_id, "AKIA_DEFAULT");
        assert_eq!(default.session_token, None);
        let work = credentials_from_ini(ini, "work").expect("work profile");
        assert_eq!(work.secret_access_key, "secret_work");
        assert_eq!(work.session_token.as_deref(), Some("tok_work"));
        assert_eq!(credentials_from_ini(ini, "absent"), None);
    }

    /// env 가 파일보다 우선하고, 키 쌍이 불완전하면 파일로 폴백한다.
    #[test]
    fn env_credentials_win_over_file() {
        let lookup = |key: &str| match key {
            "AWS_ACCESS_KEY_ID" => Some("AKIA_ENV".to_string()),
            "AWS_SECRET_ACCESS_KEY" => Some("secret_env".to_string()),
            "AWS_SESSION_TOKEN" => Some(" ".to_string()), // blank → None
            _ => None,
        };
        let credentials = resolve_credentials(&lookup).expect("env pair");
        assert_eq!(credentials.access_key_id, "AKIA_ENV");
        assert_eq!(credentials.session_token, None);
    }
}
