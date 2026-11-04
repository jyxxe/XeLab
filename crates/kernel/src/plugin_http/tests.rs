use super::*;
use serde_json::json;

fn fixture() -> (Value, Vec<u8>) {
    let schema = json!({"type":"object","properties":{"text":{"type":"string","maxLength":100}},"required":["text"],"additionalProperties":false});
    let doc = json!({"openapi":"3.1.0","info":{"title":"Echo","version":"1.0.0"},"paths":{
        "/plugins/org.xelab.http-echo/echo":{"post":{"operationId":"echo","x-permission":"plugin.http.invoke",
            "requestBody":{"required":true,"content":{"application/json":{"schema":schema}}},
            "parameters":[{"in":"header","name":"X-Client-Label","schema":{"type":"string","maxLength":16}}],
            "responses":{"200":{"description":"Echo","content":{"application/json":{"schema":schema}}},
                "400":{"description":"Error","content":{"application/json":{"schema":{"type":"object"}}}}}}}
    }});
    let bytes = serde_json::to_vec(&doc).unwrap();
    let manifest = json!({"plugin_id":"org.xelab.http-echo","routes":[
        {"path_prefix":"/plugins/org.xelab.http-echo/","methods":["POST"],"kind":"http","required_action":"plugin.http.invoke","maximum_body_bytes":1024,"timeout_ms":1000,"http_contract_digest":digest(&bytes)},
        {"path_prefix":"/ui/org.xelab.http-echo/","methods":["GET","HEAD"],"kind":"static","required_action":"plugin.ui.open","maximum_body_bytes":0,"timeout_ms":1000,"authentication":"public"}
    ]});
    (manifest, bytes)
}
fn changed_doc(mut edit: impl FnMut(&mut Value)) -> (Value, Vec<u8>) {
    let (mut manifest, bytes) = fixture();
    let mut doc: Value = serde_json::from_slice(&bytes).unwrap();
    edit(&mut doc);
    let bytes = serde_json::to_vec(&doc).unwrap();
    manifest["routes"][0]["http_contract_digest"] = json!(digest(&bytes));
    (manifest, bytes)
}
#[test]
fn raw_digest_and_exact_operation_are_binding() {
    let (manifest, bytes) = fixture();
    let registry = Registry::compile(&manifest, Some(&bytes)).unwrap();
    let op = registry
        .resolve(&Method::POST, "/plugins/org.xelab.http-echo/echo")
        .unwrap();
    assert_eq!(op.id, "echo");
    assert_eq!(op.contract_digest, digest(&bytes));
    for (m, p) in [
        (Method::GET, "/plugins/org.xelab.http-echo/echo"),
        (Method::POST, "/plugins/org.xelab.http-echo/not-declared"),
    ] {
        assert!(registry.resolve(&m, p).is_err());
    }
    let mut tampered = bytes.clone();
    tampered.push(b' ');
    assert!(Registry::compile(&manifest, Some(&tampered)).is_err());
    assert!(Registry::compile(&manifest, None).is_err());
}
#[test]
fn namespace_path_and_protocol_ambiguities_are_closed() {
    for path in [
        "//plugins/x/",
        "/plugins/x/../secret",
        "/plugins/x/./secret",
        "/plugins/x/%2e%2e/",
        "/plugins/x/%2F",
        "/plugins/x/%252F",
        "/plugins/x/\\secret",
        "/plugins/x/a?b",
        "/plugins/x/a#b",
        "/plugins/x/{id}",
        "/plugins/x/\r\n",
        "https://evil.test/a",
        "/plugins/x/é",
        "/plugins/x/\"",
        "/plugins/x/<",
        "/plugins/x/>",
        "/plugins/x/`",
    ] {
        assert!(canonical_path(path).is_err(), "{path}");
    }
    let (manifest, bytes) = fixture();
    let registry = Registry::compile(&manifest, Some(&bytes)).unwrap();
    assert!(registry
        .resolve_static(&Method::GET, "/ui/org.xelab.http-echo-evil/a")
        .is_err());
    assert!(registry
        .resolve_static(&Method::POST, "/ui/org.xelab.http-echo/a")
        .is_err());
    assert!(registry
        .resolve_static(&Method::HEAD, "/ui/org.xelab.http-echo/index.html")
        .is_ok());
    for name in ["upgrade", "sec-websocket-key", "last-event-id"] {
        let mut headers = HeaderMap::new();
        headers.insert(name, HeaderValue::from_static("x"));
        assert!(reject_upgrade(&headers).is_err());
    }
}
#[test]
fn malformed_contracts_cannot_expand_route_or_authority() {
    for edit in [
        |d: &mut Value| {
            d["paths"]["/plugins/org.xelab.http-echo/echo"]["post"]["x-permission"] =
                json!("account.manage")
        },
        |d: &mut Value| {
            d["paths"]["/plugins/org.xelab.http-echo/echo"]["post"]["requestBody"]["content"]
                ["application/json"]["schema"] = json!({"$ref":"https://evil.test/schema"})
        },
        |d: &mut Value| d["servers"] = json!([{"url":"https://evil.test"}]),
        |d: &mut Value| {
            d["paths"]["/plugins/org.xelab.http-echo/echo"]["post"]["parameters"][0]["name"] =
                json!("X-Xelab-Delegation")
        },
        |d: &mut Value| {
            d["paths"]["/plugins/org.xelab.http-echo/echo"]["post"]["responses"]["302"] =
                json!({"description":"Redirect"})
        },
    ] {
        let (manifest, bytes) = changed_doc(edit);
        assert!(Registry::compile(&manifest, Some(&bytes)).is_err());
    }
    let (mut manifest, bytes) = fixture();
    manifest["routes"][0]["path_prefix"] = json!("/api/platform/v1/");
    assert!(Registry::compile(&manifest, Some(&bytes)).is_err());
    let (mut manifest, bytes) = fixture();
    let duplicate = manifest["routes"][0].clone();
    manifest["routes"].as_array_mut().unwrap().push(duplicate);
    assert!(Registry::compile(&manifest, Some(&bytes)).is_err());
    for edit in [
        |d: &mut Value| {
            let op = &mut d["paths"]["/plugins/org.xelab.http-echo/echo"]["post"];
            op["responses"]["0200"] = op["responses"]["200"].clone();
        },
        |d: &mut Value| {
            d["paths"]["/plugins/org.xelab.http-echo/echo"]["post"]["responses"]["200"]["headers"] = json!({"X-Label":{"schema":{"type":"string"}},"x-label":{"schema":{"type":"string"}}})
        },
    ] {
        let (manifest, bytes) = changed_doc(edit);
        assert!(Registry::compile(&manifest, Some(&bytes)).is_err());
    }
    let (mut manifest, bytes) = changed_doc(|d| {
        d["paths"]["/plugins/org.xelab.http-echo/echo"]["post"]["x-permission"] =
            json!("plugin.read")
    });
    manifest["routes"][0]["required_action"] = json!("plugin.read");
    assert!(Registry::compile(&manifest, Some(&bytes)).is_err());
}

#[test]
fn signed_artifact_loader_preserves_exact_file_and_refuses_symlinks() {
    let directory = tempfile::tempdir().unwrap();
    let builder = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/http/build_artifact.py");
    let result = std::process::Command::new("python3")
        .arg(builder)
        .arg("--publisher-id")
        .arg(uuid::Uuid::new_v4().to_string())
        .arg("--output")
        .arg(directory.path())
        .output()
        .unwrap();
    assert!(result.status.success());
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(directory.path().join("xelab/plugin.json")).unwrap())
            .unwrap();
    validate("Manifest", &manifest).unwrap();
    for skill in manifest["skills"].as_array().unwrap() {
        validate("SkillDefinition", skill).unwrap();
    }
    let registry = from_signed_artifact(&manifest, directory.path()).unwrap();
    assert_eq!(
        registry.bind_skill(&manifest["skills"][0]).unwrap().id,
        "echoMessage"
    );
    let mut altered_manifest = manifest.clone();
    altered_manifest["skills"][0]["output_schema"] = json!({"type":"string"});
    assert!(from_signed_artifact(&altered_manifest, directory.path()).is_err());
    assert_eq!(
        from_signed_artifact(&manifest, directory.path())
            .unwrap()
            .operations
            .len(),
        8
    );
    let path = directory.path().join("xelab/http/openapi.json");
    let original = std::fs::read(&path).unwrap();
    let mut altered = original.clone();
    altered.push(b' ');
    std::fs::write(&path, altered).unwrap();
    assert!(from_signed_artifact(&manifest, directory.path()).is_err());
    let other = directory.path().join("outside.json");
    std::fs::write(&other, original).unwrap();
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(other, path).unwrap();
    assert!(from_signed_artifact(&manifest, directory.path()).is_err());
}

#[test]
fn static_origin_is_derived_from_config_and_deployment() {
    let mut value = serde_json::to_value(
        toml::from_str::<toml::Value>(include_str!(
            "../../../../docs/contracts/examples/kernel.toml"
        ))
        .unwrap(),
    )
    .unwrap();
    value["ui_gateway"] = json!({"base_domain":"plugins.example","public_port":9443,"shell_origins":["https://desktop.example"]});
    let config = crate::config::Config::from_value(value).unwrap();
    let deployment = uuid::Uuid::new_v4();
    let origin = transport::StaticOrigin::for_deployment(
        &config,
        deployment,
        Some("https://desktop.example"),
    )
    .unwrap();
    assert_eq!(
        origin.origin(),
        format!("https://d-{}.plugins.example:9443", deployment.simple())
    );
    assert!(transport::StaticOrigin::for_deployment(
        &config,
        deployment,
        Some("https://forged.example")
    )
    .is_err());
}
#[test]
fn only_declared_safe_headers_and_strict_json_survive() {
    let (manifest, bytes) = fixture();
    let registry = Registry::compile(&manifest, Some(&bytes)).unwrap();
    let op = registry
        .resolve(&Method::POST, "/plugins/org.xelab.http-echo/echo")
        .unwrap();
    let mut headers = HeaderMap::new();
    for (name, value) in [
        ("content-type", "application/json"),
        ("authorization", "Bearer secret"),
        ("cookie", "__Host-xelab_auth=secret"),
        ("x-xelab-subject", "forged"),
        ("x-xelab-delegation", "forged"),
        ("x-user-id", "forged"),
        ("x-forwarded-host", "evil.test"),
        ("x-client-label", "test"),
        ("x-unknown", "no"),
        ("connection", "keep-alive"),
    ] {
        headers.insert(
            HeaderName::from_bytes(name.as_bytes()).unwrap(),
            HeaderValue::from_str(value).unwrap(),
        );
    }
    let filtered = op.request(&headers, br#"{"text":"hi"}"#).unwrap();
    assert_eq!(filtered.len(), 2);
    assert_eq!(filtered["x-client-label"], "test");
    for body in [
        br#"{"text":"a","text":"b"}"#.as_slice(),
        br#"{"text":"a","extra":1}"#,
        br#"{"text":1}"#,
    ] {
        assert!(op.request(&headers, body).is_err());
    }
    headers.append("x-client-label", HeaderValue::from_static("duplicate"));
    assert!(op.request(&headers, br#"{"text":"hi"}"#).is_err());
    let mut response = HeaderMap::new();
    response.insert("content-type", HeaderValue::from_static("application/json"));
    assert!(op
        .response(StatusCode::OK, &response, br#"{"text":"hi"}"#)
        .is_ok());
    assert!(op
        .response(StatusCode::CREATED, &response, br#"{"text":"hi"}"#)
        .is_err());
    for name in ["set-cookie", "location"] {
        let mut bad = response.clone();
        bad.insert(name, HeaderValue::from_static("evil"));
        assert!(op
            .response(StatusCode::OK, &bad, br#"{"text":"hi"}"#)
            .is_err());
    }
}
#[test]
fn static_origin_cookie_redirect_and_content_boundaries() {
    use transport::{static_response, StaticOrigin};
    for (plugin, shell) in [
        ("https://kernel.example", None),
        ("https://plugin.example/", None),
        ("http://plugin.example", None),
        ("https://plugin.example", Some("https://plugin.example")),
    ] {
        assert!(StaticOrigin::new(plugin, "https://kernel.example", shell).is_err());
    }
    let origin = StaticOrigin::new(
        "https://plugin.example",
        "https://kernel.example",
        Some("https://shell.example"),
    )
    .unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        "content-type",
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.insert("access-control-allow-origin", HeaderValue::from_static("*"));
    let out = static_response(&Method::GET, StatusCode::OK, &headers, b"hello", &origin).unwrap();
    assert!(out["content-security-policy"]
        .to_str()
        .unwrap()
        .contains("frame-ancestors https://shell.example"));
    assert!(!out.contains_key("access-control-allow-origin"));
    assert!(static_response(&Method::HEAD, StatusCode::OK, &headers, b"hello", &origin).is_err());
    for name in ["set-cookie", "location", "upgrade"] {
        let mut bad = headers.clone();
        bad.insert(name, HeaderValue::from_static("evil"));
        assert!(static_response(&Method::GET, StatusCode::OK, &bad, b"hello", &origin).is_err());
    }
    headers.insert(
        "content-type",
        HeaderValue::from_static("application/octet-stream"),
    );
    assert!(static_response(&Method::GET, StatusCode::OK, &headers, b"hello", &origin).is_err());
}

#[tokio::test]
async fn real_reference_server_uses_pinned_mtls_and_sanitized_transport() {
    use rcgen::{
        BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, KeyPair, SanType,
    };
    use std::{fs, process::Stdio};
    use tokio::io::{AsyncBufReadExt, BufReader};
    use transport::{Delegation, InstanceHttp, StaticOrigin};
    crate::init_crypto();
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let key = KeyPair::generate().unwrap();
    let mut params = CertificateParams::new(vec!["test-ca".into()]).unwrap();
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "test-workload-ca");
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca = params.self_signed(&key).unwrap();
    let instance = uuid::Uuid::new_v4();
    let host = format!("i-{instance}-g-1.xelab.local");
    let leaf = KeyPair::generate().unwrap();
    let mut params = CertificateParams::new(vec![host]).unwrap();
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.subject_alt_names.push(SanType::URI(
        format!("spiffe://xelab.local/instance/{instance}/generation/1")
            .try_into()
            .unwrap(),
    ));
    let cert = params.signed_by(&leaf, &ca, &key).unwrap();
    fs::write(root.join("tls.crt"), cert.pem()).unwrap();
    fs::write(root.join("tls.key"), leaf.serialize_pem()).unwrap();
    fs::write(root.join("ca.crt"), ca.pem()).unwrap();
    let leaf = KeyPair::generate().unwrap();
    let mut params = CertificateParams::new(vec!["kernel.xelab.local".into()]).unwrap();
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    params.subject_alt_names.push(SanType::URI(
        "spiffe://xelab.local/kernel".try_into().unwrap(),
    ));
    let kernel = params.signed_by(&leaf, &ca, &key).unwrap();
    let identity = format!("{}{}", kernel.pem(), leaf.serialize_pem());
    let script = "import sys\nsys.path.insert(0,sys.argv[1])\nfrom http_server import State,KernelHTTPServer\ns=State();s.initialize()\nh=KernelHTTPServer(('127.0.0.1',0),s,sys.argv[2])\nprint(h.server_address[1],flush=True)\nh.serve_forever()\n";
    let examples = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/http");
    let mut server = tokio::process::Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(&examples)
        .arg(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(server.stdout.take().unwrap()).lines();
    let port = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .parse::<u16>()
        .unwrap();
    let socket = format!("127.0.0.1:{port}").parse().unwrap();
    let client = InstanceHttp::new(
        instance,
        1,
        socket,
        ca.pem().as_bytes(),
        identity.as_bytes(),
    )
    .unwrap();
    let bytes = fs::read(examples.join("openapi.json")).unwrap();
    let manifest = json!({"plugin_id":"org.xelab.http-echo","routes":[
        {"path_prefix":"/plugins/org.xelab.http-echo/","methods":["GET","HEAD","POST","PUT","PATCH","DELETE"],"kind":"http","required_action":"plugin.http.invoke","maximum_body_bytes":8192,"timeout_ms":10000,"http_contract_digest":digest(&bytes)},
        {"path_prefix":"/ui/org.xelab.http-echo/","methods":["GET","HEAD"],"kind":"static","required_action":"plugin.ui.open","maximum_body_bytes":0,"timeout_ms":10000,"authentication":"public"}
    ]});
    let registry = Registry::compile(&manifest, Some(&bytes)).unwrap();
    let op = &registry.operations[0];
    let mut headers = HeaderMap::new();
    for (name, value) in [
        ("content-type", "application/json"),
        ("authorization", "Bearer external-secret"),
        ("cookie", "__Host-xelab_auth=external-secret"),
        ("x-xelab-subject", "forged"),
        ("x-xelab-delegation", "forged"),
    ] {
        headers.insert(
            HeaderName::from_bytes(name.as_bytes()).unwrap(),
            HeaderValue::from_str(value).unwrap(),
        );
    }
    let response = client
        .invoke(
            op,
            &headers,
            br#"{"message":"hello"}"#,
            Delegation {
                token: "01234567890123456789012345678901",
                subject: uuid::Uuid::new_v4(),
                call: uuid::Uuid::new_v4(),
                deadline_unix_ms: (chrono::Utc::now() + chrono::Duration::seconds(30))
                    .timestamp_millis(),
                idempotency_key: "business-key",
            },
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<Value>(&response.bytes).unwrap(),
        json!({"message":"hello"})
    );
    assert_eq!(response.headers["cache-control"], "no-store");
    let wrong_generation = InstanceHttp::new(
        instance,
        2,
        socket,
        ca.pem().as_bytes(),
        identity.as_bytes(),
    )
    .unwrap();
    assert!(wrong_generation
        .invoke(
            op,
            &headers,
            br#"{"message":"hello"}"#,
            Delegation {
                token: "01234567890123456789012345678901",
                subject: uuid::Uuid::new_v4(),
                call: uuid::Uuid::new_v4(),
                deadline_unix_ms: (chrono::Utc::now() + chrono::Duration::seconds(30))
                    .timestamp_millis(),
                idempotency_key: "business-key",
            },
            Duration::from_secs(5)
        )
        .await
        .is_err());
    let origin = StaticOrigin::new(
        "https://plugin.example",
        "https://kernel.example",
        Some("https://shell.example"),
    )
    .unwrap();
    let path = "/ui/org.xelab.http-echo/index.html";
    let route = registry.resolve_static(&Method::GET, path).unwrap();
    let page = client
        .static_page(
            route,
            &Method::GET,
            path,
            &headers,
            &origin,
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert_eq!(page.status, StatusCode::OK);
    assert!(std::str::from_utf8(&page.bytes)
        .unwrap()
        .contains("HTTP reference"));
    let head = client
        .static_page(
            route,
            &Method::HEAD,
            path,
            &headers,
            &origin,
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert!(head.bytes.is_empty());
    headers.insert("if-none-match", page.headers["etag"].clone());
    let cached = client
        .static_page(
            route,
            &Method::GET,
            path,
            &headers,
            &origin,
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert_eq!(cached.status, StatusCode::NOT_MODIFIED);
    assert!(cached.bytes.is_empty());
    assert!(client
        .static_page(
            route,
            &Method::GET,
            "/ui/org.xelab.http-echo/%2e%2e/a",
            &headers,
            &origin,
            Duration::from_secs(5)
        )
        .await
        .is_err());
    headers.insert("range", HeaderValue::from_static("bytes=0-10"));
    assert!(client
        .static_page(
            route,
            &Method::GET,
            path,
            &headers,
            &origin,
            Duration::from_secs(5)
        )
        .await
        .is_err());
    let leaf = KeyPair::generate().unwrap();
    let mut params = CertificateParams::new(vec!["foreign.xelab.local".into()]).unwrap();
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    params.subject_alt_names.push(SanType::URI(
        "spiffe://xelab.local/instance/00000000-0000-4000-8000-000000000002/generation/1"
            .try_into()
            .unwrap(),
    ));
    let foreign = params.signed_by(&leaf, &ca, &key).unwrap();
    let foreign_identity = format!("{}{}", foreign.pem(), leaf.serialize_pem());
    let foreign_client = InstanceHttp::new(
        instance,
        1,
        socket,
        ca.pem().as_bytes(),
        foreign_identity.as_bytes(),
    )
    .unwrap();
    assert!(foreign_client
        .invoke(
            op,
            &headers,
            br#"{"message":"hello"}"#,
            Delegation {
                token: "01234567890123456789012345678901",
                subject: uuid::Uuid::new_v4(),
                call: uuid::Uuid::new_v4(),
                deadline_unix_ms: (chrono::Utc::now() + chrono::Duration::seconds(30))
                    .timestamp_millis(),
                idempotency_key: "business-key",
            },
            Duration::from_secs(5)
        )
        .await
        .is_err());
    // A CA-valid certificate with the correct DNS name but a different SPIFFE
    // generation must fail during TLS, before any delegation is transmitted.
    for (uri, expired) in [
        (
            format!("spiffe://xelab.local/instance/{instance}/generation/2"),
            false,
        ),
        (
            format!("spiffe://xelab.local/instance/{instance}/generation/1"),
            true,
        ),
    ] {
        let leaf = KeyPair::generate().unwrap();
        let mut params =
            CertificateParams::new(vec![format!("i-{instance}-g-1.xelab.local")]).unwrap();
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params
            .subject_alt_names
            .push(SanType::URI(uri.try_into().unwrap()));
        if expired {
            params.not_after = time::OffsetDateTime::now_utc() - time::Duration::minutes(1);
        }
        let cert = params.signed_by(&leaf, &ca, &key).unwrap();
        fs::write(root.join("tls.crt"), cert.pem()).unwrap();
        fs::write(root.join("tls.key"), leaf.serialize_pem()).unwrap();
        assert!(client
            .invoke(
                op,
                &headers,
                br#"{"message":"hello"}"#,
                Delegation {
                    token: "01234567890123456789012345678901",
                    subject: uuid::Uuid::new_v4(),
                    call: uuid::Uuid::new_v4(),
                    deadline_unix_ms: (chrono::Utc::now() + chrono::Duration::seconds(30))
                        .timestamp_millis(),
                    idempotency_key: "business-key",
                },
                Duration::from_secs(5)
            )
            .await
            .is_err());
    }
    server.kill().await.unwrap();
}

#[test]
fn http_stream_and_fixed_method_candidates_keep_distinct_signed_schema_authorities() {
    let directory = tempfile::tempdir().unwrap();
    let result = std::process::Command::new("python3")
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/http/build_artifact.py"),
        )
        .arg("--publisher-id")
        .arg(uuid::Uuid::new_v4().to_string())
        .arg("--output")
        .arg(directory.path())
        .output()
        .unwrap();
    assert!(result.status.success());
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(directory.path().join("xelab/plugin.json")).unwrap())
            .unwrap();
    let registry = from_signed_artifact(&manifest, directory.path()).unwrap();
    let definitions = manifest["skills"].as_array().unwrap();
    assert_eq!(definitions.len(), 7);
    let stream = registry.bind_skill(&definitions[1]).unwrap();
    assert_eq!(stream.id, "streamMessages");
    assert_eq!(
        stream.responses[&200].media_type.as_deref(),
        Some("text/event-stream")
    );
    assert_ne!(
        stream.responses[&200].schema.as_ref().unwrap().value(),
        &definitions[1]["stream_item_schema"]
    );
    for (index, method, status) in [
        (2, Method::GET, 200),
        (3, Method::PUT, 201),
        (4, Method::PATCH, 200),
        (5, Method::DELETE, 204),
        (6, Method::HEAD, 200),
    ] {
        let operation = registry.bind_skill(&definitions[index]).unwrap();
        assert_eq!(operation.method, method);
        assert!(operation.responses.contains_key(&status));
    }
    let mut altered = manifest.clone();
    altered["skills"][1]["stream_item_schema"] = Value::Null;
    assert!(from_signed_artifact(&altered, directory.path()).is_err());
    let mut altered = manifest.clone();
    altered["skills"][1]["stream_item_schema"] =
        json!({"$ref":"https://outside.invalid/item.schema.json"});
    assert!(from_signed_artifact(&altered, directory.path()).is_err());
    let mut altered = manifest.clone();
    altered["skills"][1]["output_schema"] = json!({"type":"string"});
    assert!(from_signed_artifact(&altered, directory.path()).is_err());
    let mut altered = manifest.clone();
    altered["skills"][1]["stream_item_schema"] = json!({"type":"integer"});
    assert!(from_signed_artifact(&altered, directory.path()).is_err());
    let head = registry.bind_skill(&definitions[6]).unwrap();
    assert!(head
        .response(StatusCode::OK, &HeaderMap::new(), b"unverifiable body")
        .is_err());
    let raw = std::fs::read(directory.path().join("xelab/http/openapi.json")).unwrap();
    let mut doc: Value = serde_json::from_slice(&raw).unwrap();
    doc["paths"]["/plugins/org.xelab.http-echo/read"]["head"]["responses"]["200"]["content"] =
        json!({"application/json":{"schema":{"type":"object"}}});
    let raw = serde_json::to_vec(&doc).unwrap();
    let mut head_claim = manifest.clone();
    head_claim["routes"][0]["http_contract_digest"] = json!(digest(&raw));
    let error = Registry::compile(&head_claim, Some(&raw))
        .err()
        .expect("HEAD cannot promise an absent JSON output");
    assert_eq!(
        error.message,
        "Bodyless HTTP response cannot declare content"
    );
    // A false extra guard must not make a merely present then.payload
    // schema look authoritative for all item frames.
    let original: Value = serde_json::from_slice(
        &std::fs::read(directory.path().join("xelab/http/openapi.json")).unwrap(),
    )
    .unwrap();
    for variant in 0..5 {
        let mut doc = original.clone();
        let branches = doc["paths"]["/plugins/org.xelab.http-echo/events"]["post"]["responses"]
            ["200"]["content"]["text/event-stream"]["schema"]["allOf"]
            .as_array_mut()
            .unwrap();
        let branch = branches
            .iter_mut()
            .find(|branch| branch["if"]["properties"]["kind"]["const"] == "item")
            .unwrap();
        match variant {
            0 => branch["if"]["properties"]["kind"]["minLength"] = json!(5),
            1 => branch["if"]["properties"]["kind"]["enum"] = json!(["completed"]),
            2 => branch["if"]["type"] = json!("array"),
            3 => branch["if"]["required"] = json!(["kind", "absent"]),
            _ => branch["if"]["properties"]["absent"] = json!({"const":true}),
        }
        let raw = serde_json::to_vec(&doc).unwrap();
        let hash = digest(&raw);
        let mut claimed = manifest.clone();
        claimed["routes"][0]["http_contract_digest"] = json!(hash);
        for candidate in claimed["skills"].as_array_mut().unwrap() {
            candidate["target"]["http_contract_digest"] = json!(hash);
        }
        let error = Registry::compile(&claimed, Some(&raw))
            .err()
            .expect("Conditional false guards must not attest the Skill payload schema");
        assert_eq!(
            error.message,
            "HTTP Skill schemas must match its fixed signed operation and success response",
            "variant {variant}"
        );
    }
    let mut altered = definitions[1].clone();
    altered["target"]["http_contract_digest"] = json!(digest(b"different stream"));
    assert!(registry.bind_skill(&altered).is_err());
}
