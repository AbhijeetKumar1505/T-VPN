//! AWS Signature Version 4 request signing (no AWS SDK needed).
//!
//! <https://docs.aws.amazon.com/IAM/latest/UserGuide/reference_sigv-create-signed-request.html>

use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// HMAC-SHA256 (RFC 2104).
pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let inner = Sha256::new().chain_update(ipad).chain_update(data).finalize();
    let outer = Sha256::new().chain_update(opad).chain_update(inner).finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&outer);
    out
}

/// Percent-encode per RFC 3986 (what SigV4 and the AWS query protocol expect).
pub fn uri_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `application/x-www-form-urlencoded` body from key/value pairs.
pub fn form_body(params: &[(String, String)]) -> String {
    params.iter().map(|(k, v)| format!("{}={}", uri_encode(k), uri_encode(v))).collect::<Vec<_>>().join("&")
}

/// (`YYYYMMDD`, `YYYYMMDDTHHMMSSZ`) in UTC.
pub fn amz_dates(t: SystemTime) -> (String, String) {
    let secs = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let date = format!("{y:04}{m:02}{d:02}");
    (date.clone(), format!("{date}T{hh:02}{mm:02}{ss:02}Z"))
}

/// Days since 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub struct Request<'a> {
    pub method: &'a str,
    pub path: &'a str,
    /// Already canonical: sorted, URI-encoded `k=v` pairs joined by `&`.
    pub query: &'a str,
    /// Header names in lowercase; every header listed here is signed.
    pub headers: &'a [(&'a str, String)],
    pub payload: &'a [u8],
    pub region: &'a str,
    pub service: &'a str,
    pub amz_date: &'a str,
}

pub struct Signature {
    pub canonical_request_hash: String,
    pub signature: String,
    pub authorization: String,
}

pub fn sign(access_key_id: &str, secret_access_key: &str, req: &Request<'_>) -> Signature {
    let date = &req.amz_date[..8];
    let mut headers: Vec<(String, String)> =
        req.headers.iter().map(|(k, v)| (k.to_ascii_lowercase(), v.trim().to_string())).collect();
    headers.sort();
    let canonical_headers: String = headers.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
    let signed_headers = headers.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(";");
    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        req.method,
        req.path,
        req.query,
        canonical_headers,
        signed_headers,
        sha256_hex(req.payload)
    );
    let canonical_request_hash = sha256_hex(canonical_request.as_bytes());
    let scope = format!("{date}/{}/{}/aws4_request", req.region, req.service);
    let string_to_sign = format!("AWS4-HMAC-SHA256\n{}\n{scope}\n{canonical_request_hash}", req.amz_date);

    let k_date = hmac_sha256(format!("AWS4{secret_access_key}").as_bytes(), date.as_bytes());
    let k_region = hmac_sha256(&k_date, req.region.as_bytes());
    let k_service = hmac_sha256(&k_region, req.service.as_bytes());
    let k_signing = hmac_sha256(&k_service, b"aws4_request");
    let signature = hex(&hmac_sha256(&k_signing, string_to_sign.as_bytes()));

    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/{scope}, SignedHeaders={signed_headers}, Signature={signature}"
    );
    Signature { canonical_request_hash, signature, authorization }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn hmac_rfc4231_case_2() {
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(hex(&mac), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
    }

    #[test]
    fn hmac_rfc4231_long_key() {
        // Test case 6: 131-byte key (longer than the block size).
        let key = [0xaau8; 131];
        let mac = hmac_sha256(&key, b"Test Using Larger Than Block-Size Key - Hash Key First");
        assert_eq!(hex(&mac), "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54");
    }

    #[test]
    fn aws_documented_example() {
        // The IAM ListUsers example from the AWS SigV4 documentation.
        let headers = [
            ("content-type", "application/x-www-form-urlencoded; charset=utf-8".to_string()),
            ("host", "iam.amazonaws.com".to_string()),
            ("x-amz-date", "20150830T123600Z".to_string()),
        ];
        let sig = sign(
            "AKIDEXAMPLE",
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            &Request {
                method: "GET",
                path: "/",
                query: "Action=ListUsers&Version=2010-05-08",
                headers: &headers,
                payload: b"",
                region: "us-east-1",
                service: "iam",
                amz_date: "20150830T123600Z",
            },
        );
        assert_eq!(
            sig.canonical_request_hash,
            "f536975d06c0309214f805bb90ccff089219ecd68b2577efef23edd43b7e1a59"
        );
        assert_eq!(sig.signature, "5d672d79c15b13162d9279b0855cfba6789a8edb4c82c400e06b5924a6f2b5d7");
        assert!(sig.authorization.starts_with(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/iam/aws4_request, \
             SignedHeaders=content-type;host;x-amz-date, Signature="
        ));
    }

    #[test]
    fn dates() {
        let t = UNIX_EPOCH + Duration::from_secs(1_440_938_160); // 2015-08-30T12:36:00Z
        assert_eq!(amz_dates(t), ("20150830".into(), "20150830T123600Z".into()));
        let leap = UNIX_EPOCH + Duration::from_secs(951_782_400); // 2000-02-29T00:00:00Z
        assert_eq!(amz_dates(leap).1, "20000229T000000Z");
    }

    #[test]
    fn encoding() {
        assert_eq!(uri_encode("a b+c/=~"), "a%20b%2Bc%2F%3D~");
        assert_eq!(
            form_body(&[("Action".into(), "RunInstances".into()), ("UserData".into(), "ab+/=".into())]),
            "Action=RunInstances&UserData=ab%2B%2F%3D"
        );
    }
}
