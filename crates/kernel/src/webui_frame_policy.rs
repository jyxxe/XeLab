//! A deterministic policy proof for framing public contribution documents.
//! It conveys no user/session or business authority. Only public selectors are
//! encoded; deployment, installation and approval bindings stay inside the MAC.
use super::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};

pub(super) const QUERY: &str = "xelab_frame";
const DOMAIN: &str = "xelab-ui-framing-policy-v1\n";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Selector {
    client_id: String,
    contribution_id: String,
    origin_index: u8,
}

fn parse(query: &str) -> Result<(Selector, Vec<u8>)> {
    let value = query
        .strip_prefix("xelab_frame=")
        .ok_or_else(Error::denied)?;
    if value.len() > 1024 {
        return Err(Error::denied());
    }
    let parts: Vec<_> = value.split('.').collect();
    if parts.len() != 3
        || parts[0] != "v1"
        || parts[1].is_empty()
        || parts[1].len() > 768
        || parts[2].len() != 43
        || parts[1..].iter().any(|part| {
            !part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
    {
        return Err(Error::denied());
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(parts[1])
        .map_err(|_| Error::denied())?;
    let value = crate::contracts::strict_json(&bytes, 576).map_err(|_| Error::denied())?;
    let selector: Selector = serde_json::from_value(value).map_err(|_| Error::denied())?;
    if selector.origin_index > 7
        || selector.client_id.is_empty()
        || selector.client_id.chars().count() > 128
        || selector.client_id.chars().any(char::is_control)
        || selector.contribution_id.is_empty()
        || selector.contribution_id.len() > 80
        || serde_jcs::to_vec(&selector)? != bytes
        || URL_SAFE_NO_PAD.encode(&bytes) != parts[1]
    {
        return Err(Error::denied());
    }
    let mac = URL_SAFE_NO_PAD
        .decode(parts[2])
        .map_err(|_| Error::denied())?;
    if mac.len() != 32 || URL_SAFE_NO_PAD.encode(&mac) != parts[2] {
        return Err(Error::denied());
    }
    Ok((selector, mac))
}

async fn descriptor(c: &(impl GenericClient + Sync), deployment: Uuid) -> Result<Value> {
    let row = c.query_opt("SELECT d.installation_id,d.approval,n.manifest_digest,d.spec->'config'->>'client_id' AS client_id FROM deployments d JOIN installations n ON n.id=d.installation_id WHERE d.id=$1 AND d.archived_at IS NULL", &[&deployment]).await?.ok_or_else(Error::denied)?;
    Ok(
        json!({"deployment_id":deployment,"installation_id":row.get::<_,Uuid>("installation_id"),"manifest_digest":row.get::<_,String>("manifest_digest"),"approval_digest":crate::contracts::digest(&serde_jcs::to_vec(&row.get::<_,Value>("approval"))?),"client_id":row.get::<_,Option<String>>("client_id")}),
    )
}

fn allocated_parent(app: &App, origin: &str) -> Result<Uuid> {
    let authority = origin.strip_prefix("https://").ok_or_else(Error::denied)?;
    let request = axum::http::Request::builder()
        .uri("/")
        .header("host", authority)
        .body(axum::body::Body::empty())
        .map_err(|_| Error::denied())?;
    let crate::plugin_http_gateway::Host::Deployment(id) =
        crate::plugin_http_gateway::host(&app.config, &request)?
    else {
        return Err(Error::denied());
    };
    if StaticOrigin::for_deployment(&app.config, id, None)?.origin() != origin {
        return Err(Error::denied());
    }
    Ok(id)
}

async fn policy(
    c: &(impl GenericClient + Sync),
    app: &App,
    target: Uuid,
    path: &str,
    selector: &Selector,
) -> Result<(String, String)> {
    let client = crate::oauth::clients::by_public_id(c, &selector.client_id).await?;
    if client.status != "active" {
        return Err(Error::denied());
    }
    let parent = client.spec["allowed_origins"]
        .as_array()
        .and_then(|origins| origins.get(selector.origin_index as usize))
        .and_then(Value::as_str)
        .ok_or_else(Error::denied)?;
    let shell_id = allocated_parent(app, parent)?;
    if shell_id == target {
        return Err(Error::denied());
    }
    let shell = check_deployment(c, app, shell_id).await?;
    let shell_binding = descriptor(c, shell_id).await?;
    if shell.origin != parent
        || shell.manifest["plugin_id"] != client.spec["plugin_id"]
        || shell_binding["client_id"] != selector.client_id
    {
        return Err(Error::denied());
    }
    let rows = c.query(READY, &[&target]).await?;
    if rows.len() != 1 {
        return Err(Error::unavailable("Contribution is unavailable"));
    }
    let manifest: Value = rows[0].get("manifest");
    crate::ui::validate_manifest(&manifest)?;
    let spec = manifest["ui"]["contributions"]
        .as_array()
        .and_then(|specs| {
            specs
                .iter()
                .find(|spec| spec["id"] == selector.contribution_id)
        })
        .ok_or_else(Error::denied)?;
    if path
        != format!(
            "/ui/{}{}",
            string(&manifest, "plugin_id")?,
            string(spec, "entry_path")?
        )
    {
        return Err(Error::denied());
    }
    let installation: Uuid = rows[0].get("installation_id");
    let registry = crate::http_contracts::load(c, installation).await?;
    for method in [Method::GET, Method::HEAD] {
        if !registry.resolve_static(&method, path)?.public {
            return Err(Error::denied());
        }
    }
    crate::plugin_http_gateway::check_approval(c, app, target, &manifest).await?;
    let data = json!({"client_id":selector.client_id,"client_revision":client.revision.to_string(),"parent_origin":parent,"shell":shell_binding,"target":descriptor(c,target).await?,"contribution_id":selector.contribution_id,"contribution_digest":crate::skills::definition_digest(spec)?,"entry_path":path});
    let input = String::from_utf8(serde_jcs::to_vec(&data)?).map_err(|_| Error::denied())?;
    Ok((parent.to_owned(), format!("{DOMAIN}{input}")))
}

/// Called only after the existing UI visibility/permission check for this fixed
/// contribution. A claimed browser Origin alone never enables private framing.
pub(crate) async fn framing_query(
    c: &(impl GenericClient + Sync),
    app: &App,
    ctx: &Context,
    target: Uuid,
    contribution: &Value,
) -> Result<Option<String>> {
    let Some(parent) = ctx.browser_origin.as_deref() else {
        return Ok(None);
    };
    if app.config.value["ui_gateway"]["shell_origins"]
        .as_array()
        .is_some_and(|origins| origins.iter().any(|origin| origin == parent))
    {
        return Ok(None);
    }
    let row = c
        .query_opt(
            "SELECT kind,status,expires_at FROM credentials WHERE id=$1 AND owner_id=$2",
            &[&ctx.credential, &ctx.subject],
        )
        .await?
        .ok_or_else(Error::credential)?;
    if row.get::<_, String>("kind") != "oauth_access"
        || row.get::<_, String>("status") != "active"
        || row.get::<_, chrono::DateTime<chrono::Utc>>("expires_at") <= chrono::Utc::now()
    {
        return Err(Error::credential());
    }
    let binding = crate::oauth::binding(c, ctx.credential).await?;
    if binding.subject_id != ctx.subject || binding.origin != parent {
        return Err(Error::denied());
    }
    let client = crate::oauth::clients::by_public_id(c, &binding.client_id).await?;
    let index = client.spec["allowed_origins"]
        .as_array()
        .and_then(|origins| origins.iter().position(|origin| origin == parent))
        .ok_or_else(Error::denied)?;
    let selector = Selector {
        client_id: binding.client_id,
        origin_index: u8::try_from(index).map_err(|_| Error::denied())?,
        contribution_id: string(contribution, "id")?.to_owned(),
    };
    let rows = c.query("SELECT n.plugin_id FROM deployments d JOIN installations n ON n.id=d.installation_id WHERE d.id=$1", &[&target]).await?;
    let plugin: String = rows.first().ok_or_else(Error::denied)?.get(0);
    let path = format!("/ui/{plugin}{}", string(contribution, "entry_path")?);
    let (verified_parent, input) = policy(c, app, target, &path, &selector).await?;
    if verified_parent != parent {
        return Err(Error::denied());
    }
    let query = format!(
        "{QUERY}=v1.{}.{}",
        URL_SAFE_NO_PAD.encode(serde_jcs::to_vec(&selector)?),
        URL_SAFE_NO_PAD.encode(app.auth.token_digest(&input))
    );
    parse(&query)?;
    Ok(Some(query))
}

pub(crate) async fn validate_framing_query(
    c: &(impl GenericClient + Sync),
    app: &App,
    target: Uuid,
    path: &str,
    query: &str,
) -> Result<String> {
    let (selector, mac) = parse(query)?;
    // An untrusted selector must not turn current private shell/approval state
    // into distinct public errors. It still needs a valid policy MAC to frame.
    let (parent, input) = policy(c, app, target, path, &selector)
        .await
        .map_err(|_| Error::denied())?;
    if !app.auth.verify_mac(&input, &mac) {
        return Err(Error::denied());
    }
    Ok(parent)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn framing_wire_rejects_noncanonical_unknown_and_encoded_parameters() {
        let selector = Selector {
            client_id: "xelab-public".into(),
            contribution_id: "desktop".into(),
            origin_index: 0,
        };
        let raw = URL_SAFE_NO_PAD.encode(serde_jcs::to_vec(&selector).unwrap());
        let mac = URL_SAFE_NO_PAD.encode([0u8; 32]);
        let query = format!("{QUERY}=v1.{raw}.{mac}");
        assert_eq!(parse(&query).unwrap().0.client_id, "xelab-public");
        for bad in [
            format!("{query}&other=1"),
            format!("{query}&{query}"),
            format!("%78elab_frame={}", &query[12..]),
            format!("xelab_frame=%2576{}", &query[13..]),
            format!("{query}="),
            format!("xelab_frame=v2.{raw}.{mac}"),
            format!("xelab_frame=v1.{raw}+.{mac}"),
        ] {
            assert!(parse(&bad).is_err());
        }
        for json in [
            r#"{"client_id":"xelab-public","contribution_id":"desktop","origin_index":8}"#,
            r#"{"client_id":"xelab-public","contribution_id":"desktop","origin_index":0,"other":1}"#,
            r#"{"client_id":"xelab-public","client_id":"xelab-public","contribution_id":"desktop","origin_index":0}"#,
            r#"{"origin_index":0,"contribution_id":"desktop","client_id":"xelab-public"}"#,
            r#"{"client_id":"xelab-public","contribution_id":"desktop","origin_index":0.0}"#,
        ] {
            assert!(parse(&format!(
                "{QUERY}=v1.{}.{mac}",
                URL_SAFE_NO_PAD.encode(json)
            ))
            .is_err());
        }
        assert!(parse(&format!("{QUERY}=v1.{}.{mac}", "A".repeat(769))).is_err());
    }
}
