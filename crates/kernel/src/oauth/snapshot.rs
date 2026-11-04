//! Validate frozen OAuth metadata before an offline restore resets its trust.
//! Historical requests are checked against their saved identity and ceiling,
//! not a Client's later mutable redirect list or current authorization.
use crate::{
    contracts::{openapi, validate, validate_business},
    error::{Error, Result},
    policy::{self, Path},
};
use serde_json::Value;
use std::collections::BTreeSet;
use tokio_postgres::GenericClient;
use uuid::Uuid;

fn conflict() -> Error {
    Error::conflict("OAuth snapshot binding is inconsistent")
}

/// A restored database cannot publish readiness while any historical browser
/// client, consent, continuation or credential family can confer authority.
pub async fn ensure_authority_reset(c: &(impl GenericClient + Sync)) -> Result<()> {
    let live: bool = c.query_one("SELECT EXISTS(SELECT 1 FROM clients WHERE status='active') OR EXISTS(SELECT 1 FROM authorization_requests WHERE status IN ('pending','approved')) OR EXISTS(SELECT 1 FROM oauth_families WHERE status='active') OR EXISTS(SELECT 1 FROM oauth_login_intents) OR EXISTS(SELECT 1 FROM credentials WHERE kind IN ('oauth_access','oauth_refresh') AND status='active')", &[]).await?.get(0);
    if live {
        return Err(Error::conflict(
            "Historical browser authority survived recovery",
        ));
    }
    Ok(())
}

fn rules(value: &Value) -> Result<()> {
    let values = value.as_array().ok_or_else(conflict)?;
    if values.len() > 4096 {
        return Err(conflict());
    }
    for rule in values {
        validate("PermissionRule", rule)?;
    }
    Ok(())
}

fn frozen(rules_value: &Value, saved: &Value) -> Result<()> {
    rules(rules_value)?;
    let values = saved.as_array().ok_or_else(conflict)?;
    if values.len() > 4096 {
        return Err(conflict());
    }
    for path in values {
        let object = path.as_object().ok_or_else(conflict)?;
        if object.len() != 2 || !object.contains_key("rule") || !object.contains_key("sources") {
            return Err(conflict());
        }
        let source = path["sources"].as_array().ok_or_else(conflict)?;
        let mut distinct = BTreeSet::new();
        for id in source {
            let id = id
                .as_str()
                .and_then(|value| value.parse::<Uuid>().ok())
                .ok_or_else(conflict)?;
            if !distinct.insert(id) {
                return Err(conflict());
            }
        }
    }
    let paths: Vec<Path> = serde_json::from_value(saved.clone()).map_err(|_| conflict())?;
    if policy::rules(&paths) != *rules_value {
        return Err(conflict());
    }
    Ok(())
}

fn scope(value: &str, client: &str) -> Result<()> {
    if value.is_empty() || value.len() > 512 {
        return Err(conflict());
    }
    let full = format!("client:{client}");
    let mut previous = None;
    for handle in value.split(' ') {
        if previous.is_some_and(|old: &str| old >= handle) {
            return Err(conflict());
        }
        if handle != full {
            let hash = handle.strip_prefix("rule:sha256:").ok_or_else(conflict)?;
            if hash.len() != 64
                || !hash
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(conflict());
            }
        }
        previous = Some(handle);
    }
    Ok(())
}

fn origin(value: &str) -> Result<()> {
    let parsed = url::Url::parse(value).map_err(|_| conflict())?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.origin().ascii_serialization() != value
    {
        return Err(conflict());
    }
    Ok(())
}

fn intent(value: &Value) -> Result<()> {
    let _: super::authorization::AuthorizationInput =
        serde_json::from_value(value.clone()).map_err(|_| conflict())?;
    for parameter in openapi()["paths"]["/oauth/authorize"]["get"]["parameters"]
        .as_array()
        .ok_or_else(conflict)?
    {
        let name = parameter["name"].as_str().ok_or_else(conflict)?;
        if let Some(value) = value.get(name) {
            validate_business(&parameter["schema"], value)?;
        }
    }
    Ok(())
}

/// Call only with writers fenced, using the same trusted schema as the
/// authenticated snapshot. No live permission or publisher trust is inferred.
/// Recovery must still revoke credentials/families/clients and delete intents.
pub async fn validate_snapshot(c: &(impl GenericClient + Sync)) -> Result<()> {
    // These are internal provenance fields, not authority supplied by a
    // restored plugin or a new HTTP request. NULL explicitly denotes CLI use.
    for table in [
        "host_proxy_approvals",
        "plugin_http_approvals",
        "host_rpc_contexts",
    ] {
        let mut after: Option<Uuid> = None;
        let key = if table == "host_rpc_contexts" {
            "call_id"
        } else {
            "deployment_id"
        };
        loop {
            let rows=c.query(&format!("SELECT {key},browser_origin FROM {table} WHERE ($1::uuid IS NULL OR {key}>$1) ORDER BY {key} LIMIT 200"),&[&after]).await?;
            if rows.is_empty() {
                break;
            }
            for row in rows {
                after = Some(row.get(key));
                if let Some(value) = row.get::<_, Option<String>>("browser_origin") {
                    origin(&value)?;
                }
            }
        }
    }
    let mut after: Option<Uuid> = None;
    loop {
        let rows = c
            .query(
                "SELECT id,client_id,spec FROM clients WHERE ($1::uuid IS NULL OR id>$1) ORDER BY id LIMIT 200",
                &[&after],
            )
            .await?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            after = Some(row.get("id"));
            let client: String = row.get("client_id");
            if client.is_empty() || client.len() > 128 || client.chars().any(char::is_whitespace) {
                return Err(conflict());
            }
            super::clients::validate_spec(&row.get::<_, Value>("spec"))?;
        }
    }
    after = None;
    loop {
        let rows=c.query("SELECT r.*,k.kind AS session_kind,p.revision AS current_client_revision FROM authorization_requests r LEFT JOIN credentials k ON k.id=r.session_id LEFT JOIN clients p ON p.client_id=r.client_id WHERE ($1::uuid IS NULL OR r.id>$1) ORDER BY r.id LIMIT 200",&[&after]).await?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            after = Some(row.get("id"));
            let client: String = row.get("client_id");
            if row.get::<_, Option<String>>("session_kind").as_deref() != Some("session")
                || row
                    .get::<_, Option<i64>>("current_client_revision")
                    .is_none_or(|revision| revision < row.get::<_, i64>("client_revision"))
                || row.get::<_, String>("hmac_key_id").is_empty()
                || row.get::<_, Vec<u8>>("csrf_digest").len() != 32
                || row
                    .get::<_, Option<Vec<u8>>>("code_digest")
                    .is_some_and(|value| value.len() != 32)
                || (matches!(
                    row.get::<_, String>("status").as_str(),
                    "approved" | "consumed"
                ) && row.get::<_, Option<Vec<u8>>>("code_digest").is_none())
            {
                return Err(conflict());
            }
            scope(&row.get::<_, String>("scope"), &client)?;
            origin(&row.get::<_, String>("origin"))?;
            intent(
                &serde_json::json!({"client_id":client,"redirect_uri":row.get::<_,String>("redirect_uri"),"state":row.get::<_,String>("state"),"code_challenge":row.get::<_,String>("code_challenge"),"scope":row.get::<_,String>("scope")}),
            )?;
            frozen(
                &row.get::<_, Value>("fixed_rules"),
                &row.get::<_, Value>("fixed_paths"),
            )?;
            // A later Client update may remove this historical callback.
            let redirect =
                url::Url::parse(&row.get::<_, String>("redirect_uri")).map_err(|_| conflict())?;
            if redirect.origin().ascii_serialization() != row.get::<_, String>("origin")
                || redirect.fragment().is_some()
                || !redirect.username().is_empty()
                || redirect.password().is_some()
            {
                return Err(conflict());
            }
        }
    }
    after = None;
    loop {
        let rows=c.query("SELECT f.*,k.kind AS session_kind,k.owner_id AS session_owner,p.revision AS current_client_revision FROM oauth_families f LEFT JOIN credentials k ON k.id=f.session_id LEFT JOIN clients p ON p.client_id=f.client_id WHERE ($1::uuid IS NULL OR f.id>$1) ORDER BY f.id LIMIT 200",&[&after]).await?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            after = Some(row.get("id"));
            if row.get::<_, Option<String>>("session_kind").as_deref() != Some("session")
                || row.get::<_, Option<Uuid>>("session_owner") != Some(row.get("subject_id"))
                || row
                    .get::<_, Option<i64>>("current_client_revision")
                    .is_none_or(|revision| revision < row.get::<_, i64>("client_revision"))
            {
                return Err(conflict());
            }
            scope(
                &row.get::<_, String>("scope"),
                &row.get::<_, String>("client_id"),
            )?;
            origin(&row.get::<_, String>("origin"))?;
            frozen(
                &row.get::<_, Value>("maximum_rules"),
                &row.get::<_, Value>("fixed_paths"),
            )?;
        }
    }
    let invalid:bool=c.query_one("SELECT EXISTS(SELECT 1 FROM credentials k LEFT JOIN oauth_families f ON f.id=k.family_id WHERE k.kind IN ('oauth_access','oauth_refresh') AND (f.id IS NULL OR k.owner_id<>f.subject_id OR k.client_id IS DISTINCT FROM f.client_id OR k.issuer_credential_id IS DISTINCT FROM f.session_id OR k.inheritance_mode IS DISTINCT FROM 'selected' OR k.maximum_rules IS NULL OR k.conditions<>jsonb_build_object('audiences',jsonb_build_array('xelab.platform')))) OR EXISTS(SELECT 1 FROM oauth_refresh_tokens r LEFT JOIN credentials k ON k.id=r.credential_id LEFT JOIN credentials n ON n.id=r.replacement_id WHERE k.id IS NULL OR k.kind<>'oauth_refresh' OR k.family_id IS DISTINCT FROM r.family_id OR (r.replacement_id IS NOT NULL AND (n.id IS NULL OR n.kind<>'oauth_refresh' OR n.family_id IS DISTINCT FROM r.family_id OR n.id=k.id))) OR EXISTS(SELECT 1 FROM credentials k WHERE k.kind='oauth_refresh' AND NOT EXISTS(SELECT 1 FROM oauth_refresh_tokens r WHERE r.credential_id=k.id AND r.family_id=k.family_id))",&[]).await?.get(0);
    if invalid {
        return Err(conflict());
    }
    after = None;
    loop {
        let rows=c.query("SELECT id,maximum_rules FROM credentials WHERE kind IN ('oauth_access','oauth_refresh') AND ($1::uuid IS NULL OR id>$1) ORDER BY id LIMIT 200",&[&after]).await?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            after = Some(row.get("id"));
            rules(&row.get::<_, Value>("maximum_rules"))?;
        }
    }
    // Pending unauthenticated intents contain only an HMAC verifier and public
    // authorize parameters, and are discarded by reset_authority after import.
    let invalid:bool=c.query_one("SELECT EXISTS(SELECT 1 FROM oauth_login_intents i LEFT JOIN clients p ON p.client_id=i.client_id WHERE p.id IS NULL OR i.client_revision>p.revision OR jsonb_typeof(i.request_spec)<>'object' OR i.request_spec->>'client_id' IS DISTINCT FROM i.client_id OR i.hmac_key_id='')",&[]).await?.get(0);
    if invalid {
        return Err(conflict());
    }
    after = None;
    loop {
        let rows = c.query("SELECT id,request_spec FROM oauth_login_intents WHERE ($1::uuid IS NULL OR id>$1) ORDER BY id LIMIT 200", &[&after]).await?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            after = Some(row.get("id"));
            intent(&row.get::<_, Value>("request_spec"))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn historical_scopes_and_origins_remain_exact_without_resolving_latest_rules() {
        scope("client:public-id", "public-id").unwrap();
        scope(&format!("rule:sha256:{}", "a".repeat(64)), "public-id").unwrap();
        for value in [
            "client:other-id",
            "client:public-id client:public-id",
            " client:public-id",
            "rule:sha256:not-a-digest",
        ] {
            assert!(scope(value, "public-id").is_err());
        }
        origin("https://ui.example:9443").unwrap();
        for value in [
            "http://ui.example",
            "https://ui.example/",
            "https://ui.example/path",
            "https://user@ui.example",
            "https://ui.example#fragment",
        ] {
            assert!(origin(value).is_err());
        }
    }

    #[test]
    fn frozen_rules_cannot_be_separated_from_their_original_paths() {
        let rule = json!({"action":"plugin.read","resource":{"kind":"plugin","match":"all"}});
        let saved = json!([{"rule":rule,"sources":[Uuid::new_v4()]}]);
        frozen(&json!([rule]), &saved).unwrap();
        let mut changed = saved.clone();
        changed[0]["rule"]["action"] = json!("plugin.install");
        assert!(frozen(&json!([rule]), &changed).is_err());
        let mut forged = saved;
        forged[0]["subject_id"] = json!(Uuid::new_v4());
        assert!(frozen(&json!([rule]), &forged).is_err());
    }

    #[tokio::test]
    async fn actual_postgres_history_survives_client_update_but_rejects_changed_source() {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
        use sha2::{Digest, Sha256};
        let Some(f) = crate::backups::tests::Fixture::new().await else {
            return;
        };
        let mut c = f.app.db.connect().await.unwrap();
        // Component evidence only: register real sequential 15/16 migrations
        // before the final offline COPY/import acceptance in the C integration.
        let peer = "127.0.0.1".parse().unwrap();
        let ctx = f
            .app
            .auth
            .authenticate(&c, &f.token, peer, Uuid::new_v4())
            .await
            .unwrap();
        let client_id = format!("xelab-{}", Uuid::new_v4().simple());
        let client = Uuid::new_v4();
        let spec = json!({"name":"Snapshot UI","plugin_id":"org.xelab.webui","redirect_uris":["https://old.example/callback"],"allowed_origins":["https://old.example"],"maximum_rules":[{"action":"platform.read","resource":{"kind":"platform","match":"all"}}]});
        super::super::clients::validate_spec(&spec).unwrap();
        c.execute("INSERT INTO clients(id,client_id,spec,status,created_by,created_at,updated_at) VALUES($1,$2,$3,'active',$4,now(),now())",&[&client,&client_id,&spec,&ctx.subject]).await.unwrap();
        let verifier = "snapshot-verifier-with-enough-entropy-for-the-test-123456";
        let input = super::super::authorization::AuthorizationInput {
            client_id: client_id.clone(),
            redirect_uri: "https://old.example/callback".into(),
            state: "snapshot-state".into(),
            code_challenge: URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())),
            scope: format!("client:{client_id}"),
        };
        let tx = c.transaction().await.unwrap();
        crate::db::auth_lock(&tx, true).await.unwrap();
        let view = super::super::authorization::begin_authorization(&tx, &f.app.auth, &ctx, &input)
            .await
            .unwrap();
        let outcome = super::super::authorization::decide_consent(
            &tx,
            &f.app.auth,
            &ctx,
            view.authorization_request_id,
            true,
            &view.csrf_token,
        )
        .await
        .unwrap();
        let super::super::authorization::ConsentOutcome::Approved { location } = outcome else {
            panic!("trusted snapshot fixture must approve")
        };
        tx.commit().await.unwrap();
        let code = url::Url::parse(&location)
            .unwrap()
            .query_pairs()
            .find(|(name, _)| name == "code")
            .unwrap()
            .1
            .into_owned();
        let exchange=super::super::tokens::exchange(&f.app.db,&f.app.auth,peer,Uuid::new_v4(),&json!({"grant_type":"authorization_code","client_id":client_id,"code":code,"redirect_uri":input.redirect_uri,"code_verifier":verifier}),Some("https://old.example")).await.unwrap();
        assert!(matches!(
            exchange,
            super::super::tokens::ExchangeOutcome::Token(_)
        ));
        validate_snapshot(&c).await.unwrap();
        let mut changed = spec;
        changed["redirect_uris"] = json!(["https://new.example/updated"]);
        changed["allowed_origins"] = json!(["https://new.example"]);
        changed["maximum_rules"] =
            json!([{"action":"audit.read","resource":{"kind":"audit","match":"all"}}]);
        super::super::clients::validate_spec(&changed).unwrap();
        c.execute("UPDATE clients SET spec=$2,revision=revision+1,status='disabled',updated_at=now() WHERE id=$1",&[&client,&changed]).await.unwrap();
        super::super::revoke_family(
            &c,
            c.query_one(
                "SELECT id FROM oauth_families WHERE client_id=$1",
                &[&client_id],
            )
            .await
            .unwrap()
            .get(0),
        )
        .await
        .unwrap();
        validate_snapshot(&c).await.unwrap();
        let original: Uuid = c
            .query_one(
                "SELECT session_id FROM oauth_families WHERE client_id=$1",
                &[&client_id],
            )
            .await
            .unwrap()
            .get(0);
        let other = Uuid::new_v4();
        let (_, digest) = f.app.auth.random_token();
        c.execute("INSERT INTO credentials(id,owner_id,kind,hmac_key_id,verification_digest,client_id,status,conditions,expires_at,created_at,updated_at) VALUES($1,$2,'session',$3,$4,'xelabctl','active','{}',now()+interval '1 hour',now(),now())",&[&other,&ctx.subject,&f.app.auth.key_id.as_str(),&digest]).await.unwrap();
        c.execute(
            "UPDATE oauth_families SET session_id=$2 WHERE client_id=$1",
            &[&client_id, &other],
        )
        .await
        .unwrap();
        assert!(
            validate_snapshot(&c).await.is_err(),
            "historical metadata cannot replace the token's original session source"
        );
        c.execute(
            "UPDATE oauth_families SET session_id=$2 WHERE client_id=$1",
            &[&client_id, &original],
        )
        .await
        .unwrap();
        c.execute(
            "UPDATE authorization_requests SET fixed_rules='[]' WHERE id=$1",
            &[&view.authorization_request_id],
        )
        .await
        .unwrap();
        assert!(
            validate_snapshot(&c).await.is_err(),
            "rules and frozen paths must stay bound even after trust was revoked"
        );
    }
}
