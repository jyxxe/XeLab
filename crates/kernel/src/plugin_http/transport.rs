//! Fixed-instance mTLS transport. It does not select providers, authorize callers,
//! reserve quota or retry: the existing Broker must do those before dispatch.
use super::*;
use crate::{config::Config, runtime::Runtime};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use futures_util::StreamExt;
use std::{
    fs,
    io::Cursor,
    net::{IpAddr, SocketAddr},
    sync::Arc,
};
use uuid::Uuid;

pub struct InstanceHttp {
    client: reqwest::Client,
    origin: url::Url,
    tls: Arc<rustls::ClientConfig>,
    address: SocketAddr,
    server_name: String,
}

#[derive(Debug)]
struct InstanceVerifier {
    inner: Arc<rustls::client::WebPkiServerVerifier>,
    expected_uri: String,
}
impl rustls::client::danger::ServerCertVerifier for InstanceVerifier {
    fn verify_server_cert(
        &self,
        cert: &rustls::pki_types::CertificateDer<'_>,
        intermediates: &[rustls::pki_types::CertificateDer<'_>],
        name: &rustls::pki_types::ServerName<'_>,
        ocsp: &[u8],
        now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        let verified = self
            .inner
            .verify_server_cert(cert, intermediates, name, ocsp, now)?;
        let identity = x509_parser::parse_x509_certificate(cert.as_ref()).ok()
            .is_some_and(|(_,cert)| cert.subject_alternative_name().ok().flatten()
                .is_some_and(|san| san.value.general_names.iter().any(|name|
                    matches!(name,x509_parser::extensions::GeneralName::URI(uri) if *uri == self.expected_uri))));
        if !identity {
            return Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::ApplicationVerificationFailure,
            ));
        }
        Ok(verified)
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        signed: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, signed)
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        signed: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, signed)
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

pub struct Response {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub bytes: Vec<u8>,
}

/// Created by the trusted caller after durable Broker admission. Never parse
/// this from external headers; its opaque delegation comes from Auth::issue_rpc.
pub struct Delegation<'a> {
    pub token: &'a str,
    pub subject: Uuid,
    pub call: Uuid,
    pub deadline_unix_ms: i64,
    pub idempotency_key: &'a str,
}

/// Origin allocation is trusted platform configuration, never a plugin field.
/// All third-party pages require an origin separate from the trusted login and
/// the embedding shell. The caller must also ensure uniqueness across plugins.
pub struct StaticOrigin {
    origin: String,
    frame_ancestor: String,
}
impl StaticOrigin {
    pub fn for_deployment(
        config: &Config,
        deployment: Uuid,
        shell_origin: Option<&str>,
    ) -> Result<Self> {
        let gateway = &config.value["ui_gateway"];
        if gateway.is_null() {
            return Err(Error::unavailable("UI origin allocation is not configured"));
        }
        if shell_origin.is_some_and(|shell| {
            !gateway["shell_origins"]
                .as_array()
                .is_some_and(|list| list.iter().any(|v| v == shell))
        }) {
            return Err(Error::invalid("UI shell origin is not approved"));
        }
        let base = string(gateway, "base_domain")?;
        let port = unsigned(&gateway["public_port"])?;
        let suffix = if port == 443 {
            String::new()
        } else {
            format!(":{port}")
        };
        let mut policy = Self::new(
            &format!("https://d-{}.{base}{suffix}", deployment.simple()),
            config.origin()?.as_str().trim_end_matches('/'),
            shell_origin,
        )?;
        if shell_origin.is_none() {
            let origins = gateway["shell_origins"]
                .as_array()
                .ok_or_else(|| Error::invalid("UI shell origins are missing"))?
                .iter()
                .map(|value| {
                    exact_origin(
                        value
                            .as_str()
                            .ok_or_else(|| Error::invalid("Invalid UI shell origin"))?,
                    )
                })
                .collect::<Result<Vec<_>>>()?;
            if !origins.is_empty() {
                policy.frame_ancestor = origins.join(" ");
            }
        }
        Ok(policy)
    }
    pub fn origin(&self) -> &str {
        &self.origin
    }
    pub fn new(
        plugin_origin: &str,
        trusted_origin: &str,
        shell_origin: Option<&str>,
    ) -> Result<Self> {
        let plugin = exact_origin(plugin_origin)?;
        let trusted = exact_origin(trusted_origin)?;
        if plugin == trusted {
            return Err(Error::invalid("Plugin pages require an isolated origin"));
        }
        let frame_ancestor = if let Some(shell) = shell_origin {
            let shell = exact_origin(shell)?;
            if shell == plugin {
                return Err(Error::invalid(
                    "Plugin frames cannot share the shell origin",
                ));
            }
            shell
        } else {
            "'none'".into()
        };
        Ok(Self {
            origin: plugin,
            frame_ancestor,
        })
    }
}

fn exact_origin(value: &str) -> Result<String> {
    let url = url::Url::parse(value).map_err(|_| Error::invalid("Invalid isolated origin"))?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || url.origin().ascii_serialization() != value
    {
        return Err(Error::invalid("An exact HTTPS origin is required"));
    }
    Ok(value.into())
}

impl InstanceHttp {
    pub fn from_runtime(
        config: &Config,
        runtime: &Runtime,
        instance: Uuid,
        generation: i64,
    ) -> Result<Self> {
        let address: IpAddr = runtime
            .address(instance)?
            .parse()
            .map_err(|_| Error::invalid("Runtime HTTP address is not an IP"))?;
        let cidr: ipnet::Ipv4Net = config
            .text("runtime", "network_cidr")
            .parse()
            .map_err(|_| Error::invalid("Invalid runtime CIDR"))?;
        if !matches!(address, IpAddr::V4(ip) if cidr.contains(&ip)) || generation < 1 {
            return Err(Error::invalid(
                "Runtime HTTP address or generation is invalid",
            ));
        }
        let identity = config.root().join("kernel-identity");
        let mut pem = fs::read(identity.join("tls.crt"))?;
        pem.extend(fs::read(identity.join("tls.key"))?);
        Self::new(
            instance,
            generation,
            SocketAddr::new(address, 8080),
            &fs::read(config.text("security", "workload_ca_cert_file"))?,
            &pem,
        )
    }

    pub(super) fn new(
        instance: Uuid,
        generation: i64,
        address: SocketAddr,
        ca: &[u8],
        identity: &[u8],
    ) -> Result<Self> {
        let host = format!("i-{instance}-g-{generation}.xelab.local");
        let origin = url::Url::parse(&format!("https://{host}:{}", address.port()))
            .map_err(|_| Error::invalid("Invalid instance origin"))?;
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut roots = rustls::RootCertStore::empty();
        for cert in rustls_pemfile::certs(&mut Cursor::new(ca)) {
            roots
                .add(cert.map_err(|_| Error::invalid("Invalid workload CA"))?)
                .map_err(|_| Error::invalid("Invalid workload CA"))?;
        }
        let verifier = rustls::client::WebPkiServerVerifier::builder_with_provider(
            Arc::new(roots),
            provider.clone(),
        )
        .build()
        .map_err(|_| Error::invalid("Invalid workload CA"))?;
        let certificates = rustls_pemfile::certs(&mut Cursor::new(identity))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|_| Error::invalid("Invalid kernel HTTP identity"))?;
        let private_key = rustls_pemfile::private_key(&mut Cursor::new(identity))
            .map_err(|_| Error::invalid("Invalid kernel HTTP identity"))?
            .ok_or_else(|| Error::invalid("Kernel HTTP key is missing"))?;
        let tls = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|_| Error::unavailable("Cannot configure workload HTTP TLS"))?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(InstanceVerifier {
                inner: verifier,
                expected_uri: format!(
                    "spiffe://xelab.local/instance/{instance}/generation/{generation}"
                ),
            }))
            .with_client_auth_cert(certificates, private_key)
            .map_err(|_| Error::invalid("Invalid kernel HTTP identity"))?;
        let tls = Arc::new(tls);
        let client = reqwest::Client::builder()
            .no_proxy()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .use_preconfigured_tls((*tls).clone())
            .resolve(&host, address)
            .connect_timeout(Duration::from_secs(2))
            .pool_max_idle_per_host(0)
            .build()
            .map_err(|_| Error::unavailable("Cannot build workload HTTP transport"))?;
        Ok(Self {
            client,
            origin,
            tls,
            address,
            server_name: host,
        })
    }

    /// Verify the declared HTTP listener without inventing an undeclared probe
    /// operation or executing any business work during lifecycle readiness.
    pub async fn probe(&self) -> Result<()> {
        let work = async {
            let socket = tokio::net::TcpStream::connect(self.address).await?;
            let name = rustls::pki_types::ServerName::try_from(self.server_name.clone())
                .map_err(|_| Error::invalid("Invalid HTTP workload name"))?;
            let connection = tokio_rustls::TlsConnector::from(self.tls.clone())
                .connect(name, socket)
                .await
                .map_err(|_| Error::unavailable("HTTP workload TLS listener is unavailable"))?;
            drop(connection);
            Ok(())
        };
        tokio::time::timeout(Duration::from_secs(2), work)
            .await
            .map_err(|_| timeout())?
    }

    pub async fn invoke(
        &self,
        op: &Operation,
        headers: &HeaderMap,
        bytes: &[u8],
        delegation: Delegation<'_>,
        remaining: Duration,
    ) -> Result<Response> {
        let deadline = tokio::time::Instant::now() + remaining.min(op.route.timeout);
        check_deadline(deadline)?;
        let mut headers = op.request(headers, bytes)?;
        inject(&mut headers, Some(delegation))?;
        check_deadline(deadline)?;
        let work = async {
            let response = self
                .send(&op.method, &op.path, headers, bytes, deadline)
                .await?;
            let headers = op.response(response.status, &response.headers, &response.bytes)?;
            check_deadline(deadline)?;
            Ok(Response {
                headers,
                ..response
            })
        };
        tokio::time::timeout_at(deadline, work)
            .await
            .map_err(|_| timeout())?
    }

    pub async fn static_page(
        &self,
        route: &Route,
        method: &Method,
        path: &str,
        headers: &HeaderMap,
        origin: &StaticOrigin,
        remaining: Duration,
    ) -> Result<Response> {
        let deadline = tokio::time::Instant::now() + remaining.min(route.timeout);
        check_deadline(deadline)?;
        canonical_path(path)?;
        if route.kind == "http"
            || !route.methods.contains(method)
            || !under(path, &route.prefix)
            || (method != Method::GET && method != Method::HEAD)
        {
            return Err(Error::not_found());
        }
        reject_upgrade(headers)?;
        // Range is deliberately unsupported until its response validation and
        // multipart limits are implemented; do not silently forward it.
        if headers.contains_key("range") {
            return Err(Error::invalid("Static byte ranges are not supported"));
        }
        let headers = allowed_request_headers(headers, false)?;
        check_deadline(deadline)?;
        let work = async {
            let response = self.send(method, path, headers, &[], deadline).await?;
            let headers = static_response(
                method,
                response.status,
                &response.headers,
                &response.bytes,
                origin,
            )?;
            check_deadline(deadline)?;
            Ok(Response {
                headers,
                ..response
            })
        };
        tokio::time::timeout_at(deadline, work)
            .await
            .map_err(|_| timeout())?
    }

    async fn send(
        &self,
        method: &Method,
        path: &str,
        headers: HeaderMap,
        bytes: &[u8],
        deadline: tokio::time::Instant,
    ) -> Result<Response> {
        let response = self
            .open_request(method, path, headers, bytes, deadline)
            .await?;
        if response
            .content_length()
            .is_some_and(|l| l > MAXIMUM_RESPONSE_BYTES as u64)
        {
            return Err(too_large());
        }
        let status = response.status();
        let headers = response.headers().clone();
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| {
                if error.is_timeout() {
                    timeout()
                } else {
                    Error::unavailable("Plugin HTTP response ended before completion")
                }
            })?;
            if chunk.len() > MAXIMUM_RESPONSE_BYTES.saturating_sub(bytes.len()) {
                return Err(too_large());
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(Response {
            status,
            headers,
            bytes,
        })
    }
    pub(crate) async fn open(
        &self,
        op: &Operation,
        headers: &HeaderMap,
        bytes: &[u8],
        context: Delegation<'_>,
        deadline: tokio::time::Instant,
    ) -> Result<reqwest::Response> {
        check_deadline(deadline)?;
        let mut headers = op.request(headers, bytes)?;
        inject(&mut headers, Some(context))?;
        check_deadline(deadline)?;
        self.open_request(&op.method, &op.path, headers, bytes, deadline)
            .await
    }
    async fn open_request(
        &self,
        method: &Method,
        path: &str,
        headers: HeaderMap,
        bytes: &[u8],
        deadline: tokio::time::Instant,
    ) -> Result<reqwest::Response> {
        canonical_path(path)?;
        check_deadline(deadline)?;
        let mut url = self.origin.clone();
        url.set_path(path);
        if url.path() != path {
            return Err(Error::invalid(
                "HTTP target path changed during serialization",
            ));
        }
        let response = self
            .client
            .request(method.clone(), url)
            .headers(headers)
            .body(bytes.to_owned())
            .timeout(deadline.saturating_duration_since(tokio::time::Instant::now()))
            .send()
            .await
            .map_err(|error| {
                if error.is_timeout() {
                    timeout()
                } else {
                    Error::unavailable(
                        "Plugin HTTP request failed; dispatch outcome may be unknown",
                    )
                }
            })?;
        check_deadline(deadline)?;
        Ok(response)
    }
}

pub(crate) fn static_response(
    method: &Method,
    status: StatusCode,
    headers: &HeaderMap,
    bytes: &[u8],
    origin: &StaticOrigin,
) -> Result<HeaderMap> {
    reject_upgrade(headers)?;
    if ![
        StatusCode::OK,
        StatusCode::NOT_MODIFIED,
        StatusCode::NOT_FOUND,
    ]
    .contains(&status)
        || headers.contains_key("location")
        || headers.contains_key("set-cookie")
    {
        return Err(Error::invalid(
            "Static response attempted a redirect, cookie or undeclared status",
        ));
    }
    if bytes.len() > MAXIMUM_RESPONSE_BYTES {
        return Err(too_large());
    }
    if (method == Method::HEAD || status == StatusCode::NOT_MODIFIED) && !bytes.is_empty() {
        return Err(Error::invalid("Static HEAD or 304 must be bodyless"));
    }
    if unique(headers, "content-encoding")?.is_some_and(|v| v != "identity") {
        return Err(Error::invalid("Encoded static responses are not admitted"));
    }
    let mut result = HeaderMap::new();
    if status != StatusCode::NOT_MODIFIED {
        let media = unique(headers, "content-type")?
            .ok_or_else(|| Error::invalid("Static content type is missing"))?;
        let name = media
            .to_str()
            .map_err(|_| Error::invalid("Invalid static media type"))?
            .split(';')
            .next()
            .unwrap()
            .trim();
        if ![
            "text/html",
            "text/css",
            "text/plain",
            "application/javascript",
            "text/javascript",
            "application/json",
            "image/png",
            "image/jpeg",
            "image/gif",
            "image/webp",
            "image/svg+xml",
            "image/x-icon",
            "font/woff",
            "font/woff2",
        ]
        .contains(&name)
        {
            return Err(Error::invalid("Unapproved static media type"));
        }
        result.insert("content-type", media.clone());
    }
    for name in ["etag", "last-modified"] {
        if let Some(value) = unique(headers, name)? {
            result.insert(HeaderName::from_static(name), value.clone());
        }
    }
    let csp = format!("default-src 'none'; script-src 'self'; style-src 'self'; font-src 'self'; img-src 'self' blob:; media-src 'self' blob:; connect-src 'none'; worker-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors {}",origin.frame_ancestor);
    result.insert(
        "content-security-policy",
        HeaderValue::from_str(&csp).map_err(|_| Error::invalid("Invalid static CSP"))?,
    );
    result.insert("cache-control", HeaderValue::from_static("no-store"));
    result.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    result.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    result.insert(
        "permissions-policy",
        HeaderValue::from_static("camera=(), microphone=(), geolocation=(), payment=(), usb=()"),
    );
    Ok(result)
}

fn inject(headers: &mut HeaderMap, delegation: Option<Delegation<'_>>) -> Result<()> {
    if let Some(d) = delegation {
        if d.token.len() < 32 || d.token.len() > 512 {
            return Err(Error::invalid("Invalid admitted HTTP delegation"));
        }
        headers.insert(
            "x-xelab-delegation",
            HeaderValue::from_str(d.token)
                .map_err(|_| Error::invalid("Invalid admitted HTTP delegation"))?,
        );
        headers.insert(
            "x-xelab-subject",
            HeaderValue::from_str(&d.subject.to_string()).unwrap(),
        );
        if d.deadline_unix_ms <= chrono::Utc::now().timestamp_millis()
            || d.idempotency_key.is_empty()
            || d.idempotency_key.chars().count() > 128
        {
            return Err(Error::invalid(
                "Admitted HTTP context deadline or business key is invalid",
            ));
        }
        headers.insert(
            "x-xelab-call-id",
            HeaderValue::from_str(&d.call.to_string()).unwrap(),
        );
        headers.insert(
            "x-xelab-deadline-unix-ms",
            HeaderValue::from_str(&d.deadline_unix_ms.to_string()).unwrap(),
        );
        headers.insert(
            "x-xelab-idempotency-key",
            HeaderValue::from_str(&URL_SAFE_NO_PAD.encode(d.idempotency_key.as_bytes())).unwrap(),
        );
    }
    Ok(())
}
fn timeout() -> Error {
    Error::new(
        "DEADLINE_EXCEEDED",
        "Plugin HTTP deadline exceeded",
        StatusCode::GATEWAY_TIMEOUT,
    )
}
fn check_deadline(deadline: tokio::time::Instant) -> Result<()> {
    if deadline <= tokio::time::Instant::now() {
        Err(timeout())
    } else {
        Ok(())
    }
}
