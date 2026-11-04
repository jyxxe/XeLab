//! Signed declarative HTTP policy. This module deliberately has no public
//! gateway handler: admission and authority remain the Broker's responsibility.
use crate::{
    artifact::read_image_file,
    contracts::{compile_business, digest, strict_json, string, unsigned, validate},
    error::{Error, Result},
};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    time::Duration,
};

pub mod transport;

pub const MAXIMUM_CONTRACT_BYTES: usize = 4 * 1024 * 1024;
pub const MAXIMUM_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Route {
    pub prefix: String,
    pub methods: Vec<Method>,
    pub action: String,
    pub maximum_body_bytes: usize,
    pub timeout: Duration,
    pub public: bool,
    pub kind: String,
}

pub struct Schema {
    value: Value,
    validator: jsonschema::JSONSchema,
}
impl Schema {
    fn new(value: &Value) -> Result<Self> {
        Ok(Self {
            value: value.clone(),
            validator: compile_business(value)?,
        })
    }
    pub fn value(&self) -> &Value {
        &self.value
    }
    pub(crate) fn check(&self, value: &Value) -> Result<()> {
        if self.validator.is_valid(value) {
            Ok(())
        } else {
            Err(Error::invalid(
                "HTTP value does not match its signed schema",
            ))
        }
    }
}

pub struct ResponseSpec {
    pub media_type: Option<String>,
    pub schema: Option<Schema>,
    headers: HashMap<HeaderName, Schema>,
}

pub struct Operation {
    pub id: String,
    pub path: String,
    pub method: Method,
    pub route: Route,
    pub contract_digest: String,
    pub input: Option<Schema>,
    request_headers: HashMap<HeaderName, (bool, Schema)>,
    pub responses: BTreeMap<u16, ResponseSpec>,
}

pub struct Registry {
    pub operations: Vec<Operation>,
    pub static_routes: Vec<Route>,
    plugin: String,
    candidates: Vec<Value>,
}

/// Only call after the OCI image and Manifest signature have been verified.
/// No file supplied by a request or downloaded from an OpenAPI URL is read.
pub fn from_signed_artifact(manifest: &Value, root: &Path) -> Result<Registry> {
    let has_http = manifest["routes"]
        .as_array()
        .is_some_and(|r| r.iter().any(|r| r["kind"] == "http"));
    let bytes = if has_http {
        Some(read_image_file(
            root,
            "xelab/http/openapi.json",
            MAXIMUM_CONTRACT_BYTES,
        )?)
    } else {
        None
    };
    Registry::compile(manifest, bytes.as_deref())
}

impl Registry {
    pub fn compile(manifest: &Value, raw: Option<&[u8]>) -> Result<Self> {
        let plugin = string(manifest, "plugin_id")?;
        // Reuse the published identifier schema, rather than interpolating an
        // unvalidated ID into a route namespace.
        validate(
            "HttpSkillTarget",
            &serde_json::json!({
                "kind":"http", "plugin_id":plugin, "http_operation_id":"validation",
                "http_contract_digest":format!("sha256:{}", "0".repeat(64))
            }),
        )?;
        let mut http_routes = Vec::new();
        let mut static_routes = Vec::new();
        let routes = manifest["routes"]
            .as_array()
            .ok_or_else(|| Error::invalid("Signed routes must be an array"))?;
        for value in routes {
            validate("RouteSpec", value)?;
            let prefix = string(value, "path_prefix")?;
            canonical_path(prefix)?;
            if !prefix.starts_with(&format!("/plugins/{plugin}/"))
                && !prefix.starts_with(&format!("/ui/{plugin}/"))
            {
                return Err(Error::invalid("HTTP route escapes its plugin namespace"));
            }
            let methods = value["methods"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| Method::from_bytes(v.as_str().unwrap().as_bytes()).unwrap())
                .collect::<Vec<_>>();
            if methods
                .iter()
                .enumerate()
                .any(|(i, m)| methods[..i].contains(m))
            {
                return Err(Error::invalid("Repeated HTTP route method"));
            }
            let route = Route {
                prefix: prefix.to_owned(),
                methods,
                action: string(value, "required_action")?.to_owned(),
                maximum_body_bytes: unsigned(&value["maximum_body_bytes"])? as usize,
                timeout: Duration::from_millis(unsigned(&value["timeout_ms"])?),
                public: value["authentication"] == "public",
                kind: string(value, "kind")?.to_owned(),
            };
            if route.kind == "http" {
                if route.public || !prefix.starts_with(&format!("/plugins/{plugin}/")) {
                    return Err(Error::invalid(
                        "Business HTTP routes must require user authority",
                    ));
                }
                if route.action != "plugin.http.invoke"
                    && !route.action.starts_with(&format!("{plugin}."))
                {
                    return Err(Error::invalid(
                        "Business HTTP must use its own namespaced action or plugin.http.invoke",
                    ));
                }
                http_routes.push((route, string(value, "http_contract_digest")?.to_owned()));
            } else {
                if route.maximum_body_bytes != 0
                    || route
                        .methods
                        .iter()
                        .any(|m| m != Method::GET && m != Method::HEAD)
                {
                    return Err(Error::invalid(
                        "Static routes only admit bodyless GET and HEAD",
                    ));
                }
                static_routes.push(route);
            }
        }
        let all_routes = http_routes
            .iter()
            .map(|(r, _)| r)
            .chain(static_routes.iter())
            .collect::<Vec<_>>();
        for (i, route) in all_routes.iter().enumerate() {
            if all_routes[..i].iter().any(|other| {
                (under(&route.prefix, &other.prefix) || under(&other.prefix, &route.prefix))
                    && route.methods.iter().any(|m| other.methods.contains(m))
            }) {
                return Err(Error::invalid("Ambiguous overlapping signed HTTP routes"));
            }
        }
        let mut operations = Vec::new();
        if !http_routes.is_empty() {
            let raw = raw.ok_or_else(|| Error::invalid("Signed OpenAPI file is missing"))?;
            let hash = digest(raw);
            if http_routes.iter().any(|(_, d)| *d != hash) {
                return Err(Error::invalid(
                    "Plugin OpenAPI raw-byte digest differs from Manifest",
                ));
            }
            let doc = strict_json(raw, MAXIMUM_CONTRACT_BYTES)?;
            if doc["openapi"] != "3.1.0" && doc["openapi"] != "3.1.1" {
                return Err(Error::invalid("Plugin HTTP requires OpenAPI 3.1"));
            }
            for field in ["title", "version"] {
                if doc["info"][field].as_str().is_none_or(str::is_empty) {
                    return Err(Error::invalid("Plugin OpenAPI info is incomplete"));
                }
            }
            if doc.get("servers").is_some_and(nonempty)
                || doc.get("webhooks").is_some_and(nonempty)
                || doc.get("security").is_some_and(nonempty)
            {
                return Err(Error::invalid(
                    "OpenAPI cannot supply backend origins or webhooks",
                ));
            }
            let paths = doc["paths"]
                .as_object()
                .filter(|p| !p.is_empty())
                .ok_or_else(|| Error::invalid("Plugin OpenAPI must declare operations"))?;
            for (path, item) in paths {
                canonical_path(path)?;
                if !path.starts_with(&format!("/plugins/{plugin}/")) {
                    return Err(Error::invalid(
                        "OpenAPI path escapes its business namespace",
                    ));
                }
                for (method, value) in item
                    .as_object()
                    .ok_or_else(|| Error::invalid("Invalid OpenAPI path item"))?
                {
                    if matches!(method.as_str(), "summary" | "description") {
                        continue;
                    }
                    if method == "parameters" && value.as_array().is_some_and(Vec::is_empty) {
                        continue;
                    }
                    let method = method.parse::<Method>().ok().filter(|m| {
                        matches!(
                            m.as_str(),
                            "get" | "post" | "put" | "patch" | "delete" | "head"
                        )
                    });
                    // HTTP Method is case sensitive. OpenAPI uses lowercase.
                    let method =
                        method.ok_or_else(|| Error::invalid("Unsupported OpenAPI path field"))?;
                    let method =
                        Method::from_bytes(method.as_str().to_ascii_uppercase().as_bytes())
                            .unwrap();
                    let matching = http_routes
                        .iter()
                        .filter(|(r, _)| under(path, &r.prefix) && r.methods.contains(&method))
                        .collect::<Vec<_>>();
                    if matching.len() != 1 {
                        return Err(Error::invalid(
                            "OpenAPI operation is not uniquely admitted by signed routes",
                        ));
                    }
                    let op = Operation::compile(path, method, value, &matching[0].0, &hash)?;
                    if operations.iter().any(|o: &Operation| o.id == op.id) {
                        return Err(Error::invalid("OpenAPI operationId must be unique"));
                    }
                    operations.push(op);
                }
            }
            if http_routes
                .iter()
                .any(|(r, _)| !operations.iter().any(|o| o.route.prefix == r.prefix))
            {
                return Err(Error::invalid(
                    "Signed business route has no declared operation",
                ));
            }
        } else if raw.is_some() {
            return Err(Error::invalid(
                "OpenAPI without a business route is not admitted",
            ));
        }
        let registry = Self {
            operations,
            static_routes,
            plugin: plugin.into(),
            candidates: manifest
                .get("skills")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
        };
        for definition in &registry.candidates {
            if definition["target"]["kind"] == "http" {
                registry.bind_skill(definition)?;
            }
        }
        Ok(registry)
    }

    pub fn resolve(&self, method: &Method, path: &str) -> Result<&Operation> {
        canonical_path(path)?;
        self.operations
            .iter()
            .find(|o| o.method == *method && o.path == path)
            .ok_or_else(Error::not_found)
    }
    pub fn resolve_static(&self, method: &Method, path: &str) -> Result<&Route> {
        canonical_path(path)?;
        self.static_routes
            .iter()
            .find(|r| r.methods.contains(method) && under(path, &r.prefix))
            .ok_or_else(Error::not_found)
    }

    /// Resolve only an exact signed candidate; registry lookup by operation ID
    /// never substitutes for the existing persistent Skill/installation checks.
    pub fn bind_skill(&self, definition: &Value) -> Result<&Operation> {
        validate("SkillDefinition", definition)?;
        if definition["target"]["kind"] != "http"
            || definition["target"]["plugin_id"] != self.plugin
            || !string(definition, "name")?.starts_with(&format!("{}.", self.plugin))
            || definition["mapping"] != "identity"
            || !definition["stream_item_schema"].is_null()
            || !self.candidates.contains(definition)
        {
            return Err(Error::invalid(
                "HTTP Skill differs from its signed candidate",
            ));
        }
        let op = self
            .operations
            .iter()
            .find(|op| {
                definition["target"]["http_operation_id"] == op.id
                    && definition["target"]["http_contract_digest"] == op.contract_digest
            })
            .ok_or_else(|| Error::invalid("HTTP Skill operation binding is unavailable"))?;
        if op
            .responses
            .get(&200)
            .and_then(|response| response.media_type.as_deref())
            != Some("application/json")
            || op.method != Method::POST
            || op
                .input
                .as_ref()
                .is_none_or(|schema| schema.value() != &definition["input_schema"])
            || op
                .responses
                .get(&200)
                .and_then(|response| response.schema.as_ref())
                .is_none_or(|schema| schema.value() != &definition["output_schema"])
        {
            return Err(Error::invalid(
                "HTTP Skill requires the fixed POST JSON to 200 JSON schemas",
            ));
        }
        Ok(op)
    }
}

fn nonempty(value: &Value) -> bool {
    !matches!(value, Value::Null)
        && !value.as_array().is_some_and(Vec::is_empty)
        && !value.as_object().is_some_and(serde_json::Map::is_empty)
}

pub fn canonical_path(path: &str) -> Result<()> {
    if path.len() > 2048
        || !path.starts_with('/')
        || path.contains("//")
        || path
            .bytes()
            .any(|c| !c.is_ascii_alphanumeric() && !b"/-._~!$&'()*+,;=:@".contains(&c))
        || path.split('/').any(|p| p == "." || p == "..")
    {
        return Err(Error::invalid("HTTP path is noncanonical or ambiguous"));
    }
    Ok(())
}
fn under(path: &str, prefix: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|tail| prefix.ends_with('/') || tail.starts_with('/'))
}

impl Operation {
    fn compile(
        path: &str,
        method: Method,
        value: &Value,
        route: &Route,
        hash: &str,
    ) -> Result<Self> {
        if method == Method::HEAD {
            return Err(Error::invalid(
                "Business HEAD is not implemented by the JSON HTTP profile",
            ));
        }
        if value.get("servers").is_some_and(nonempty)
            || value.get("callbacks").is_some_and(nonempty)
            || value.get("security").is_some_and(nonempty)
            || value["x-permission"] != route.action
        {
            return Err(Error::invalid(
                "HTTP operation changes its signed target or authority",
            ));
        }
        let id = string(value, "operationId")?;
        if id.is_empty() || id.len() > 128 || id.chars().any(char::is_control) {
            return Err(Error::invalid("Invalid HTTP operationId"));
        }
        let mut request_headers = HashMap::new();
        if let Some(params) = value.get("parameters") {
            for param in params
                .as_array()
                .ok_or_else(|| Error::invalid("Invalid HTTP parameters"))?
            {
                if param["in"] != "header" || param.get("$ref").is_some() {
                    return Err(Error::invalid(
                        "This HTTP profile supports literal paths and declared header parameters",
                    ));
                }
                let name = custom_header(string(param, "name")?)?;
                if request_headers
                    .insert(
                        name,
                        (param["required"] == true, Schema::new(&param["schema"])?),
                    )
                    .is_some()
                {
                    return Err(Error::invalid("Duplicate declared HTTP header"));
                }
            }
        }
        let input = if let Some(body) = value.get("requestBody") {
            if method == Method::GET || method == Method::HEAD || body["required"] != true {
                return Err(Error::invalid(
                    "JSON HTTP request bodies must be explicit and required",
                ));
            }
            Some(json_media(body)?)
        } else {
            None
        };
        let mut responses = BTreeMap::new();
        for (code, response) in value["responses"]
            .as_object()
            .ok_or_else(|| Error::invalid("HTTP operation must declare responses"))?
        {
            if code.len() != 3 || !code.bytes().all(|c| c.is_ascii_digit()) {
                return Err(Error::invalid(
                    "HTTP response status must use three decimal digits",
                ));
            }
            let status = code
                .parse::<u16>()
                .ok()
                .filter(|c| (200..=599).contains(c))
                .ok_or_else(|| Error::invalid("HTTP response status must be explicit"))?;
            if (300..400).contains(&status) {
                return Err(Error::invalid("Business HTTP redirects are not admitted"));
            }
            let (media_type, schema) = if status == 204 {
                if response.get("content").is_some_and(nonempty) {
                    return Err(Error::invalid("204 response cannot carry content"));
                }
                (None, None)
            } else {
                let content = response["content"]
                    .as_object()
                    .filter(|c| c.len() == 1)
                    .ok_or_else(|| {
                        Error::invalid("HTTP response requires one explicit media type")
                    })?;
                let media = content.keys().next().unwrap();
                if media != "application/json" && !(status == 200 && media == "text/event-stream") {
                    return Err(Error::invalid(
                        "Unsupported declared HTTP response media type",
                    ));
                }
                let schema = content[media]
                    .get("schema")
                    .ok_or_else(|| Error::invalid("HTTP response schema is missing"))?;
                (Some(media.clone()), Some(Schema::new(schema)?))
            };
            let mut headers = HashMap::new();
            if let Some(h) = response.get("headers") {
                for (name, spec) in h
                    .as_object()
                    .ok_or_else(|| Error::invalid("Invalid response headers"))?
                {
                    let name = custom_header(name)?;
                    if headers
                        .insert(name, Schema::new(&spec["schema"])?)
                        .is_some()
                    {
                        return Err(Error::invalid("Duplicate declared HTTP response header"));
                    }
                }
            }
            responses.insert(
                status,
                ResponseSpec {
                    media_type,
                    schema,
                    headers,
                },
            );
        }
        if !responses.keys().any(|c| *c < 300) || !responses.keys().any(|c| *c >= 400) {
            return Err(Error::invalid(
                "HTTP contract must declare success and structured errors",
            ));
        }
        Ok(Self {
            id: id.into(),
            path: path.into(),
            method,
            route: route.clone(),
            contract_digest: hash.into(),
            input,
            request_headers,
            responses,
        })
    }

    pub fn request(&self, headers: &HeaderMap, bytes: &[u8]) -> Result<HeaderMap> {
        reject_upgrade(headers)?;
        if bytes.len() > self.route.maximum_body_bytes {
            return Err(too_large());
        }
        let mut result = allowed_request_headers(headers, self.input.is_some())?;
        if let Some(schema) = &self.input {
            schema.check(&strict_json(bytes, self.route.maximum_body_bytes)?)?;
        } else if !bytes.is_empty() {
            return Err(Error::invalid("Undeclared HTTP request body"));
        }
        for (name, (required, schema)) in &self.request_headers {
            if let Some(value) = unique(headers, name.as_str())? {
                schema.check(&Value::String(
                    value
                        .to_str()
                        .map_err(|_| Error::invalid("Invalid request header"))?
                        .into(),
                ))?;
                result.insert(name.clone(), value.clone());
            } else if *required {
                return Err(Error::invalid("Required declared HTTP header is missing"));
            }
        }
        Ok(result)
    }

    pub fn response(
        &self,
        status: StatusCode,
        headers: &HeaderMap,
        bytes: &[u8],
    ) -> Result<HeaderMap> {
        if bytes.len() > MAXIMUM_RESPONSE_BYTES {
            return Err(too_large());
        }
        let (spec, result) = self.response_head(status, headers)?;
        if spec.media_type.as_deref() == Some("text/event-stream") {
            return Err(Error::invalid(
                "Event streams require incremental validation",
            ));
        }
        if let Some(schema) = &spec.schema {
            schema.check(&strict_json(bytes, MAXIMUM_RESPONSE_BYTES)?)?;
        } else if !bytes.is_empty() {
            return Err(Error::invalid("Body on a bodyless response"));
        }
        Ok(result)
    }

    pub fn response_head(
        &self,
        status: StatusCode,
        headers: &HeaderMap,
    ) -> Result<(&ResponseSpec, HeaderMap)> {
        reject_upgrade(headers)?;
        if headers.contains_key("set-cookie")
            || headers.contains_key("location")
            || headers.contains_key("content-encoding")
        {
            return Err(Error::invalid(
                "Business HTTP cannot set cookies, redirect or change encoding",
            ));
        }
        let spec = self
            .responses
            .get(&status.as_u16())
            .ok_or_else(|| Error::invalid("Undeclared plugin response status"))?;
        let mut result = HeaderMap::new();
        if let Some(media) = &spec.media_type {
            let actual = unique(headers, "content-type")?
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| Error::invalid("HTTP response content type is missing"))?;
            if actual != media && actual != format!("{media}; charset=utf-8") {
                return Err(Error::invalid(
                    "HTTP response media type differs from contract",
                ));
            }
            result.insert("content-type", HeaderValue::from_str(media).unwrap());
        }
        for (name, schema) in &spec.headers {
            if let Some(value) = unique(headers, name.as_str())? {
                schema.check(&Value::String(
                    value
                        .to_str()
                        .map_err(|_| Error::invalid("Invalid response header"))?
                        .into(),
                ))?;
                result.insert(name.clone(), value.clone());
            }
        }
        result.insert("cache-control", HeaderValue::from_static("no-store"));
        Ok((spec, result))
    }
}

fn json_media(value: &Value) -> Result<Schema> {
    let media = value["content"]
        .as_object()
        .filter(|m| m.len() == 1 && m.contains_key("application/json"))
        .ok_or_else(|| Error::invalid("This HTTP profile requires explicit JSON media schemas"))?;
    let schema = media["application/json"]
        .get("schema")
        .ok_or_else(|| Error::invalid("HTTP schema is missing"))?;
    Schema::new(schema)
}

fn custom_header(name: &str) -> Result<HeaderName> {
    let name = HeaderName::from_bytes(name.as_bytes())
        .map_err(|_| Error::invalid("Invalid declared header name"))?;
    if !name.as_str().starts_with("x-")
        || name.as_str().starts_with("x-xelab-")
        || name.as_str().starts_with("x-forwarded-")
        || [
            "auth",
            "token",
            "key",
            "credential",
            "secret",
            "cookie",
            "subject",
            "identity",
            "delegation",
        ]
        .iter()
        .any(|part| name.as_str().contains(part))
        || [
            "x-user-id",
            "x-plugin-id",
            "x-account-id",
            "x-csrf-token",
            "x-request-id",
            "x-api-key",
            "x-auth-token",
        ]
        .contains(&name.as_str())
    {
        return Err(Error::invalid("Sensitive or reserved custom header"));
    }
    Ok(name)
}

fn unique<'a>(headers: &'a HeaderMap, name: &str) -> Result<Option<&'a HeaderValue>> {
    if headers.get_all(name).iter().count() > 1 {
        return Err(Error::invalid("Duplicate HTTP header"));
    }
    Ok(headers.get(name))
}
fn require_json(headers: &HeaderMap) -> Result<()> {
    let media = unique(headers, "content-type")?
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if media.eq_ignore_ascii_case("application/json")
        || media.eq_ignore_ascii_case("application/json; charset=utf-8")
    {
        Ok(())
    } else {
        Err(Error::new(
            "UNSUPPORTED_MEDIA_TYPE",
            "HTTP contract requires application/json",
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ))
    }
}
pub(crate) fn reject_upgrade(headers: &HeaderMap) -> Result<()> {
    if headers.contains_key("upgrade")
        || headers.contains_key("sec-websocket-key")
        || headers.contains_key("sec-websocket-version")
        || headers.contains_key("last-event-id")
        || headers.get_all("connection").iter().any(|v| {
            v.to_str().map_or(true, |s| {
                s.split(',').any(|p| {
                    !["keep-alive", "close"]
                        .iter()
                        .any(|safe| p.trim().eq_ignore_ascii_case(safe))
                })
            })
        })
    {
        return Err(Error::invalid(
            "Protocol upgrades and stream resumption are not admitted",
        ));
    }
    Ok(())
}
fn allowed_request_headers(headers: &HeaderMap, has_body: bool) -> Result<HeaderMap> {
    let mut result = HeaderMap::new();
    if has_body {
        require_json(headers)?;
        result.insert("content-type", HeaderValue::from_static("application/json"));
    }
    for name in ["accept", "if-none-match"] {
        if let Some(value) = unique(headers, name)? {
            result.insert(HeaderName::from_static(name), value.clone());
        }
    }
    Ok(result)
}
pub(crate) fn too_large() -> Error {
    Error::new(
        "PAYLOAD_TOO_LARGE",
        "HTTP payload exceeds its signed limit",
        StatusCode::PAYLOAD_TOO_LARGE,
    )
}

#[cfg(test)]
mod tests;
