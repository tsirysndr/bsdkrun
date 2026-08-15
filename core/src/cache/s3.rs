//! The S3 cache backend: AWS Signature Version 4, sent with `curl`.
//!
//! Signing is a pure function of the request, so it needs no HTTP client — we
//! compute the `Authorization` header and hand the request to `curl`, which
//! this crate already depends on for every image pull. That keeps the "no HTTP
//! crate, no async runtime" rule in `oci.rs` intact and works against anything
//! that speaks S3: AWS, Cloudflare R2, MinIO, Backblaze B2.
//!
//! Reference: <https://docs.aws.amazon.com/AmazonS3/latest/API/sig-v4-header-based-auth.html>

use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use super::config::{credentials, Credentials, S3Config};

type HmacSha256 = Hmac<Sha256>;

/// `UNSIGNED-PAYLOAD` is allowed over HTTPS, but signing the real hash is what
/// MinIO and older gateways expect, and it costs one pass over a file we have
/// on disk anyway.
fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn sha256_file(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)
        .with_context(|| format!("opening {} to sign it", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn hmac(key: &[u8], data: &str) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("hmac takes a key of any length");
    mac.update(data.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

/// Percent-encode a path segment per RFC 3986, leaving `/` alone.
///
/// S3 signs the *encoded* path, so this has to agree byte for byte with what
/// curl puts on the wire — which is why keys are restricted to a safe alphabet
/// upstream rather than relying on this to normalise anything exotic.
fn uri_encode(s: &str, encode_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b'/' if !encode_slash => out.push('/'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// A signed request, ready to hand to curl.
struct Signed {
    url: String,
    headers: Vec<(String, String)>,
}

/// Build the SigV4 headers for one request.
///
/// `query` must already be in canonical form: sorted by key, URI-encoded. The
/// only query this module sends is ListObjectsV2, which builds it that way.
fn sign(
    cfg: &S3Config,
    creds: &Credentials,
    method: &str,
    key: &str,
    query: &str,
    payload_hash: &str,
    now: (String, String),
) -> Signed {
    let (date_stamp, amz_date) = now; // (YYYYMMDD, YYYYMMDDTHHMMSSZ)
    let host = cfg.host();

    // A path-style endpoint puts the bucket in the path, so the canonical URI
    // has to include it — take whatever follows the host in the base URL.
    let base = cfg.base_url();
    let path_prefix = base
        .split_once("://")
        .map(|(_, rest)| rest)
        .and_then(|rest| rest.split_once('/'))
        .map(|(_, p)| format!("/{p}"))
        .unwrap_or_default();
    let canonical_uri = format!("{}/{}", path_prefix, uri_encode(key, false));

    let mut headers: Vec<(String, String)> = vec![
        ("host".into(), host.clone()),
        ("x-amz-content-sha256".into(), payload_hash.to_string()),
        ("x-amz-date".into(), amz_date.clone()),
    ];
    if let Some(token) = &creds.session_token {
        headers.push(("x-amz-security-token".into(), token.clone()));
    }
    headers.sort_by(|a, b| a.0.cmp(&b.0));

    let signed_headers = headers
        .iter()
        .map(|(k, _)| k.as_str())
        .collect::<Vec<_>>()
        .join(";");
    let canonical_headers = headers
        .iter()
        .map(|(k, v)| format!("{k}:{}\n", v.trim()))
        .collect::<String>();

    let canonical_request = format!(
        "{method}\n{canonical_uri}\n{query}\n{canonical_headers}\n{signed_headers}\n{payload_hash}"
    );

    let scope = format!("{date_stamp}/{}/s3/aws4_request", cfg.region);
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        sha256_hex(canonical_request.as_bytes())
    );

    let k_date = hmac(format!("AWS4{}", creds.secret_key).as_bytes(), &date_stamp);
    let k_region = hmac(&k_date, &cfg.region);
    let k_service = hmac(&k_region, "s3");
    let k_signing = hmac(&k_service, "aws4_request");
    let signature = hex(&hmac(&k_signing, &string_to_sign));

    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
        creds.access_key
    );

    let mut out: Vec<(String, String)> = headers
        .into_iter()
        .filter(|(k, _)| k != "host") // curl derives Host from the URL
        .collect();
    out.push(("authorization".into(), authorization));

    let url = if query.is_empty() {
        format!("{base}/{key}")
    } else {
        format!("{base}/{key}?{query}")
    };
    Signed { url, headers: out }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `(YYYYMMDD, YYYYMMDDTHHMMSSZ)` for right now, which is what SigV4 wants.
fn timestamps() -> (String, String) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (date, time) = civil(secs);
    (date.clone(), format!("{date}T{time}Z"))
}

/// Split unix seconds into `(YYYYMMDD, HHMMSS)` — the civil-from-days algorithm,
/// same as the estargz TOC's timestamps.
fn civil(secs: u64) -> (String, String) {
    let days = (secs / 86_400) as i64;
    let tod = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (
        format!("{y:04}{m:02}{d:02}"),
        format!("{:02}{:02}{:02}", tod / 3600, (tod % 3600) / 60, tod % 60),
    )
}

/// Send a signed request, writing the response body to `body_to` and returning
/// the HTTP status.
///
/// The body always goes to a file and the status always comes back on stdout,
/// because the alternative — `--fail-with-body` plus curl's exit code — cannot
/// tell a 404 from a network error once `-o` has taken the body away. Reading
/// the status directly is what lets "not cached yet" be an ordinary answer
/// rather than a failed command.
fn curl(signed: &Signed, extra: &[&str], body_to: &Path, what: &str) -> Result<u16> {
    let mut cmd = Command::new("curl");
    cmd.args(["-sS", "--max-time", "900", "-w", "%{http_code}", "-o"])
        .arg(body_to);
    for (k, v) in &signed.headers {
        cmd.arg("-H").arg(format!("{k}: {v}"));
    }
    cmd.args(extra).arg(&signed.url);

    let out = cmd
        .output()
        .map_err(|e| crate::fetch::spawn_error(&cmd, what, e))?;
    if !out.status.success() {
        bail!(
            "{what} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .with_context(|| format!("{what}: curl reported no HTTP status"))
}

/// Fail unless the status is a success, quoting S3's own explanation.
fn expect_ok(status: u16, body_to: &Path, what: &str) -> Result<()> {
    if (200..300).contains(&status) {
        return Ok(());
    }
    let body = std::fs::read_to_string(body_to).unwrap_or_default();
    // S3 explains itself in an XML body; surfacing that beats "HTTP 403".
    let detail = extract(&body, "Message")
        .or_else(|| extract(&body, "Code"))
        .unwrap_or_else(|| body.trim().to_string());
    if detail.is_empty() {
        bail!("{what} failed with HTTP {status}");
    }
    bail!("{what} failed (HTTP {status}): {detail}");
}

/// Pull the text out of the first `<tag>…</tag>`. S3's error and listing bodies
/// are small, flat XML; a parser crate would be the only dependency here that
/// earned nothing.
fn extract(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].to_string())
}

/// Every `<tag>` value in the document, in order.
fn extract_all(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let from = start + open.len();
        let Some(end) = rest[from..].find(&close) else {
            break;
        };
        out.push(rest[from..from + end].to_string());
        rest = &rest[from + end + close.len()..];
    }
    out
}

/// Upload `file` to `key`.
pub fn put_file(cfg: &S3Config, key: &str, file: &Path) -> Result<()> {
    let creds = credentials()?;
    let hash = sha256_file(file)?;
    let signed = sign(
        cfg,
        &creds,
        "PUT",
        &cfg.object_key(key),
        "",
        &hash,
        timestamps(),
    );
    let path = file.to_string_lossy().into_owned();
    let what = format!("uploading the cache entry to s3://{}/{key}", cfg.bucket);
    let sink = tempfile::NamedTempFile::new()?;
    let status = curl(
        &signed,
        &["-X", "PUT", "--upload-file", &path],
        sink.path(),
        &what,
    )?;
    expect_ok(status, sink.path(), &what)
}

/// Upload `body` to `key`. Used for the small JSON metadata objects.
pub fn put_bytes(cfg: &S3Config, key: &str, body: &[u8]) -> Result<()> {
    let mut tmp = tempfile::NamedTempFile::new()?;
    std::io::Write::write_all(&mut tmp, body)?;
    std::io::Write::flush(&mut tmp)?;
    put_file(cfg, key, tmp.path())
}

/// Download `key` to `dest`. `Ok(false)` means it is simply not there.
pub fn get_file(cfg: &S3Config, key: &str, dest: &Path) -> Result<bool> {
    let creds = credentials()?;
    let signed = sign(
        cfg,
        &creds,
        "GET",
        &cfg.object_key(key),
        "",
        &sha256_hex(b""),
        timestamps(),
    );
    let what = format!("downloading s3://{}/{key}", cfg.bucket);
    let status = curl(&signed, &[], dest, &what)?;
    // 404 is the answer to "is this cached?", not a failure — and 403 is what a
    // bucket that hides missing keys returns instead, so treat both as a miss.
    if status == 404 || status == 403 {
        let _ = std::fs::remove_file(dest);
        return Ok(false);
    }
    expect_ok(status, dest, &what)?;
    Ok(true)
}

/// Fetch `key` into memory. Used for metadata.
pub fn get_bytes(cfg: &S3Config, key: &str) -> Result<Option<Vec<u8>>> {
    let tmp = tempfile::NamedTempFile::new()?;
    if !get_file(cfg, key, tmp.path())? {
        return Ok(None);
    }
    Ok(Some(std::fs::read(tmp.path())?))
}

pub fn delete(cfg: &S3Config, key: &str) -> Result<()> {
    let creds = credentials()?;
    let signed = sign(
        cfg,
        &creds,
        "DELETE",
        &cfg.object_key(key),
        "",
        &sha256_hex(b""),
        timestamps(),
    );
    let what = format!("deleting s3://{}/{key}", cfg.bucket);
    let sink = tempfile::NamedTempFile::new()?;
    let status = curl(&signed, &["-X", "DELETE"], sink.path(), &what)?;
    // S3 answers 204 for a delete whether or not the key was there.
    if status == 404 {
        return Ok(());
    }
    expect_ok(status, sink.path(), &what)
}

/// List object keys under `prefix`, relative to any configured bucket prefix.
pub fn list(cfg: &S3Config, prefix: &str) -> Result<Vec<String>> {
    let creds = credentials()?;
    let full = cfg.object_key(prefix);
    // Canonical query: sorted by key, values URI-encoded.
    let query = format!("list-type=2&prefix={}", uri_encode(&full, true));
    let signed = sign(
        cfg,
        &creds,
        "GET",
        "",
        &query,
        &sha256_hex(b""),
        timestamps(),
    );
    let what = format!("listing s3://{}/{full}", cfg.bucket);
    let sink = tempfile::NamedTempFile::new()?;
    let status = curl(&signed, &[], sink.path(), &what)?;
    expect_ok(status, sink.path(), &what)?;
    let xml = std::fs::read_to_string(sink.path()).unwrap_or_default();

    let strip = cfg.object_key("");
    Ok(extract_all(&xml, "Key")
        .into_iter()
        .map(|k| k.trim_start_matches(&strip).to_string())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn creds() -> Credentials {
        Credentials {
            access_key: "AKIDEXAMPLE".into(),
            secret_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into(),
            session_token: None,
        }
    }

    fn cfg() -> S3Config {
        S3Config {
            bucket: "examplebucket".into(),
            region: "us-east-1".into(),
            prefix: String::new(),
            endpoint: None,
        }
    }

    fn authorization(signed: &Signed) -> String {
        signed
            .headers
            .iter()
            .find(|(k, _)| k == "authorization")
            .map(|(_, v)| v.clone())
            .expect("every signed request carries an authorization header")
    }

    /// The signature is pinned against a value computed by an independent
    /// implementation of SigV4 (python hmac/hashlib) over the same inputs. A
    /// wrong signature is a flat 403 with no hint as to which of the five
    /// derivation steps drifted, so this is the test that has to be exact
    /// rather than approximate.
    #[test]
    fn signing_matches_an_independent_sigv4_implementation() {
        let signed = sign(
            &cfg(),
            &creds(),
            "GET",
            "test.txt",
            "",
            &sha256_hex(b""),
            ("20130524".into(), "20130524T000000Z".into()),
        );
        let auth = authorization(&signed);
        assert!(
            auth.contains("Credential=AKIDEXAMPLE/20130524/us-east-1/s3/aws4_request"),
            "{auth}"
        );
        assert!(
            auth.contains("SignedHeaders=host;x-amz-content-sha256;x-amz-date"),
            "{auth}"
        );
        assert!(
            auth.contains(
                "Signature=0cee62862edd8e0aec93e9fbb49b3463c45da6ba7363da395910103de3775840"
            ),
            "{auth}"
        );
    }

    /// The empty-payload hash is a constant AWS documents; getting it wrong
    /// signs a body we never send.
    #[test]
    fn the_empty_payload_hash_is_the_documented_constant() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    /// A session token has to be both sent and signed; signing without it is a
    /// 403 that reads like bad credentials.
    #[test]
    fn a_session_token_is_signed_not_just_sent() {
        let mut c = creds();
        c.session_token = Some("tok".into());
        let signed = sign(
            &cfg(),
            &c,
            "GET",
            "k",
            "",
            &sha256_hex(b""),
            ("20130524".into(), "20130524T000000Z".into()),
        );
        let auth = authorization(&signed);
        assert!(auth.contains("x-amz-security-token"), "{auth}");
        assert!(signed
            .headers
            .iter()
            .any(|(k, v)| k == "x-amz-security-token" && v == "tok"));
    }

    /// A path-style endpoint puts the bucket in the URI, and the *signature*
    /// covers that path — sign the AWS-style path against MinIO and every
    /// request fails authentication.
    #[test]
    fn a_path_style_endpoint_signs_the_bucket_in_the_uri() {
        let cfg = S3Config {
            bucket: "b".into(),
            region: "auto".into(),
            prefix: String::new(),
            endpoint: Some("https://minio.local:9000".into()),
        };
        let signed = sign(
            &cfg,
            &creds(),
            "GET",
            "obj",
            "",
            &sha256_hex(b""),
            ("20130524".into(), "20130524T000000Z".into()),
        );
        assert_eq!(signed.url, "https://minio.local:9000/b/obj");
    }

    #[test]
    fn uri_encoding_leaves_safe_characters_alone() {
        assert_eq!(uri_encode("a/b-c_d.e~f", false), "a/b-c_d.e~f");
        assert_eq!(uri_encode("a/b", true), "a%2Fb");
        assert_eq!(uri_encode("a b+c", false), "a%20b%2Bc");
    }

    #[test]
    fn error_bodies_are_read_for_their_message() {
        let xml = "<Error><Code>NoSuchKey</Code><Message>The specified key does not exist.</Message></Error>";
        assert_eq!(extract(xml, "Code").as_deref(), Some("NoSuchKey"));
        assert_eq!(
            extract(xml, "Message").as_deref(),
            Some("The specified key does not exist.")
        );
    }

    #[test]
    fn listings_yield_every_key() {
        let xml = "<ListBucketResult><Contents><Key>a.json</Key></Contents>\
                   <Contents><Key>b.tar.gz</Key></Contents></ListBucketResult>";
        assert_eq!(extract_all(xml, "Key"), vec!["a.json", "b.tar.gz"]);
    }

    /// A 404 has to stay distinguishable from a failure: it is the answer to
    /// "is this key cached yet?", which every `cache save` asks first. Reading
    /// it out of curl's exit code instead of the status made the very first
    /// save against a real bucket fail.
    #[test]
    fn a_successful_status_passes_and_a_failure_quotes_s3s_own_message() {
        let body = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(body.path(), "").unwrap();
        expect_ok(200, body.path(), "x").unwrap();
        expect_ok(204, body.path(), "x").unwrap();

        std::fs::write(
            body.path(),
            "<Error><Code>AccessDenied</Code><Message>Access Denied.</Message></Error>",
        )
        .unwrap();
        let err = expect_ok(403, body.path(), "uploading")
            .unwrap_err()
            .to_string();
        assert!(err.contains("403"), "{err}");
        assert!(err.contains("Access Denied."), "{err}");

        // An empty body still has to name the status rather than say nothing.
        std::fs::write(body.path(), "").unwrap();
        let err = expect_ok(500, body.path(), "uploading")
            .unwrap_err()
            .to_string();
        assert!(err.contains("500"), "{err}");
    }

    #[test]
    fn timestamps_are_the_shape_sigv4_requires() {
        let (date, time) = civil(1_369_353_600); // 2013-05-24T00:00:00Z
        assert_eq!(date, "20130524");
        assert_eq!(time, "000000");
    }
}
