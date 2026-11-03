//! Narrow S3 backup adapter: fixed bucket/prefix, SigV4, no redirects or opaque
//! user URLs. Compatibility endpoints remain unverified as off-host storage.
use crate::{
    config::read_private,
    error::{Error, Result},
};
use base64::{engine::general_purpose::STANDARD, Engine};
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, io::Read, path::Path, time::Duration};
use url::Url;
use zeroize::{Zeroize, Zeroizing};
const CONTROL_LIMIT: usize = 1048576;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CredentialFile {
    access_key_id: String,
    secret_access_key: String,
    session_token: Option<String>,
}
impl Drop for CredentialFile {
    fn drop(&mut self) {
        self.secret_access_key.zeroize();
        if let Some(token) = &mut self.session_token {
            token.zeroize();
        }
    }
}
pub struct Credentials {
    access: String,
    secret: Zeroizing<String>,
    token: Option<Zeroizing<String>>,
}
impl Credentials {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = Zeroizing::new(read_private(path, 32768)?);
        let mut file: CredentialFile = serde_json::from_slice(&raw)
            .map_err(|_| Error::unavailable("Backup credentials are invalid"))?;
        Self::new(
            std::mem::take(&mut file.access_key_id),
            std::mem::take(&mut file.secret_access_key),
            file.session_token.take(),
        )
    }
    fn new(access: String, secret: String, token: Option<String>) -> Result<Self> {
        let secret = Zeroizing::new(secret);
        let token = token.map(Zeroizing::new);
        if !(3..=128).contains(&access.len())
            || !access
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || !(8..=256).contains(&secret.len())
            || !secret.is_ascii()
            || secret.bytes().any(|b| b.is_ascii_control())
            || token.as_ref().is_some_and(|t| {
                t.is_empty()
                    || t.len() > 16384
                    || !t.is_ascii()
                    || t.bytes().any(|b| b.is_ascii_control())
            })
        {
            return Err(Error::unavailable("Backup credentials are invalid"));
        }
        Ok(Self {
            access,
            secret,
            token,
        })
    }
}
#[derive(Clone)]
pub struct Location {
    bucket: String,
    prefix: String,
    region: String,
    endpoint: Url,
    off_host: bool,
}
impl Location {
    pub fn aws(uri: &str, region: &str) -> Result<Self> {
        if !(3..=63).contains(&region.len())
            || !region
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(Error::invalid("Invalid S3 region"));
        }
        let uri = Url::parse(uri).map_err(|_| Error::invalid("Invalid S3 backup URI"))?;
        if uri.scheme() != "s3"
            || !uri.username().is_empty()
            || uri.password().is_some()
            || uri.port().is_some()
            || uri.query().is_some()
            || uri.fragment().is_some()
        {
            return Err(Error::invalid("Invalid S3 backup URI"));
        }
        let bucket = uri
            .host_str()
            .ok_or_else(|| Error::invalid("S3 bucket is required"))?;
        if !(3..=63).contains(&bucket.len())
            || !bucket
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'.'))
            || bucket.starts_with(['-', '.'])
            || bucket.ends_with(['-', '.'])
            || bucket.contains("..")
            || bucket.parse::<std::net::IpAddr>().is_ok()
        {
            return Err(Error::invalid("Invalid S3 bucket"));
        }
        let prefix = uri.path().trim_start_matches('/').trim_end_matches('/');
        if prefix.is_empty()
            || prefix.len() > 256
            || !prefix
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_'))
            || prefix.split('/').any(str::is_empty)
        {
            return Err(Error::invalid(
                "A bounded node-specific S3 prefix is required",
            ));
        }
        let domain = if region.starts_with("cn-") {
            "amazonaws.com.cn"
        } else {
            "amazonaws.com"
        };
        let endpoint = Url::parse(&format!("https://s3.{region}.{domain}")).unwrap();
        Ok(Self {
            bucket: bucket.into(),
            prefix: format!("{prefix}/"),
            region: region.into(),
            endpoint,
            off_host: true,
        })
    }
    /// An operator may use another S3 service; its physical placement is not
    /// established merely by this URL, so it cannot satisfy public off_host.
    pub fn compatible(mut self, endpoint: &str) -> Result<Self> {
        let endpoint = Url::parse(endpoint)
            .map_err(|_| Error::invalid("Invalid S3 compatibility endpoint"))?;
        if !matches!(endpoint.scheme(), "http" | "https")
            || endpoint.host_str().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
            || endpoint.path() != "/"
        {
            return Err(Error::invalid("Invalid S3 compatibility endpoint"));
        }
        if endpoint.scheme() == "http"
            && !endpoint
                .host_str()
                .and_then(|s| s.parse::<std::net::IpAddr>().ok())
                .is_some_and(|ip| ip.is_loopback())
        {
            return Err(Error::invalid("S3 compatibility transport requires HTTPS"));
        }
        self.endpoint = endpoint;
        self.off_host = false;
        Ok(self)
    }
    pub fn off_host(&self) -> bool {
        self.off_host
    }
    pub fn reference(&self, key: &str) -> Result<String> {
        self.check_key(key)?;
        Ok(format!("s3://{}/{}{key}", self.bucket, self.prefix))
    }
    fn check_key(&self, key: &str) -> Result<()> {
        if key.is_empty()
            || key.len() > 512
            || key.starts_with('/')
            || !key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.'))
            || key
                .split('/')
                .any(|p| p.is_empty() || p == "." || p == "..")
        {
            return Err(Error::invalid("Invalid managed backup key"));
        }
        Ok(())
    }
    fn url(&self, key: Option<&str>, query: &[(String, String)]) -> Result<Url> {
        let path = if let Some(key) = key {
            self.check_key(key)?;
            format!("/{}/{}{key}", self.bucket, self.prefix)
        } else {
            format!("/{}", self.bucket)
        };
        let mut url = self.endpoint.clone();
        url.set_path(&path);
        let mut pairs: Vec<_> = query
            .iter()
            .map(|(k, v)| (encode(k.as_bytes()), encode(v.as_bytes())))
            .collect();
        pairs.sort();
        if !pairs.is_empty() {
            url.set_query(Some(
                &pairs
                    .into_iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join("&"),
            ));
        }
        Ok(url)
    }
}
fn encode(bytes: &[u8]) -> String {
    let mut out = String::new();
    for b in bytes {
        if b.is_ascii_alphanumeric() || matches!(*b, b'-' | b'_' | b'.' | b'~') {
            out.push(*b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}
fn authorization(
    credentials: &Credentials,
    region: &str,
    method: &str,
    url: &Url,
    headers: &BTreeMap<String, String>,
    body_hash: &str,
    now: DateTime<Utc>,
) -> Result<String> {
    let date = now.format("%Y%m%d").to_string();
    let stamp = now.format("%Y%m%dT%H%M%SZ").to_string();
    let canonical_headers = headers
        .iter()
        .map(|(k, v)| {
            format!(
                "{k}:{}\n",
                v.split_whitespace().collect::<Vec<_>>().join(" ")
            )
        })
        .collect::<String>();
    let signed = headers.keys().cloned().collect::<Vec<_>>().join(";");
    let canonical = format!(
        "{method}\n{}\n{}\n{canonical_headers}\n{signed}\n{body_hash}",
        url.path(),
        url.query().unwrap_or("")
    );
    let scope = format!("{date}/{region}/s3/aws4_request");
    let to_sign = format!(
        "AWS4-HMAC-SHA256\n{stamp}\n{scope}\n{:x}",
        Sha256::digest(canonical.as_bytes())
    );
    let secret = Zeroizing::new(format!("AWS4{}", credentials.secret.as_str()));
    let date_key = Zeroizing::new(hmac(secret.as_bytes(), date.as_bytes()));
    let region_key = Zeroizing::new(hmac(&date_key, region.as_bytes()));
    let service_key = Zeroizing::new(hmac(&region_key, b"s3"));
    let signing = Zeroizing::new(hmac(&service_key, b"aws4_request"));
    let signature = hmac(&signing, to_sign.as_bytes());
    Ok(format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed}, Signature={}",
        credentials.access,
        signature
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    ))
}
pub struct Client {
    location: Location,
    credentials: Credentials,
    http: reqwest::Client,
}
impl Client {
    pub fn new(location: Location, credentials: Credentials) -> Result<Self> {
        let mut http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(300));
        if let Some(path) =
            std::env::var_os("SSL_CERT_FILE").or_else(|| std::env::var_os("REQUESTS_CA_BUNDLE"))
        {
            let mut bytes = Vec::new();
            std::fs::File::open(path)?
                .take(2097153)
                .read_to_end(&mut bytes)?;
            if bytes.len() > 2097152 {
                return Err(Error::invalid("CA bundle exceeds its limit"));
            }
            for cert in reqwest::Certificate::from_pem_bundle(&bytes)
                .map_err(|_| Error::invalid("Invalid backup CA bundle"))?
            {
                http = http.add_root_certificate(cert);
            }
        }
        Ok(Self {
            location,
            credentials,
            http: http
                .build()
                .map_err(|_| Error::unavailable("S3 transport is unavailable"))?,
        })
    }
    pub fn off_host(&self) -> bool {
        self.location.off_host()
    }
    async fn send(
        &self,
        method: &str,
        key: Option<&str>,
        query: &[(String, String)],
        body: Vec<u8>,
        extra: &[(&str, String)],
    ) -> Result<reqwest::Response> {
        let now = Utc::now();
        let url = self.location.url(key, query)?;
        let digest = format!("{:x}", Sha256::digest(&body));
        let mut headers = BTreeMap::from([
            (
                "host".into(),
                url[url::Position::BeforeHost..url::Position::AfterPort].to_owned(),
            ),
            (
                "x-amz-date".into(),
                now.format("%Y%m%dT%H%M%SZ").to_string(),
            ),
            ("x-amz-content-sha256".into(), digest.clone()),
        ]);
        if let Some(token) = &self.credentials.token {
            headers.insert("x-amz-security-token".into(), token.to_string());
        }
        for (name, value) in extra {
            headers.insert(name.to_ascii_lowercase(), value.clone());
        }
        let signed = authorization(
            &self.credentials,
            &self.location.region,
            method,
            &url,
            &headers,
            &digest,
            now,
        )?;
        let method = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|_| Error::invalid("Invalid S3 method"))?;
        let mut request = self.http.request(method, url).body(body);
        for (name, value) in headers {
            let mut value = reqwest::header::HeaderValue::from_str(&value)
                .map_err(|_| Error::invalid("Invalid S3 header"))?;
            if name == "x-amz-security-token" {
                value.set_sensitive(true);
            }
            request = request.header(name, value);
        }
        let mut authorization = reqwest::header::HeaderValue::from_str(&signed)
            .map_err(|_| Error::unavailable("Invalid S3 authorization header"))?;
        authorization.set_sensitive(true);
        request
            .header(reqwest::header::AUTHORIZATION, authorization)
            .send()
            .await
            .map_err(|_| Error::unavailable("Backup object transport failed"))
    }
    pub async fn create_bucket(&self) -> Result<()> {
        let body = if self.location.region == "us-east-1" {
            Vec::new()
        } else {
            format!("<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><LocationConstraint>{}</LocationConstraint></CreateBucketConfiguration>",self.location.region).into_bytes()
        };
        let response = self.send("PUT", None, &[], body, &[]).await?;
        if !response.status().is_success() {
            return Err(Error::unavailable("Backup bucket creation failed"));
        }
        Ok(())
    }
    pub async fn put(&self, key: &str, body: Vec<u8>) -> Result<()> {
        let checksum = STANDARD.encode(Sha256::digest(&body));
        let response = self
            .send(
                "PUT",
                Some(key),
                &[],
                body,
                &[
                    ("if-none-match", "*".into()),
                    ("x-amz-checksum-sha256", checksum),
                ],
            )
            .await?;
        if response.status() == reqwest::StatusCode::PRECONDITION_FAILED {
            return Err(Error::conflict("Backup object already exists"));
        }
        if !response.status().is_success() {
            return Err(Error::unavailable("Backup object upload failed"));
        }
        Ok(())
    }
    pub async fn get(&self, key: &str) -> Result<reqwest::Response> {
        let response = self.send("GET", Some(key), &[], Vec::new(), &[]).await?;
        if !response.status().is_success() {
            return Err(Error::unavailable("Backup object read failed"));
        }
        Ok(response)
    }
    pub async fn initiate(&self, key: &str) -> Result<String> {
        #[derive(Deserialize)]
        struct Created {
            #[serde(rename = "UploadId")]
            id: String,
        }
        let response = self
            .send(
                "POST",
                Some(key),
                &[("uploads".into(), String::new())],
                Vec::new(),
                &[],
            )
            .await?;
        if !response.status().is_success() {
            return Err(Error::unavailable("Backup multipart creation failed"));
        }
        let body = bounded(response, CONTROL_LIMIT).await?;
        let value: Created = quick_xml::de::from_reader(body.as_slice())
            .map_err(|_| Error::unavailable("Invalid S3 multipart response"))?;
        if value.id.is_empty()
            || value.id.len() > 4096
            || value.id.bytes().any(|b| b.is_ascii_control())
        {
            return Err(Error::unavailable("Invalid multipart identity"));
        }
        Ok(value.id)
    }
    pub async fn part(
        &self,
        key: &str,
        upload: &str,
        number: u32,
        body: Vec<u8>,
    ) -> Result<String> {
        if !(1..=10000).contains(&number) || body.len() > 536870912 {
            return Err(Error::invalid("Invalid backup part"));
        }
        let response = self
            .send(
                "PUT",
                Some(key),
                &[
                    ("partNumber".into(), number.to_string()),
                    ("uploadId".into(), upload.into()),
                ],
                body,
                &[],
            )
            .await?;
        if !response.status().is_success() {
            return Err(Error::unavailable("Backup part upload failed"));
        }
        let etag = response
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| Error::unavailable("S3 part checksum is missing"))?;
        if etag.is_empty() || etag.len() > 256 || !etag.is_ascii() {
            return Err(Error::unavailable("Invalid S3 part identity"));
        }
        Ok(etag.into())
    }
    pub async fn complete(&self, key: &str, upload: &str, parts: &[String]) -> Result<()> {
        if parts.is_empty() || parts.len() > 10000 {
            return Err(Error::invalid("Invalid backup parts"));
        }
        let mut body = String::from("<CompleteMultipartUpload>");
        for (n, etag) in parts.iter().enumerate() {
            body.push_str(&format!(
                "<Part><PartNumber>{}</PartNumber><ETag>{}</ETag></Part>",
                n + 1,
                escape(etag)
            ));
        }
        body.push_str("</CompleteMultipartUpload>");
        let response = self
            .send(
                "POST",
                Some(key),
                &[("uploadId".into(), upload.into())],
                body.into_bytes(),
                &[("if-none-match", "*".into())],
            )
            .await?;
        if response.status() == reqwest::StatusCode::PRECONDITION_FAILED {
            return Err(Error::conflict("Backup object already exists"));
        }
        if !response.status().is_success() {
            return Err(Error::unavailable("Backup multipart completion failed"));
        }
        // S3 may return HTTP 200 with an embedded Error after the stream starts.
        let body = bounded(response, CONTROL_LIMIT).await?;
        #[derive(Deserialize)]
        #[serde(rename = "CompleteMultipartUploadResult")]
        struct Completed {
            #[serde(rename = "ETag")]
            etag: String,
        }
        let value: Completed = quick_xml::de::from_reader(body.as_slice())
            .map_err(|_| Error::unavailable("Backup multipart completion is unconfirmed"))?;
        if value.etag.is_empty() {
            return Err(Error::unavailable(
                "Backup multipart completion is unconfirmed",
            ));
        }
        Ok(())
    }
    pub async fn abort(&self, key: &str, upload: &str) -> Result<()> {
        let response = self
            .send(
                "DELETE",
                Some(key),
                &[("uploadId".into(), upload.into())],
                Vec::new(),
                &[],
            )
            .await?;
        if !response.status().is_success() && response.status() != reqwest::StatusCode::NOT_FOUND {
            return Err(Error::unavailable("Backup multipart cleanup is pending"));
        }
        Ok(())
    }
}
fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
pub async fn bounded(response: reqwest::Response, maximum: usize) -> Result<Vec<u8>> {
    let mut stream = response.bytes_stream();
    let mut out = Vec::new();
    while let Some(bytes) = stream.next().await {
        let bytes =
            bytes.map_err(|_| Error::unavailable("Backup object stream was interrupted"))?;
        if out
            .len()
            .checked_add(bytes.len())
            .is_none_or(|n| n > maximum)
        {
            return Err(Error::exhausted());
        }
        out.extend_from_slice(&bytes);
    }
    Ok(out)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use uuid::Uuid;
    pub async fn fixture() -> Option<(Client, tempfile::TempDir)> {
        crate::init_crypto();
        let Ok(endpoint) = std::env::var("XELAB_TEST_S3_ENDPOINT") else {
            eprintln!("SKIPPED: real S3 component requires XELAB_TEST_S3_ENDPOINT");
            return None;
        };
        let root = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in("/tmp")
            .unwrap();
        let file = root.path().join("s3.json");
        std::fs::write(&file,b"{\"access_key_id\":\"xelab-test\",\"secret_access_key\":\"test-only-backup-password\"}").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        let bucket = format!("xelab-test-{}", Uuid::new_v4().simple());
        let location = Location::aws(&format!("s3://{bucket}/node-test/"), "us-east-1")
            .unwrap()
            .compatible(&endpoint)
            .unwrap();
        let client = Client::new(location, Credentials::load(&file).unwrap()).unwrap();
        client.create_bucket().await.unwrap();
        Some((client, root))
    }
    pub async fn cleanup(client: &Client, keys: &[String]) {
        for key in keys {
            assert!(client
                .send("DELETE", Some(key), &[], Vec::new(), &[])
                .await
                .unwrap()
                .status()
                .is_success());
        }
        assert!(client
            .send("DELETE", None, &[], Vec::new(), &[])
            .await
            .unwrap()
            .status()
            .is_success());
    }
    #[test]
    fn backup_locations_keep_managed_keys_and_off_host_evidence_separate() {
        for uri in [
            "file:///tmp/backup",
            "s3://user:password@backup-bucket/node/",
            "s3://backup-bucket/",
            "s3://backup-bucket/node/?key=x",
            "s3://127.0.0.1/node/",
            "s3://backup-bucket/node//sub/",
        ] {
            assert!(Location::aws(uri, "us-east-1").is_err(), "{uri}");
        }
        let location = Location::aws("s3://backup-bucket/node-01/", "us-east-1").unwrap();
        assert!(location.off_host());
        assert!(location.clone().compatible("http://example.com/").is_err());
        let local = location.compatible("http://127.0.0.1:9000").unwrap();
        assert!(!local.off_host());
        for key in [
            "../other",
            "/absolute",
            "snapshots//object",
            "snapshots/../other",
            "snapshots/%2fescape",
        ] {
            assert!(local.reference(key).is_err());
        }
        assert_eq!(
            local.reference("snapshot/object.sealed").unwrap(),
            "s3://backup-bucket/node-01/snapshot/object.sealed"
        );
    }
    #[tokio::test]
    async fn actual_s3_encrypted_roundtrip_conditional_publish_multipart_and_abort() {
        crate::init_crypto();
        let Ok(endpoint) = std::env::var("XELAB_TEST_S3_ENDPOINT") else {
            eprintln!("SKIPPED: real S3 component requires XELAB_TEST_S3_ENDPOINT");
            return;
        };
        let root = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in("/tmp")
            .unwrap();
        let file = root.path().join("s3.json");
        std::fs::write(&file,b"{\"access_key_id\":\"xelab-test\",\"secret_access_key\":\"test-only-backup-password\"}").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        let bucket = format!("xelab-test-{}", Uuid::new_v4().simple());
        let location = Location::aws(&format!("s3://{bucket}/node-test/"), "us-east-1")
            .unwrap()
            .compatible(&endpoint)
            .unwrap();
        let client = Client::new(location.clone(), Credentials::load(&file).unwrap()).unwrap();
        assert!(!client.off_host());
        client.create_bucket().await.unwrap();
        let backup = Uuid::new_v4();
        let object = Uuid::new_v4();
        let key = format!("{backup}/{object}.sealed");
        let plaintext = vec![91; 131077];
        let mut encrypted = Vec::new();
        super::super::backup_crypto::seal(
            plaintext.as_slice(),
            &mut encrypted,
            &[55; 32],
            backup,
            object,
            plaintext.len() as u64,
        )
        .unwrap();
        client.put(&key, encrypted.clone()).await.unwrap();
        assert!(client
            .put(&key, b"must not replace".to_vec())
            .await
            .is_err());
        let downloaded = bounded(client.get(&key).await.unwrap(), encrypted.len())
            .await
            .unwrap();
        assert_eq!(downloaded, encrypted);
        let mut restored = Vec::new();
        super::super::backup_crypto::open(
            downloaded.as_slice(),
            &mut restored,
            &[55; 32],
            backup,
            object,
            plaintext.len() as u64,
        )
        .unwrap();
        assert_eq!(restored, plaintext);
        let wrong = Client::new(
            location,
            Credentials::new(
                "xelab-test".into(),
                "wrong-test-backup-password".into(),
                None,
            )
            .unwrap(),
        )
        .unwrap();
        assert!(wrong.get(&key).await.is_err());
        let large_key = format!("{backup}/{}.sealed", Uuid::new_v4());
        let large = vec![27; 10 * 1024 * 1024 + 177];
        let upload = client.initiate(&large_key).await.unwrap();
        let mut parts = Vec::new();
        for (n, part) in large.chunks(8 * 1024 * 1024).enumerate() {
            parts.push(
                client
                    .part(&large_key, &upload, (n + 1) as u32, part.to_vec())
                    .await
                    .unwrap(),
            );
        }
        client.complete(&large_key, &upload, &parts).await.unwrap();
        assert_eq!(
            bounded(client.get(&large_key).await.unwrap(), large.len())
                .await
                .unwrap(),
            large
        );
        let pending_key = format!("{backup}/{}.sealed", Uuid::new_v4());
        let upload = client.initiate(&pending_key).await.unwrap();
        client
            .part(&pending_key, &upload, 1, vec![0; 5 * 1024 * 1024])
            .await
            .unwrap();
        client.abort(&pending_key, &upload).await.unwrap();
        client.abort(&pending_key, &upload).await.unwrap();
        assert!(client.get(&pending_key).await.is_err());
        for key in [&key, &large_key] {
            assert!(client
                .send("DELETE", Some(key), &[], Vec::new(), &[])
                .await
                .unwrap()
                .status()
                .is_success());
        }
        assert!(client
            .send("DELETE", None, &[], Vec::new(), &[])
            .await
            .unwrap()
            .status()
            .is_success());
    }
}
