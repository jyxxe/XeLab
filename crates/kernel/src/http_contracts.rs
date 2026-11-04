//! Immutable, verified OpenAPI provenance. This storage helper does not
//! authenticate callers or replace the current Authorizer's route checks.
use crate::{
    contracts::{digest, id, string, unsigned, validate},
    error::{Error, Result},
    plugin_http::{self, Registry},
};
use axum::http::Method;
use serde_json::{json, Value};
use std::{
    collections::{HashSet, VecDeque},
    path::Path,
    sync::{Arc, OnceLock},
};
use tokio::sync::{Mutex, Semaphore};
use tokio_postgres::GenericClient;
use uuid::Uuid;

const MAXIMUM_CACHED_REGISTRIES: usize = 4;
#[derive(PartialEq, Eq)]
struct RegistryKey {
    installation: Uuid,
    raw_digest: Option<String>,
    manifest_digest: String,
}
struct CachedRegistry {
    key: RegistryKey,
    registry: Arc<Registry>,
}
fn registry_cache() -> &'static Mutex<VecDeque<CachedRegistry>> {
    static CACHE: OnceLock<Mutex<VecDeque<CachedRegistry>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}
async fn cached_registry(key: &RegistryKey) -> Option<Arc<Registry>> {
    let mut cache = registry_cache().lock().await;
    let index = cache.iter().position(|entry| entry.key == *key)?;
    let entry = cache.remove(index)?;
    let registry = entry.registry.clone();
    cache.push_back(entry);
    Some(registry)
}

/// Called in the same transaction which persists the already-verified
/// installation. No user-supplied OpenAPI or plugin Describe response enters it.
pub async fn store(
    c: &(impl GenericClient + Sync),
    installation: Uuid,
    manifest: &Value,
    raw: Option<&[u8]>,
) -> Result<()> {
    Registry::compile(manifest, raw)?;
    validate_actions(c, manifest).await?;
    let installed: Value = c
        .query_opt(
            "SELECT manifest FROM installations WHERE id=$1 FOR SHARE",
            &[&installation],
        )
        .await?
        .ok_or_else(Error::not_found)?
        .get(0);
    if installed != *manifest {
        return Err(Error::conflict("HTTP contract installation source differs"));
    }
    if let Some(raw) = raw {
        c.execute(
            "INSERT INTO plugin_http_contracts(installation_id,digest,raw) VALUES($1,$2,$3) ON CONFLICT(installation_id) DO NOTHING",
            &[&installation, &digest(raw), &raw],
        )
        .await?;
    }
    let stored = c
        .query_opt(
            "SELECT digest,raw FROM plugin_http_contracts WHERE installation_id=$1",
            &[&installation],
        )
        .await?;
    match (raw, stored.as_ref()) {
        (Some(raw), Some(row))
            if row.get::<_, String>("digest") == digest(raw)
                && row.get::<_, Vec<u8>>("raw") == raw => {}
        (None, None) => {}
        _ => {
            return Err(Error::conflict(
                "An immutable HTTP contract already differs",
            ))
        }
    }
    Ok(())
}

/// Cache only compiled immutable definitions. Every lookup still reads current
/// database bytes, verifies their digest and exact signed route correspondence.
/// Trust, actions, approval, deadlines and caller authority are never cached.
pub async fn load(c: &(impl GenericClient + Sync), installation: Uuid) -> Result<Arc<Registry>> {
    let row = c.query_opt("SELECT n.manifest,h.digest,h.raw FROM installations n LEFT JOIN plugin_http_contracts h ON h.installation_id=n.id WHERE n.id=$1",&[&installation]).await?.ok_or_else(Error::not_found)?;
    let manifest: Value = row.get("manifest");
    let raw: Option<Vec<u8>> = row.get("raw");
    let hash: Option<String> = row.get("digest");
    if raw.as_ref().map(|bytes| digest(bytes)) != hash {
        return Err(Error::conflict("Stored HTTP contract digest differs"));
    }
    let routes = manifest["routes"]
        .as_array()
        .ok_or_else(|| Error::invalid("Manifest routes are missing"))?;
    let http_routes = routes
        .iter()
        .filter(|route| route["kind"] == "http")
        .collect::<Vec<_>>();
    if raw.is_some() != !http_routes.is_empty()
        || http_routes
            .iter()
            .any(|route| route["http_contract_digest"].as_str() != hash.as_deref())
    {
        return Err(Error::conflict(
            "Stored HTTP contract does not match every signed route",
        ));
    }
    let key = RegistryKey {
        installation,
        raw_digest: hash,
        manifest_digest: digest(&serde_json::to_vec(&manifest)?),
    };
    if let Some(registry) = cached_registry(&key).await {
        return Ok(registry);
    }
    // Serialize cold builds, while keeping the cache map available to unrelated
    // hot lookups. The permit travels through the blocking job and survives a
    // canceled waiter, so abandoned compiles cannot accumulate in the background.
    static COMPILER: OnceLock<Arc<Semaphore>> = OnceLock::new();
    let permit = COMPILER
        .get_or_init(|| Arc::new(Semaphore::new(1)))
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| Error::unavailable("HTTP schema compiler is unavailable"))?;
    if let Some(registry) = cached_registry(&key).await {
        return Ok(registry);
    }
    let (compiled, _permit) = tokio::task::spawn_blocking(move || {
        (
            Registry::compile(&manifest, raw.as_deref()).map(Arc::new),
            permit,
        )
    })
    .await
    .map_err(|_| Error::unavailable("HTTP schema compilation stopped"))?;
    let registry = compiled?;
    let mut cache = registry_cache().lock().await;
    if cache.len() >= MAXIMUM_CACHED_REGISTRIES {
        cache.pop_front();
    }
    cache.push_back(CachedRegistry {
        key,
        registry: registry.clone(),
    });
    Ok(registry)
}

/// Recheck the original member when exporting/importing an immutable OCI image.
/// The deployment may have changed since historical calls were admitted; compare
/// against the fixed installation rather than the current deployment target.
pub async fn verify_image(
    c: &(impl GenericClient + Sync),
    installation: Uuid,
    manifest: &Value,
    root: &Path,
) -> Result<Option<Vec<u8>>> {
    let has_http = manifest["routes"]
        .as_array()
        .is_some_and(|routes| routes.iter().any(|route| route["kind"] == "http"));
    let raw = if has_http {
        Some(crate::artifact::read_image_file(
            root,
            "xelab/http/openapi.json",
            plugin_http::MAXIMUM_CONTRACT_BYTES,
        )?)
    } else {
        None
    };
    Registry::compile(manifest, raw.as_deref())?;
    let stored = c
        .query_opt(
            "SELECT digest,raw FROM plugin_http_contracts WHERE installation_id=$1",
            &[&installation],
        )
        .await?;
    match (raw.as_ref(), stored.as_ref()) {
        (Some(raw), Some(row))
            if row.get::<_, String>("digest") == digest(raw)
                && row.get::<_, Vec<u8>>("raw") == *raw => {}
        (None, None) => (),
        _ => {
            return Err(Error::conflict(
                "OCI HTTP member differs from immutable installation provenance",
            ))
        }
    }
    Ok(raw)
}

/// Restrict signed actions to their existing permission registration. A valid
/// action name does not itself authorize a caller or approve a deployment.
pub async fn validate_actions(c: &(impl GenericClient + Sync), manifest: &Value) -> Result<()> {
    let plugin = string(manifest, "plugin_id")?;
    let routes = manifest["routes"]
        .as_array()
        .ok_or_else(|| Error::invalid("Manifest routes are missing"))?;
    let mut checked = HashSet::new();
    for route in routes {
        validate("RouteSpec", route)?;
        let action = string(route, "required_action")?;
        if route["kind"] == "http"
            && action != "plugin.http.invoke"
            && !action.starts_with(&format!("{plugin}."))
        {
            return Err(Error::invalid(
                "HTTP business action is outside its plugin namespace",
            ));
        }
        if !checked.insert(action.to_owned()) {
            continue;
        }
        let definition = c
            .query_opt(
                "SELECT resource_kinds FROM permission_definitions WHERE action=$1",
                &[&action],
            )
            .await?
            .ok_or_else(|| Error::invalid("HTTP route action is not registered"))?;
        let kinds: Value = definition.get(0);
        if !kinds
            .as_array()
            .is_some_and(|kinds| kinds.iter().any(|kind| kind == "plugin"))
        {
            return Err(Error::invalid("HTTP route action does not target plugins"));
        }
    }
    Ok(())
}

/// Check exact data-only provenance before snapshot export and after import.
/// Historical records do not require today's publisher/credential/deployment
/// enable state; recovery separately resets all trust and live authority.
pub async fn validate_snapshot_bindings(c: &(impl GenericClient + Sync)) -> Result<()> {
    if c.query_one("SELECT EXISTS(SELECT 1 FROM calls c JOIN skills s ON s.id=c.skill_id WHERE s.definition->'target'->>'kind'='http' AND c.http_binding IS NULL)",&[]).await?.get::<_,bool>(0) {
        return Err(Error::conflict("Historical HTTP Skill call has no fixed HTTP binding"));
    }
    let mut cursor: Option<Uuid> = None;
    loop {
        let rows = c
            .query(
                "SELECT id,plugin_id,version,publisher_id,image_digest,manifest_digest,manifest FROM installations WHERE ($1::uuid IS NULL OR id>$1) ORDER BY id LIMIT 200",
                &[&cursor],
            )
            .await?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            let installation: Uuid = row.get("id");
            cursor = Some(installation);
            let manifest: Value = row.get("manifest");
            let has_routes = manifest["routes"]
                .as_array()
                .is_some_and(|routes| !routes.is_empty());
            let stored = c
                .query_opt(
                    "SELECT digest,raw FROM plugin_http_contracts WHERE installation_id=$1",
                    &[&installation],
                )
                .await?;
            if has_routes || stored.is_some() {
                validate("Manifest", &manifest)?;
                if manifest["plugin_id"] != row.get::<_, String>("plugin_id")
                    || manifest["version"] != row.get::<_, String>("version")
                    || manifest["publisher_id"] != row.get::<_, Uuid>("publisher_id").to_string()
                    || ["image_digest", "manifest_digest"].iter().any(|field| {
                        let hash: String = row.get(*field);
                        hash.len() != 71
                            || !hash.starts_with("sha256:")
                            || !hash[7..]
                                .bytes()
                                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    })
                {
                    return Err(Error::conflict(
                        "HTTP installation metadata differs from its immutable source",
                    ));
                }
                load(c, installation).await?;
            }
        }
    }
    let mut cursor: Option<Uuid> = None;
    loop {
        let rows=c.query("SELECT c.id,c.http_binding,c.skill_id,c.skill_definition_digest,c.contract,c.provider_instance_id,i.deployment_id,i.generation FROM calls c LEFT JOIN instances i ON i.id=c.provider_instance_id WHERE c.http_binding IS NOT NULL AND ($1::uuid IS NULL OR c.id>$1) ORDER BY c.id LIMIT 200",&[&cursor]).await?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            let call: Uuid = row.get("id");
            cursor = Some(call);
            let binding: Value = row.get("http_binding");
            let installation = id(&binding, "installation_id")?;
            let deployment = id(&binding, "deployment_id")?;
            let generation = unsigned(&binding["generation"])?;
            if generation == 0
                || generation > i64::MAX as u64
                || row.get::<_, Option<Uuid>>("provider_instance_id").is_none()
                || row.get::<_, Option<Uuid>>("deployment_id") != Some(deployment)
                || row.get::<_, Option<i64>>("generation") != Some(generation as i64)
            {
                return Err(Error::conflict("Historical HTTP instance binding differs"));
            }
            let registry = load(c, installation).await?;
            let method = Method::from_bytes(string(&binding, "method")?.as_bytes())
                .map_err(|_| Error::invalid("Historical HTTP method is invalid"))?;
            let operation = registry.resolve(&method, string(&binding, "path")?)?;
            if operation.id != string(&binding, "operation_id")?
                || operation.contract_digest != string(&binding, "http_contract_digest")?
                || operation.route.action != string(&binding, "required_action")?
            {
                return Err(Error::conflict("Historical HTTP operation binding differs"));
            }
            let contract: Value = row.get("contract");
            if let Some(skill) = row.get::<_, Option<Uuid>>("skill_id") {
                let skill_row=c.query_opt("SELECT installation_id,definition,definition_digest FROM skills WHERE id=$1",&[&skill]).await?.ok_or_else(||Error::conflict("Historical HTTP Skill is missing"))?;
                let definition: Value = skill_row.get("definition");
                let hash: String = skill_row.get("definition_digest");
                let selected = registry.bind_skill(&definition)?;
                if skill_row.get::<_, Uuid>("installation_id") != installation
                    || crate::skills::definition_digest(&definition)? != hash
                    || row.get::<_, Option<String>>("skill_definition_digest") != Some(hash.clone())
                    || selected.id != operation.id
                    || contract
                        != json!({"name":definition["name"],"version":definition["version"],"schema_digest":hash})
                {
                    return Err(Error::conflict("Historical HTTP Skill contract differs"));
                }
            } else {
                let manifest: Value = c
                    .query_one(
                        "SELECT manifest FROM installations WHERE id=$1",
                        &[&installation],
                    )
                    .await?
                    .get(0);
                let expected = json!({"name":format!("{}.http",string(&manifest,"plugin_id")?),"version":manifest["version"],"schema_digest":operation.contract_digest});
                if contract != expected {
                    return Err(Error::conflict("Historical HTTP call contract differs"));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests;
