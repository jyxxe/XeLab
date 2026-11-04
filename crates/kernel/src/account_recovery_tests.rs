//! Actual PostgreSQL/identity behavior. Recovery readiness here is a metadata
//! fixture, not filesystem/off-host recovery or execution of the host CLI.
use super::*;
use crate::{auth::Resource, backups::tests::Fixture};

const OLD: &str = "old historical user password";
const OWNER_PASSWORD: &str = "fresh recovery owner password";
const USER_PASSWORD: &str = "fresh recovered user password";

struct Recovered {
    f: Fixture,
    child: Uuid,
    independent: Uuid,
    ctx: Context,
    stale: Context,
    old_user_token: String,
}
impl Recovered {
    async fn new() -> Option<Self> {
        let f = Fixture::new().await?;
        let mut c = f.app.db.connect().await.unwrap();
        let peer = "127.0.0.1".parse().unwrap();
        let stale = f
            .app
            .auth
            .authenticate(&c, &f.token, peer, Uuid::new_v4())
            .await
            .unwrap();
        let child = Uuid::new_v4();
        let independent = Uuid::new_v4();
        let hash = auth::password_hash(OLD).unwrap();
        for (id, parent, root) in [
            (child, Some(stale.subject), false),
            (independent, None, true),
        ] {
            c.execute("INSERT INTO accounts(id,parent_id,kind,status,display_name,password_hash,independent_owner,created_at,updated_at) VALUES($1,$2,'user','active','historical activation fixture',$3,$4,now(),now())", &[&id,&parent,&hash,&root]).await.unwrap();
            c.execute(
                "INSERT INTO account_closure(ancestor_id,descendant_id,depth) VALUES($1,$1,0)",
                &[&id],
            )
            .await
            .unwrap();
            c.execute("INSERT INTO quotas(account_id,spec,daily_bucket,created_at,updated_at) SELECT $1,spec,current_date,now(),now() FROM quotas WHERE account_id=$2", &[&id,&stale.subject]).await.unwrap();
        }
        c.execute(
            "INSERT INTO account_closure(ancestor_id,descendant_id,depth) VALUES($1,$2,1)",
            &[&stale.subject, &child],
        )
        .await
        .unwrap();
        c.execute("INSERT INTO inheritance_grants(account_id,parent_id,mode,issuer_credential_id,created_at,updated_at) VALUES($1,$2,'follow_parent',$3,now(),now())", &[&child,&stale.subject,&stale.credential]).await.unwrap();
        c.execute("INSERT INTO grants(id,subject_id,issuer_id,role_id,scope,status,created_at,updated_at) SELECT gen_random_uuid(),$1,$1,role_id,scope,'active',(SELECT created_at FROM accounts WHERE id=$1),now() FROM grants WHERE subject_id=$2 AND role_id='Owner'", &[&independent,&stale.subject]).await.unwrap();
        let old = f
            .app
            .auth
            .login(&c, independent, OLD.into(), peer, Uuid::new_v4())
            .await
            .unwrap();
        let recovery = Uuid::new_v4();
        c.execute("INSERT INTO platform_recovery_state(recovery_id,backup_id,phase) VALUES($1,$2,'importing')", &[&recovery,&f.backup]).await.unwrap();
        let tx = c.transaction().await.unwrap();
        crate::recovery::reset_rows(&tx, stale.subject, OWNER_PASSWORD, recovery)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        // Metadata-only component fixture: no claim of a physical restore.
        c.execute(
            "UPDATE platform_recovery_state SET phase='ready' WHERE recovery_id=$1",
            &[&recovery],
        )
        .await
        .unwrap();
        let fresh = f
            .app
            .auth
            .login(
                &c,
                stale.subject,
                OWNER_PASSWORD.into(),
                peer,
                Uuid::new_v4(),
            )
            .await
            .unwrap();
        let ctx = f
            .app
            .auth
            .authenticate(
                &c,
                fresh["access_token"].as_str().unwrap(),
                peer,
                Uuid::new_v4(),
            )
            .await
            .unwrap();
        Some(Self {
            f,
            child,
            independent,
            ctx,
            stale,
            old_user_token: old["access_token"].as_str().unwrap().into(),
        })
    }
    async fn request(&self, c: &(impl GenericClient + Sync), account: Uuid) -> Request {
        Request {
            account,
            confirm_account: account,
            operation: Uuid::new_v4(),
            revision: c
                .query_one("SELECT revision FROM accounts WHERE id=$1", &[&account])
                .await
                .unwrap()
                .get(0),
        }
    }
}

#[tokio::test]
async fn actual_postgres_reactivation_sets_fresh_password_without_restoring_authority() {
    let Some(f) = Recovered::new().await else {
        return;
    };
    let auth = &f.f.app.auth;
    let mut c = f.f.app.db.connect().await.unwrap();
    for account in [f.child, f.independent] {
        assert!(auth
            .login(&c, account, OLD.into(), f.ctx.peer, Uuid::new_v4())
            .await
            .is_err());
        let request = f.request(&c, account).await;
        let operation = request.operation;
        let revision = request.revision;
        let tx = c.transaction().await.unwrap();
        db::auth_lock(&tx, true).await.unwrap();
        let response = issue_rows(&tx, auth, &f.ctx, request).await.unwrap();
        tx.commit().await.unwrap();
        let token = response["activation_token"].as_str().unwrap();
        let pending: Value = c
            .query_one(
                "SELECT to_jsonb(r) FROM recovered_account_activations r WHERE account_id=$1",
                &[&account],
            )
            .await
            .unwrap()
            .get(0);
        assert!(!pending.to_string().contains(token));
        assert!(!db::operation_value(&c, operation)
            .await
            .unwrap()
            .to_string()
            .contains(token));
        let audit: Value = c
            .query_one(
                "SELECT jsonb_agg(to_jsonb(a)) FROM audit_events a WHERE request_id=$1",
                &[&operation],
            )
            .await
            .unwrap()
            .get(0);
        assert!(!audit.to_string().contains(token));
        let repeated = c.transaction().await.unwrap();
        assert_eq!(
            issue_rows(
                &repeated,
                auth,
                &f.ctx,
                Request {
                    account,
                    confirm_account: account,
                    operation,
                    revision
                }
            )
            .await
            .unwrap_err()
            .code,
            "ONE_TIME_RESPONSE_UNAVAILABLE"
        );
        repeated.rollback().await.unwrap();
        let tx = c.transaction().await.unwrap();
        db::auth_lock(&tx, true).await.unwrap();
        let activated = identity::activate(
            &tx,
            auth,
            &json!({"activation_token":token,"password":USER_PASSWORD}),
            f.ctx.peer,
            Uuid::new_v4(),
        )
        .await
        .unwrap();
        assert_eq!(activated["id"], account.to_string());
        tx.commit().await.unwrap();
        assert!(identity::activate(
            &c,
            auth,
            &json!({"activation_token":token,"password":USER_PASSWORD}),
            f.ctx.peer,
            Uuid::new_v4()
        )
        .await
        .is_err());
        let logged = auth
            .login(
                &c,
                account,
                USER_PASSWORD.into(),
                f.ctx.peer,
                Uuid::new_v4(),
            )
            .await
            .unwrap();
        let user = auth
            .authenticate(
                &c,
                logged["access_token"].as_str().unwrap(),
                f.ctx.peer,
                Uuid::new_v4(),
            )
            .await
            .unwrap();
        assert!(auth::authorize(
            &c,
            &user,
            "plugin.deploy",
            &Resource::new("deployment", "new", Some(account))
        )
        .await
        .is_err());
        assert_eq!(
            c.query_one(
                "SELECT count(*) FROM grants WHERE subject_id=$1 AND status='active'",
                &[&account]
            )
            .await
            .unwrap()
            .get::<_, i64>(0),
            0
        );
        assert_eq!(
            c.query_one(
                "SELECT state FROM recovered_account_activations WHERE account_id=$1",
                &[&account]
            )
            .await
            .unwrap()
            .get::<_, String>(0),
            "used"
        );
    }
    assert!(auth
        .authenticate(&c, &f.old_user_token, f.ctx.peer, Uuid::new_v4())
        .await
        .is_err());
}

#[tokio::test]
async fn actual_postgres_reactivation_requires_original_live_issuer_and_explicit_reissue() {
    let Some(f) = Recovered::new().await else {
        return;
    };
    let auth = &f.f.app.auth;
    let mut c = f.f.app.db.connect().await.unwrap();
    for mismatch in ["confirm", "revision", "old_context"] {
        let mut request = f.request(&c, f.child).await;
        if mismatch == "confirm" {
            request.confirm_account = Uuid::new_v4();
        }
        if mismatch == "revision" {
            request.revision += 1;
        }
        let tx = c.transaction().await.unwrap();
        assert!(issue_rows(
            &tx,
            auth,
            if mismatch == "old_context" {
                &f.stale
            } else {
                &f.ctx
            },
            request
        )
        .await
        .is_err());
        tx.rollback().await.unwrap();
    }
    for change in ["archived", "service", "hash", "expired", "new_account"] {
        let tx = c.transaction().await.unwrap();
        match change {
            "archived" => {
                tx.execute(
                    "UPDATE accounts SET status='archived' WHERE id=$1",
                    &[&f.child],
                )
                .await
                .unwrap();
            }
            "service" => {
                tx.execute(
                    "UPDATE accounts SET kind='service' WHERE id=$1",
                    &[&f.child],
                )
                .await
                .unwrap();
            }
            "hash" => {
                tx.execute(
                    "UPDATE accounts SET password_hash=$2 WHERE id=$1",
                    &[&f.child, &auth::password_hash(USER_PASSWORD).unwrap()],
                )
                .await
                .unwrap();
            }
            "expired" => {
                tx.execute(
                    "UPDATE accounts SET expires_at=now()-interval '1 second' WHERE id=$1",
                    &[&f.child],
                )
                .await
                .unwrap();
            }
            _ => {
                tx.execute(
                    "UPDATE accounts SET created_at=now()+interval '1 day' WHERE id=$1",
                    &[&f.child],
                )
                .await
                .unwrap();
            }
        }
        assert!(
            issue_rows(&tx, auth, &f.ctx, f.request(&tx, f.child).await)
                .await
                .is_err(),
            "{change}"
        );
        tx.rollback().await.unwrap();
    }
    let tx = c.transaction().await.unwrap();
    let first = issue_rows(&tx, auth, &f.ctx, f.request(&tx, f.child).await)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let login = auth
        .login(
            &c,
            f.ctx.subject,
            OWNER_PASSWORD.into(),
            f.ctx.peer,
            Uuid::new_v4(),
        )
        .await
        .unwrap();
    let new_ctx = auth
        .authenticate(
            &c,
            login["access_token"].as_str().unwrap(),
            f.ctx.peer,
            Uuid::new_v4(),
        )
        .await
        .unwrap();
    c.execute(
        "UPDATE credentials SET status='revoked' WHERE id=$1",
        &[&f.ctx.credential],
    )
    .await
    .unwrap();
    let tx = c.transaction().await.unwrap();
    assert!(
        identity::activate(
            &tx,
            auth,
            &json!({"activation_token":first["activation_token"],"password":USER_PASSWORD}),
            f.ctx.peer,
            Uuid::new_v4()
        )
        .await
        .is_err(),
        "new valid Owner login cannot rescue old issuer"
    );
    tx.rollback().await.unwrap();
    let tx = c.transaction().await.unwrap();
    let second = issue_rows(&tx, auth, &new_ctx, f.request(&tx, f.child).await)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_ne!(first["activation_token"], second["activation_token"]);
    assert!(identity::activate(
        &c,
        auth,
        &json!({"activation_token":first["activation_token"],"password":USER_PASSWORD}),
        f.ctx.peer,
        Uuid::new_v4()
    )
    .await
    .is_err());
    let tx = c.transaction().await.unwrap();
    identity::activate(
        &tx,
        auth,
        &json!({"activation_token":second["activation_token"],"password":USER_PASSWORD}),
        f.ctx.peer,
        Uuid::new_v4(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let tx = c.transaction().await.unwrap();
    let pending = issue_rows(&tx, auth, &new_ctx, f.request(&tx, f.independent).await)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let next_recovery = Uuid::new_v4();
    c.execute(
        "UPDATE platform_recovery_state SET recovery_id=$1,phase='importing'",
        &[&next_recovery],
    )
    .await
    .unwrap();
    let tx = c.transaction().await.unwrap();
    crate::recovery::reset_rows(
        &tx,
        f.ctx.subject,
        "another fresh recovery owner password",
        next_recovery,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        c.query_one(
            "SELECT state FROM recovered_account_activations WHERE account_id=$1",
            &[&f.independent]
        )
        .await
        .unwrap()
        .get::<_, String>(0),
        "revoked"
    );
    assert!(identity::activate(
        &c,
        auth,
        &json!({"activation_token":pending["activation_token"],"password":USER_PASSWORD}),
        f.ctx.peer,
        Uuid::new_v4()
    )
    .await
    .is_err());
}

#[tokio::test]
#[ignore = "requires host admin, an isolated host lease, actual xelabctl and PostgreSQL; tools/test-xfs.sh"]
async fn actual_host_cli_reactivates_only_with_maintenance_and_current_owner_credential() {
    use std::os::unix::fs::PermissionsExt;
    crate::config::require_host_admin().unwrap();
    let f = Recovered::new()
        .await
        .expect("host CLI acceptance requires PostgreSQL");
    let mut c = f.f.app.db.connect().await.unwrap();
    let login =
        f.f.app
            .auth
            .login(
                &c,
                f.ctx.subject,
                OWNER_PASSWORD.into(),
                f.ctx.peer,
                Uuid::new_v4(),
            )
            .await
            .unwrap();
    let directory = f.f.app.config.root().join("recovery-cli");
    std::fs::create_dir(&directory).unwrap();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    let database: String = c
        .query_one("SELECT current_database()", &[])
        .await
        .unwrap()
        .get(0);
    let dsn = format!(
        "{} dbname={database}",
        std::env::var("XELAB_TEST_DATABASE_URL").unwrap()
    );
    let dsn_file = directory.join("dsn");
    let key_file = directory.join("hmac");
    let token_file = directory.join("owner-token");
    crate::tls::atomic_file(&dsn_file, dsn.as_bytes(), 0o600).unwrap();
    crate::tls::atomic_file(&key_file, &[69; 32], 0o600).unwrap();
    crate::tls::atomic_file(
        &token_file,
        login["access_token"].as_str().unwrap().as_bytes(),
        0o600,
    )
    .unwrap();
    let mut config = f.f.app.config.value.clone();
    config["database"]["dsn_file"] = json!(dsn_file);
    config["security"]["token_hmac_key_file"] = json!(key_file);
    // Maintenance loads the same protected files as the daemon, even though
    // this command does not start TLS or decrypt stored business secrets.
    for (section, field, bytes) in [
        ("security", "master_key_file", vec![23; 32]),
        (
            "server",
            "tls_key_file",
            b"unused maintenance TLS key".to_vec(),
        ),
        (
            "security",
            "workload_ca_key_file",
            b"unused maintenance CA key".to_vec(),
        ),
    ] {
        let path = directory.join(field);
        crate::tls::atomic_file(&path, &bytes, 0o600).unwrap();
        config[section][field] = json!(path);
    }
    let config_file = directory.join("kernel.toml");
    let text = toml::to_string(&config).unwrap();
    crate::tls::atomic_file(&config_file, text.as_bytes(), 0o600).unwrap();
    let binary = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("xelabctl");
    assert!(binary.is_file(), "build the actual xelabctl binary first");
    let request = f.request(&c, f.independent).await;
    let args = vec![
        "recover-account".into(),
        "--config".into(),
        config_file.to_string_lossy().into_owned(),
        "--token-file".into(),
        token_file.to_string_lossy().into_owned(),
        "--account-id".into(),
        f.independent.to_string(),
        "--confirm-account-id".into(),
        f.independent.to_string(),
        "--operation-id".into(),
        request.operation.to_string(),
        "--revision".into(),
        request.revision.to_string(),
    ];
    let blocked = std::process::Command::new(&binary)
        .args(&args)
        .output()
        .unwrap();
    assert_eq!(
        blocked.status.code(),
        Some(5),
        "the live database leader must prevent maintenance"
    );
    assert!(String::from_utf8_lossy(&blocked.stderr)
        .contains("Stop the kernel before recovered-account maintenance"));
    assert!(c
        .query_opt(
            "SELECT account_id FROM recovered_account_activations WHERE account_id=$1",
            &[&f.independent]
        )
        .await
        .unwrap()
        .is_none());
    // This is the fixture's own leader connection, not another server/process.
    let leader_pid: i32 =
        f.f.app
            .leader
            .query_one("SELECT pg_backend_pid()", &[])
            .await
            .unwrap()
            .get(0);
    assert!(c
        .query_one("SELECT pg_terminate_backend($1,5000)", &[&leader_pid])
        .await
        .unwrap()
        .get::<_, bool>(0));
    crate::tls::atomic_file(&token_file, f.f.token.as_bytes(), 0o600).unwrap();
    let old = std::process::Command::new(&binary)
        .args(&args)
        .output()
        .unwrap();
    assert_eq!(
        old.status.code(),
        Some(3),
        "host admin cannot use a revoked historical credential"
    );
    assert!(old.stdout.is_empty());
    crate::tls::atomic_file(
        &token_file,
        login["access_token"].as_str().unwrap().as_bytes(),
        0o600,
    )
    .unwrap();
    let issued = std::process::Command::new(&binary)
        .args(&args)
        .output()
        .unwrap();
    assert!(
        issued.status.success(),
        "{}",
        String::from_utf8_lossy(&issued.stderr)
    );
    let response: Value = serde_json::from_slice(&issued.stdout).unwrap();
    assert_eq!(response["account_id"], f.independent.to_string());
    let repeated = std::process::Command::new(&binary)
        .args(&args)
        .output()
        .unwrap();
    assert_eq!(repeated.status.code(), Some(5));
    assert!(
        repeated.stdout.is_empty(),
        "a repeated command never returns the secret again"
    );
    let tx = c.transaction().await.unwrap();
    db::auth_lock(&tx, true).await.unwrap();
    identity::activate(
        &tx,
        &f.f.app.auth,
        &json!({"activation_token":response["activation_token"],"password":USER_PASSWORD}),
        f.ctx.peer,
        Uuid::new_v4(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        c.query_one(
            "SELECT count(*) FROM grants WHERE subject_id=$1 AND status='active'",
            &[&f.independent]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        0
    );
    println!("Recovered account host CLI: actual root/file/credential checks, live leader refusal, stopped-maintenance issuance, one-time response, parentless historical user activation and no old grant restoration; recovery gate is metadata component setup, not a physical/off-host restore claim");
}
