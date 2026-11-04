//! Actual PostgreSQL OAuth domain execution. Schema16 SQL is applied explicitly
//! to an isolated schema14 fixture: migration15/16 registry integration and HTTP
//! browser tests remain a separate root-owned gate, never claimed here.
use super::*;
use crate::{
    auth, contracts, db, identity,
    oauth::authorization::{self, AuthorizationInput, ConsentOutcome},
    oauth::tokens::{self, ExchangeOutcome},
    policy::Policy,
};
use axum::http::HeaderMap;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

const VERIFIER: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
struct Fixture {
    base: crate::backups::tests::Fixture,
    client: Value,
}
impl Fixture {
    async fn new(rules: Value) -> Option<Self> {
        let base = crate::backups::tests::Fixture::new().await?;
        let mut c = base.app.db.connect().await.unwrap();
        c.batch_execute(include_str!("../migrations/0016_oauth_state.sql"))
            .await
            .unwrap();
        let tx = c.transaction().await.unwrap();
        db::auth_lock(&tx, true).await.unwrap();
        let ctx = base
            .app
            .auth
            .authenticate(
                &tx,
                &base.token,
                "127.0.0.1".parse().unwrap(),
                Uuid::new_v4(),
            )
            .await
            .unwrap();
        let spec = json!({"name":"Actual OAuth UI","plugin_id":"org.xelab.webui","redirect_uris":["https://ui.example.org/callback?app=desktop"],"allowed_origins":["https://ui.example.org"],"maximum_rules":rules});
        clients::authorize(&tx, &ctx, "createClient", &HashMap::new(), &spec)
            .await
            .unwrap();
        let (client, status) = clients::execute(
            &tx,
            &base.app.auth,
            &ctx,
            "createClient",
            &HashMap::new(),
            &spec,
            &HeaderMap::new(),
            "",
        )
        .await
        .unwrap();
        assert_eq!(status, 201);
        tx.commit().await.unwrap();
        Some(Self { base, client })
    }
    fn input(&self) -> AuthorizationInput {
        AuthorizationInput {
            client_id: self.client["client_id"].as_str().unwrap().into(),
            redirect_uri: self.client["spec"]["redirect_uris"][0]
                .as_str()
                .unwrap()
                .into(),
            state: "state-bound-to-original-browser".into(),
            code_challenge: URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER.as_bytes())),
            scope: format!("client:{}", self.client["client_id"].as_str().unwrap()),
        }
    }
    async fn ctx(&self, c: &(impl tokio_postgres::GenericClient + Sync), token: &str) -> Context {
        self.base
            .app
            .auth
            .authenticate(c, token, "127.0.0.1".parse().unwrap(), Uuid::new_v4())
            .await
            .unwrap()
    }
    async fn code(&self, session: Option<&str>) -> String {
        let mut c = self.base.app.db.connect().await.unwrap();
        let tx = c.transaction().await.unwrap();
        db::auth_lock(&tx, true).await.unwrap();
        let ctx = self.ctx(&tx, session.unwrap_or(&self.base.token)).await;
        let view =
            authorization::begin_authorization(&tx, &self.base.app.auth, &ctx, &self.input())
                .await
                .unwrap();
        let result = authorization::decide_consent(
            &tx,
            &self.base.app.auth,
            &ctx,
            view.authorization_request_id,
            true,
            &view.csrf_token,
        )
        .await
        .unwrap();
        let ConsentOutcome::Approved { location } = result else {
            panic!("approved consent must issue code")
        };
        let url = url::Url::parse(&location).unwrap();
        assert_eq!(url.origin().ascii_serialization(), "https://ui.example.org");
        let code = url
            .query_pairs()
            .find(|(k, _)| k == "code")
            .unwrap()
            .1
            .into_owned();
        assert_eq!(
            url.query_pairs().find(|(k, _)| k == "state").unwrap().1,
            self.input().state
        );
        tx.commit().await.unwrap();
        code
    }
    fn code_request(&self, code: &str) -> Value {
        json!({"grant_type":"authorization_code","client_id":self.client["client_id"],"code":code,"redirect_uri":self.input().redirect_uri,"code_verifier":VERIFIER})
    }
    async fn exchange(&self, input: &Value) -> ExchangeOutcome {
        tokens::exchange(
            &self.base.app.db,
            &self.base.app.auth,
            "127.0.0.1".parse().unwrap(),
            Uuid::new_v4(),
            input,
            Some("https://ui.example.org"),
        )
        .await
        .unwrap()
    }
    async fn token(&self) -> Value {
        let code = self.code(None).await;
        match self.exchange(&self.code_request(&code)).await {
            ExchangeOutcome::Token(v) => v,
            ExchangeOutcome::Rejected { body, .. } => panic!("unexpected rejection {body}"),
        }
    }
    fn refresh_request(&self, token: &Value) -> Value {
        json!({"grant_type":"refresh_token","client_id":self.client["client_id"],"refresh_token":token["refresh_token"]})
    }
}
fn read_rule() -> Value {
    json!({"action":"platform.read","resource":{"kind":"platform","match":"all"}})
}
fn rejection(value: ExchangeOutcome, expected: &str) {
    match value {
        ExchangeOutcome::Rejected { body, .. } => {
            contracts::validate("OAuthError", &body).unwrap();
            assert_eq!(body["error"], expected)
        }
        ExchangeOutcome::Token(_) => panic!("request must be rejected"),
    }
}

#[test]
fn authorization_query_requires_exact_unambiguous_utf8_parameters() {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.extend_pairs([
        ("response_type", "code"),
        ("client_id", "xelab-component-client"),
        ("redirect_uri", "https://ui.example.org/callback"),
        ("state", "original-state"),
        ("code_challenge", VERIFIER),
        ("code_challenge_method", "S256"),
        ("scope", "client:xelab-component-client"),
    ]);
    let query = query.finish();
    assert_eq!(
        authorization::parse_query(&query).unwrap().state,
        "original-state"
    );
    for bad in [
        query.replace("original-state", "%FF"),
        query.replace("original-state", "%GG"),
        query.replace("original-state", "%"),
        query.replace(
            "state=original-state",
            "state=original-state&st%61te=second",
        ),
        query.replace("state=original-state", "state=original-state&unknown=value"),
        query.replace("response_type=code", "response_type=token"),
        query.replace("code_challenge_method=S256", "code_challenge_method=plain"),
        format!("{query}&"),
    ] {
        assert!(
            authorization::parse_query(&bad).is_err(),
            "ambiguous OAuth request accepted"
        );
    }
}

#[tokio::test]
async fn real_code_pkce_uri_client_origin_and_single_use_produce_scoped_opaque_credentials() {
    let Some(f) = Fixture::new(json!([read_rule()])).await else {
        return;
    };
    let code = f.code(None).await;
    for (field, value) in [
        ("redirect_uri", json!("https://attacker.invalid/callback")),
        ("code_verifier", json!("b".repeat(43))),
        ("client_id", json!("another-client")),
    ] {
        let mut request = f.code_request(&code);
        request[field] = value;
        rejection(
            f.exchange(&request).await,
            if field == "client_id" {
                "invalid_client"
            } else {
                "invalid_grant"
            },
        );
    }
    let rejected = tokens::exchange(
        &f.base.app.db,
        &f.base.app.auth,
        "127.0.0.1".parse().unwrap(),
        Uuid::new_v4(),
        &f.code_request(&code),
        Some("https://attacker.invalid"),
    )
    .await
    .unwrap();
    rejection(rejected, "invalid_client");
    let response = match f.exchange(&f.code_request(&code)).await {
        ExchangeOutcome::Token(v) => v,
        _ => panic!("valid PKCE must exchange"),
    };
    contracts::validate("OAuthToken", &response).unwrap();
    assert_eq!(response["expires_in"], 900);
    rejection(f.exchange(&f.code_request(&code)).await, "invalid_grant");
    let c = f.base.app.db.connect().await.unwrap();
    let expired = f.code(None).await;
    c.execute(
        "UPDATE authorization_requests SET expires_at=now()-interval '1 second' WHERE code_digest=$1",
        &[&f.base.app.auth.token_digest(&expired)],
    )
    .await
    .unwrap();
    rejection(f.exchange(&f.code_request(&expired)).await, "invalid_grant");
    let ctx = f.ctx(&c, response["access_token"].as_str().unwrap()).await;
    auth::authorize(
        &c,
        &ctx,
        "platform.read",
        &auth::Resource::new("platform", "node", None),
    )
    .await
    .unwrap();
    assert!(
        auth::authorize(
            &c,
            &ctx,
            "credential.api-key.create",
            &auth::Resource::new("account", ctx.subject, Some(ctx.subject))
        )
        .await
        .is_err(),
        "self-service cannot escape fixed OAuth ceiling"
    );
    assert!(
        f.base
            .app
            .auth
            .authenticate(
                &c,
                response["refresh_token"].as_str().unwrap(),
                ctx.peer,
                Uuid::new_v4()
            )
            .await
            .is_err(),
        "refresh is never an API Bearer"
    );
    let row = c
        .query_one(
            "SELECT row_to_json(k)::text AS persisted,k.family_id FROM credentials k WHERE id=$1",
            &[&ctx.credential],
        )
        .await
        .unwrap();
    let persisted: String = row.get("persisted");
    assert!(!persisted.contains(response["access_token"].as_str().unwrap()));
    let b = binding(&c, ctx.credential).await.unwrap();
    assert_eq!(b.client_id, f.input().client_id);
    assert_eq!(b.origin, "https://ui.example.org");
    assert_eq!(b.subject_id, ctx.subject);
    assert!(
        validate_origin(&c, ctx.credential, Some("https://other.example.org"))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn concurrent_code_exchange_has_one_winner_and_does_not_regenerate_secrets() {
    let Some(f) = Fixture::new(json!([read_rule()])).await else {
        return;
    };
    let code = f.code(None).await;
    let request = f.code_request(&code);
    let (a, b) = tokio::join!(f.exchange(&request), f.exchange(&request));
    assert_eq!(
        usize::from(matches!(a, ExchangeOutcome::Token(_)))
            + usize::from(matches!(b, ExchangeOutcome::Token(_))),
        1
    );
    let c = f.base.app.db.connect().await.unwrap();
    assert_eq!(
        c.query_one("SELECT count(*) FROM oauth_families", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        1
    );
    assert_eq!(
        c.query_one(
            "SELECT count(*) FROM credentials WHERE kind IN('oauth_access','oauth_refresh')",
            &[]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        2
    );
}

#[tokio::test]
async fn concurrent_refresh_reuse_commits_family_revocation_including_the_winners_new_tokens() {
    let Some(f) = Fixture::new(json!([read_rule()])).await else {
        return;
    };
    let original = f.token().await;
    let request = f.refresh_request(&original);
    let (a, b) = tokio::join!(f.exchange(&request), f.exchange(&request));
    let winner = match (a, b) {
        (ExchangeOutcome::Token(v), bad) | (bad, ExchangeOutcome::Token(v)) => {
            rejection(bad, "invalid_grant");
            v
        }
        _ => panic!("exactly one refresh wins"),
    };
    let c = f.base.app.db.connect().await.unwrap();
    for token in [
        &original["access_token"],
        &winner["access_token"],
        &winner["refresh_token"],
    ] {
        assert!(f
            .base
            .app
            .auth
            .authenticate(
                &c,
                token.as_str().unwrap(),
                "127.0.0.1".parse().unwrap(),
                Uuid::new_v4()
            )
            .await
            .is_err());
    }
    assert_eq!(
        c.query_one(
            "SELECT count(*) FROM oauth_families WHERE status='revoked'",
            &[]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        1
    );
    assert_eq!(c.query_one("SELECT count(*) FROM credentials WHERE kind IN('oauth_access','oauth_refresh') AND status='active'",&[]).await.unwrap().get::<_,i64>(0),0);
    assert_eq!(
        c.query_one(
            "SELECT count(*) FROM audit_events WHERE reason_code='OAUTH_REFRESH_REUSE'",
            &[]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        1,
        "logical OAuth error must commit audit and revocation"
    );
    rejection(
        f.exchange(&f.refresh_request(&winner)).await,
        "invalid_grant",
    );
}

#[tokio::test]
async fn refresh_rotation_keeps_root_session_absolute_expiry_and_original_grant_paths() {
    let Some(f) = Fixture::new(json!([read_rule()])).await else {
        return;
    };
    let original = f.token().await;
    let c = f.base.app.db.connect().await.unwrap();
    let old = f.ctx(&c, original["access_token"].as_str().unwrap()).await;
    let b = binding(&c, old.credential).await.unwrap();
    let original_expiry = b.expires_at;
    let next = match f.exchange(&f.refresh_request(&original)).await {
        ExchangeOutcome::Token(v) => v,
        _ => panic!("fresh refresh must rotate"),
    };
    let fresh = f.ctx(&c, next["access_token"].as_str().unwrap()).await;
    let binding = binding(&c, fresh.credential).await.unwrap();
    assert_eq!(binding.family_id, b.family_id);
    assert_eq!(binding.expires_at, original_expiry);
    let issuer: Uuid = c
        .query_one(
            "SELECT issuer_credential_id FROM credentials WHERE id=$1",
            &[&fresh.credential],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(issuer, b.session_id);
    assert!(
        original_expiry
            <= c.query_one(
                "SELECT expires_at FROM credentials WHERE id=$1",
                &[&b.session_id]
            )
            .await
            .unwrap()
            .get::<_, DateTime<Utc>>(0)
    );
    let source = c
        .query_one(
            "SELECT source_grant_id FROM grant_provenance WHERE derived_credential_id=$1 LIMIT 1",
            &[&fresh.credential],
        )
        .await
        .unwrap()
        .get::<_, Uuid>(0);
    // A second trusted metadata proof now provides the same right. It must not
    // replace the source frozen into the original OAuth consent.
    c.execute("INSERT INTO grants(id,subject_id,issuer_id,issuer_credential_id,role_id,scope,status,created_at,updated_at) SELECT $1,subject_id,issuer_id,issuer_credential_id,role_id,scope,'active',created_at,now() FROM grants WHERE id=$2", &[&Uuid::new_v4(),&source]).await.unwrap();
    c.execute("UPDATE grants SET status='revoked' WHERE id=$1", &[&source])
        .await
        .unwrap();
    let root = f.ctx(&c, &f.base.token).await;
    auth::authorize(
        &c,
        &root,
        "platform.read",
        &auth::Resource::new("platform", "node", None),
    )
    .await
    .unwrap();
    assert!(auth::authorize(
        &c,
        &fresh,
        "platform.read",
        &auth::Resource::new("platform", "node", None)
    )
    .await
    .is_err());
    rejection(f.exchange(&f.refresh_request(&next)).await, "invalid_grant");
}

#[tokio::test]
async fn guest_login_intersects_owner_client_ceiling_without_gaining_future_grants_or_self_service()
{
    let Some(f) = Fixture::new(json!([read_rule()])).await else {
        return;
    };
    let mut c = f.base.app.db.connect().await.unwrap();
    let guest = Uuid::new_v4();
    let password = "guest OAuth correct horse password";
    c.execute("INSERT INTO accounts(id,kind,status,display_name,password_hash,created_at,updated_at) VALUES($1,'user','active','Guest OAuth',$2,now(),now())", &[&guest,&auth::password_hash(password).unwrap()]).await.unwrap();
    c.execute(
        "INSERT INTO account_closure(ancestor_id,descendant_id,depth) VALUES($1,$1,0)",
        &[&guest],
    )
    .await
    .unwrap();
    let login = f
        .base
        .app
        .auth
        .login(
            &c,
            guest,
            password.into(),
            "127.0.0.1".parse().unwrap(),
            Uuid::new_v4(),
        )
        .await
        .unwrap();
    let tx = c.transaction().await.unwrap();
    db::auth_lock(&tx, true).await.unwrap();
    let owner = f.ctx(&tx, &f.base.token).await;
    let guest_session = f.ctx(&tx, login["access_token"].as_str().unwrap()).await;
    let owner_consent =
        authorization::begin_authorization(&tx, &f.base.app.auth, &owner, &f.input())
            .await
            .unwrap();
    assert_eq!(
        authorization::load_consent(
            &tx,
            &f.base.app.auth,
            &guest_session,
            owner_consent.authorization_request_id
        )
        .await
        .err()
        .unwrap()
        .code,
        "RESOURCE_NOT_FOUND"
    );
    assert_eq!(
        authorization::decide_consent(
            &tx,
            &f.base.app.auth,
            &guest_session,
            owner_consent.authorization_request_id,
            true,
            &owner_consent.csrf_token
        )
        .await
        .err()
        .unwrap()
        .code,
        "RESOURCE_NOT_FOUND"
    );
    tx.rollback().await.unwrap();
    let code = f.code(Some(login["access_token"].as_str().unwrap())).await;
    let token = match f.exchange(&f.code_request(&code)).await {
        ExchangeOutcome::Token(v) => v,
        _ => panic!("empty approved intersection still identifies Guest"),
    };
    let ctx = f.ctx(&c, token["access_token"].as_str().unwrap()).await;
    assert_eq!(ctx.subject, guest);
    assert!(Policy::new(&c, ctx.peer)
        .credential(ctx.credential, 0)
        .await
        .unwrap()
        .is_empty());
    let root = f.ctx(&c, &f.base.token).await;
    c.execute("INSERT INTO grants(id,subject_id,issuer_id,issuer_credential_id,role_id,scope,status,created_at,updated_at) VALUES($1,$2,$3,$4,'Auditor',$5,'active',now(),now())", &[&Uuid::new_v4(),&guest,&root.subject,&root.credential,&json!({"kind":"platform","match":"all"})]).await.unwrap();
    let guest_session = f.ctx(&c, login["access_token"].as_str().unwrap()).await;
    auth::authorize(
        &c,
        &guest_session,
        "platform.read",
        &auth::Resource::new("platform", "node", None),
    )
    .await
    .unwrap();
    assert!(
        auth::authorize(
            &c,
            &ctx,
            "platform.read",
            &auth::Resource::new("platform", "node", None)
        )
        .await
        .is_err(),
        "old empty ceiling cannot expand through later grants"
    );
    let tx = c.transaction().await.unwrap();
    db::auth_lock(&tx, true).await.unwrap();
    assert!(
        authorization::begin_authorization(&tx, &f.base.app.auth, &ctx, &f.input())
            .await
            .is_err(),
        "OAuth token cannot impersonate trusted Cookie consent session"
    );
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn login_continuation_is_single_use_and_consent_is_session_csrf_client_bound() {
    let Some(f) = Fixture::new(json!([read_rule()])).await else {
        return;
    };
    let mut c = f.base.app.db.connect().await.unwrap();
    let tx = c.transaction().await.unwrap();
    db::auth_lock(&tx, true).await.unwrap();
    let ctx = f.ctx(&tx, &f.base.token).await;
    let prepared = authorization::prepare_login(&tx, &f.base.app.auth, &f.input(), ctx.peer)
        .await
        .unwrap();
    let view = authorization::resume_login(&tx, &f.base.app.auth, &ctx, &prepared.token)
        .await
        .unwrap();
    assert!(
        authorization::resume_login(&tx, &f.base.app.auth, &ctx, &prepared.token)
            .await
            .is_err()
    );
    assert!(authorization::decide_consent(
        &tx,
        &f.base.app.auth,
        &ctx,
        view.authorization_request_id,
        true,
        "bad-csrf"
    )
    .await
    .is_err());
    assert_eq!(
        tx.query_one(
            "SELECT status FROM authorization_requests WHERE id=$1",
            &[&view.authorization_request_id]
        )
        .await
        .unwrap()
        .get::<_, String>(0),
        "pending"
    );
    let again =
        authorization::load_consent(&tx, &f.base.app.auth, &ctx, view.authorization_request_id)
            .await
            .unwrap();
    assert_eq!(again.csrf_token, view.csrf_token);
    let denied = authorization::decide_consent(
        &tx,
        &f.base.app.auth,
        &ctx,
        view.authorization_request_id,
        false,
        &view.csrf_token,
    )
    .await
    .unwrap();
    let ConsentOutcome::Denied { location } = denied else {
        panic!("consent refusal cannot mintcode")
    };
    assert!(location.contains("error=access_denied"));
    assert_eq!(
        tx.query_one(
            "SELECT count(*) FROM credentials WHERE kind LIKE 'oauth_%'",
            &[]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        0
    );
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn oauth_derived_api_key_keeps_delegating_credential_and_client_disable_withdraws_it() {
    let Some(f)=Fixture::new(json!([read_rule(),{"action":"credential.api-key.create","resource":{"kind":"account","match":"all"}}])).await else{return;};
    let token = f.token().await;
    let mut c = f.base.app.db.connect().await.unwrap();
    let tx = c.transaction().await.unwrap();
    db::auth_lock(&tx, true).await.unwrap();
    let delegated = f.ctx(&tx, token["access_token"].as_str().unwrap()).await;
    let params = HashMap::from([("account_id".into(), delegated.subject.to_string())]);
    let request = json!({"name":"derived through OAuth","inheritance":{"mode":"selected","rules":[read_rule()]},"conditions":{"expires_at":Utc::now()+chrono::Duration::seconds(300)}});
    identity::authorize(&tx, &delegated, "createApiKey", &params, &request)
        .await
        .unwrap();
    let (key, status) = identity::execute(
        &tx,
        &f.base.app.auth,
        &delegated,
        "createApiKey",
        &params,
        &request,
        &HeaderMap::new(),
        "",
    )
    .await
    .unwrap();
    assert_eq!(status, 201);
    assert_eq!(
        tx.query_one(
            "SELECT issuer_credential_id FROM credentials WHERE id=$1",
            &[&contracts::id(&key["credential"], "id").unwrap()]
        )
        .await
        .unwrap()
        .get::<_, Uuid>(0),
        delegated.credential
    );
    tx.commit().await.unwrap();
    f.ctx(&c, key["secret"].as_str().unwrap()).await;
    let tx = c.transaction().await.unwrap();
    db::auth_lock(&tx, true).await.unwrap();
    let owner = f.ctx(&tx, &f.base.token).await;
    let params = HashMap::from([("client_id".into(), f.client["id"].as_str().unwrap().into())]);
    let mut headers = HeaderMap::new();
    headers.insert("if-match", "\"rev-1\"".parse().unwrap());
    clients::authorize(&tx, &owner, "disableClient", &params, &Value::Null)
        .await
        .unwrap();
    clients::execute(
        &tx,
        &f.base.app.auth,
        &owner,
        "disableClient",
        &params,
        &Value::Null,
        &headers,
        "",
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert!(f
        .base
        .app
        .auth
        .authenticate(
            &c,
            key["secret"].as_str().unwrap(),
            owner.peer,
            Uuid::new_v4()
        )
        .await
        .is_err());
    assert!(f
        .base
        .app
        .auth
        .authenticate(
            &c,
            token["access_token"].as_str().unwrap(),
            owner.peer,
            Uuid::new_v4()
        )
        .await
        .is_err());
    f.ctx(&c, &f.base.token).await;
    rejection(
        f.exchange(&f.refresh_request(&token)).await,
        "invalid_client",
    );
}

#[tokio::test]
async fn session_logout_revokes_only_its_families_and_recovery_discards_all_oauth_authority() {
    let Some(f) = Fixture::new(json!([read_rule()])).await else {
        return;
    };
    let token = f.token().await;
    let mut c = f.base.app.db.connect().await.unwrap();
    let root = f.ctx(&c, &f.base.token).await;
    let tx = c.transaction().await.unwrap();
    db::auth_lock(&tx, true).await.unwrap();
    revoke_session(&tx, root.credential).await.unwrap();
    tx.commit().await.unwrap();
    assert!(f
        .base
        .app
        .auth
        .authenticate(
            &c,
            token["access_token"].as_str().unwrap(),
            root.peer,
            Uuid::new_v4()
        )
        .await
        .is_err());
    f.ctx(&c, &f.base.token).await;
    let second = f.token().await;
    let tx = c.transaction().await.unwrap();
    let recovery = Uuid::new_v4();
    tx.execute("INSERT INTO platform_recovery_state(recovery_id,backup_id,phase) VALUES($1,$2,'importing')", &[&recovery,&f.base.backup]).await.unwrap();
    crate::recovery::reset_rows(
        &tx,
        root.subject,
        "new OAuth recovery correct horse password",
        recovery,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        c.query_one(
            "SELECT count(*) FROM oauth_families WHERE status<>'revoked'",
            &[]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        0
    );
    assert!(f
        .base
        .app
        .auth
        .authenticate(
            &c,
            second["refresh_token"].as_str().unwrap(),
            root.peer,
            Uuid::new_v4()
        )
        .await
        .is_err());
    assert_eq!(
        c.query_one("SELECT count(*) FROM clients WHERE status<>'disabled'", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
}

#[tokio::test]
async fn original_grant_issuer_call_budget_and_cidr_survive_consent_and_refresh() {
    let Some(f) = Fixture::new(json!([read_rule()])).await else {
        return;
    };
    let c = f.base.app.db.connect().await.unwrap();
    let root = f.ctx(&c, &f.base.token).await;
    let guest = Uuid::new_v4();
    let password = "budget OAuth correct horse password";
    c.execute("INSERT INTO accounts(id,kind,status,display_name,password_hash,created_at,updated_at) VALUES($1,'user','active','Budget Guest',$2,now(),now())", &[&guest,&auth::password_hash(password).unwrap()]).await.unwrap();
    c.execute(
        "INSERT INTO account_closure(ancestor_id,descendant_id,depth) VALUES($1,$1,0)",
        &[&guest],
    )
    .await
    .unwrap();
    let issuer = Uuid::new_v4();
    let (_, digest) = f.base.app.auth.random_token();
    // A live ordinary API-key source delegates a single business-call budget.
    // Its selectors/conditions and journal remain authoritative after refresh.
    c.execute("INSERT INTO credentials(id,owner_id,kind,hmac_key_id,verification_digest,status,inheritance_mode,conditions,expires_at,created_at,updated_at) VALUES($1,$2,'api_key',$3,$4,'active','follow_parent',$5,now()+interval '1 hour',now(),now())", &[&issuer,&root.subject,&f.base.app.auth.key_id.as_str(),&digest,&json!({"max_calls":1,"source_cidrs":["127.0.0.0/8"]})]).await.unwrap();
    c.execute("INSERT INTO grants(id,subject_id,issuer_id,issuer_credential_id,role_id,scope,status,created_at,updated_at) VALUES($1,$2,$3,$4,'Auditor',$5,'active',now(),now())", &[&Uuid::new_v4(),&guest,&root.subject,&issuer,&json!({"kind":"platform","match":"all"})]).await.unwrap();
    let login = f
        .base
        .app
        .auth
        .login(&c, guest, password.into(), root.peer, Uuid::new_v4())
        .await
        .unwrap();
    let code = f.code(Some(login["access_token"].as_str().unwrap())).await;
    let token = match f.exchange(&f.code_request(&code)).await {
        ExchangeOutcome::Token(v) => v,
        _ => panic!("delegated budget must admit first token"),
    };
    let ctx = f.ctx(&c, token["access_token"].as_str().unwrap()).await;
    let resource = auth::Resource::new("platform", "node", None);
    let charged = identity::call_sources_for(&c, &ctx, "platform.read", &resource)
        .await
        .unwrap();
    assert!(
        charged.contains(&issuer),
        "OAuth must bill the original grant issuer"
    );
    let other = f
        .base
        .app
        .auth
        .authenticate(
            &c,
            token["access_token"].as_str().unwrap(),
            "192.0.2.1".parse().unwrap(),
            Uuid::new_v4(),
        )
        .await
        .unwrap();
    assert!(
        auth::authorize(&c, &other, "platform.read", &resource)
            .await
            .is_err(),
        "CIDR cannot disappear at consent"
    );
    let call = Uuid::new_v4();
    c.execute("INSERT INTO calls(id,owner_id,credential_id,contract,input_digest,state,deadline_at,created_at) VALUES($1,$2,$3,'{}','oauth-component-budget','succeeded',now()+interval '1 minute',now())", &[&call,&guest,&ctx.credential]).await.unwrap();
    for source in charged {
        c.execute(
            "INSERT INTO call_credential_charges(call_id,credential_id) VALUES($1,$2)",
            &[&call, &source],
        )
        .await
        .unwrap();
    }
    assert_eq!(identity::key_calls(&c, issuer).await.unwrap(), 1);
    assert_eq!(
        identity::call_sources_for(&c, &ctx, "platform.read", &resource)
            .await
            .unwrap_err()
            .code,
        "QUOTA_EXCEEDED"
    );
    let next = match f.exchange(&f.refresh_request(&token)).await {
        ExchangeOutcome::Token(v) => v,
        _ => panic!("refresh rotates credentials without resetting business-call quota"),
    };
    let fresh = f.ctx(&c, next["access_token"].as_str().unwrap()).await;
    assert_eq!(
        identity::call_sources_for(&c, &fresh, "platform.read", &resource)
            .await
            .unwrap_err()
            .code,
        "QUOTA_EXCEEDED"
    );
}
