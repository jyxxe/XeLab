//! PostgreSQL transaction-local proof scope. No permissions are fabricated:
//! every path comes from a fresh authenticated caller's current Policy.
use super::*;

async fn scoped_context(
    f: &Fixture,
    tx: &tokio_postgres::Transaction<'_>,
    skill: Uuid,
    owner: Uuid,
) -> auth::Context {
    let mut ctx = f
        .app
        .auth
        .authenticate(tx, &f.token, "127.0.0.1".parse().unwrap(), Uuid::new_v4())
        .await
        .unwrap();
    let paths = crate::policy::Policy::for_context(tx, &ctx)
        .credential(ctx.credential, 0)
        .await
        .unwrap();
    ctx.proxy_paths = Some(paths);
    ctx.proxy_ceilings = vec![
        json!([{"action":"skill.read","resource":{"kind":"skill","match":"ids","ids":[skill]}}]),
        json!([{"action":"skill.read","resource":{"kind":"skill","match":"owned_by","owner_id":owner}}]),
    ];
    ctx.bind_proxy_proof(tx).await.unwrap();
    ctx
}

#[tokio::test]
async fn postgres_host_proxy_proof_is_bound_to_identity_and_actual_transaction() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (provider, _) = f.workload().await;
    let (target, _) = skill(&f, provider).await;
    let mut c = f.app.db.connect().await.unwrap();
    let owner: Uuid = c
        .query_one("SELECT created_by FROM skills WHERE id=$1", &[&target])
        .await
        .unwrap()
        .get(0);
    let resource = auth::Resource::new("skill", target, Some(owner));
    let tx = c.transaction().await.unwrap();
    crate::db::auth_lock(&tx, false).await.unwrap();
    let ctx = scoped_context(&f, &tx, target, owner).await;
    auth::authorize(&tx, &ctx, "skill.read", &resource)
        .await
        .unwrap();
    assert!(!crate::policy::Policy::for_context(&tx, &ctx)
        .credential(ctx.credential, 0)
        .await
        .unwrap()
        .is_empty());
    let old_nonce = ctx.proxy_proof.as_ref().unwrap().nonce().to_owned();
    let mut changed_peer = ctx.clone();
    changed_peer.peer = "192.0.2.17".parse().unwrap();
    assert!(
        auth::authorize(&tx, &changed_peer, "skill.read", &resource)
            .await
            .is_err(),
        "a copied transaction proof cannot change the authenticated network peer"
    );
    let mut changed_origin = ctx.clone();
    changed_origin.browser_origin = Some("https://different.example.org".into());
    assert!(
        auth::authorize(&tx, &changed_origin, "skill.read", &resource)
            .await
            .is_err(),
        "a copied proof cannot change the trusted browser-origin provenance"
    );
    let mut changed_credential = ctx.clone();
    changed_credential.credential = Uuid::new_v4();
    assert!(
        auth::authorize(&tx, &changed_credential, "skill.read", &resource)
            .await
            .is_err()
    );
    tx.commit().await.unwrap();
    let next = c.transaction().await.unwrap();
    crate::db::auth_lock(&next, false).await.unwrap();
    assert!(
        auth::authorize(&next, &ctx, "skill.read", &resource)
            .await
            .is_err(),
        "a committed transaction's proof is not reusable in the next transaction"
    );
    let fresh = scoped_context(&f, &next, target, owner).await;
    assert_ne!(fresh.proxy_proof.as_ref().unwrap().nonce(), old_nonce);
    auth::authorize(&next, &fresh, "skill.read", &resource)
        .await
        .unwrap();
    assert!(
        auth::authorize(&next, &ctx, "skill.read", &resource)
            .await
            .is_err(),
        "installing a fresh proof cannot revive an older Context with a different nonce/XID"
    );
    next.rollback().await.unwrap();
}

#[tokio::test]
async fn postgres_host_proxy_proof_does_not_freeze_credential_expiry_within_transaction() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (provider, _) = f.workload().await;
    let (target, _) = skill(&f, provider).await;
    let mut c = f.app.db.connect().await.unwrap();
    let owner: Uuid = c
        .query_one("SELECT created_by FROM skills WHERE id=$1", &[&target])
        .await
        .unwrap()
        .get(0);
    let actual = f
        .app
        .auth
        .authenticate(&c, &f.token, "127.0.0.1".parse().unwrap(), Uuid::new_v4())
        .await
        .unwrap();
    // Set up expiration before the shared authorization fence and marker.
    // No authorization rows are modified after the current proof is bound.
    let preparation = c.transaction().await.unwrap();
    crate::db::auth_lock(&preparation, true).await.unwrap();
    let expires:chrono::DateTime<chrono::Utc>=preparation.query_one("UPDATE credentials SET expires_at=clock_timestamp()+interval '3 seconds' WHERE id=$1 RETURNING expires_at",&[&actual.credential]).await.unwrap().get(0);
    preparation.commit().await.unwrap();
    let tx = c.transaction().await.unwrap();
    crate::db::auth_lock(&tx, false).await.unwrap();
    let ctx = scoped_context(&f, &tx, target, owner).await;
    let resource = auth::Resource::new("skill", target, Some(owner));
    auth::authorize(&tx, &ctx, "skill.read", &resource)
        .await
        .unwrap();
    let wait =
        (expires - chrono::Utc::now()).to_std().unwrap_or_default() + Duration::from_millis(50);
    tokio::time::sleep(wait).await;
    assert!(
        auth::authorize(&tx, &ctx, "skill.read", &resource)
            .await
            .is_err(),
        "same-transaction proof reuse must recheck time-based credential validity at the leaf"
    );
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn postgres_host_business_cache_rechecks_external_issuer_expiry_without_losing_alternatives()
{
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (provider, _) = f.workload().await;
    let (target, definition) = skill(&f, provider).await;
    let mut c = f.app.db.connect().await.unwrap();
    let caller = f
        .app
        .auth
        .authenticate(&c, &f.token, "127.0.0.1".parse().unwrap(), Uuid::new_v4())
        .await
        .unwrap();
    let contract = &definition["target"]["contract"];
    let business = auth::Resource::new(
        "capability",
        format!(
            "{}@{}",
            contract["name"].as_str().unwrap(),
            contract["version"].as_str().unwrap()
        ),
        None,
    );
    let proxy = auth::Resource::new("skill", target, Some(caller.subject));
    for credential_expiry in [false, true] {
        // Explicit authorization metadata fixture: an external account obtains
        // only the real echo capability from the existing bootstrap Owner,
        // then delegates that business scope back. Proxy Skill authority stays
        // on the caller's independent original Owner sources throughout.
        let preparation = c.transaction().await.unwrap();
        crate::db::auth_lock(&preparation, true).await.unwrap();
        let issuer = Uuid::new_v4();
        preparation.execute("INSERT INTO accounts(id,kind,status,display_name,created_at,updated_at) VALUES($1,'service','active','expiring business issuer',now(),now())",&[&issuer]).await.unwrap();
        preparation
            .execute(
                "INSERT INTO account_closure(ancestor_id,descendant_id,depth) VALUES($1,$1,0)",
                &[&issuer],
            )
            .await
            .unwrap();
        let business_scope = json!({"kind":"capability","match":"ids","ids":[business.id]});
        preparation.execute("INSERT INTO grants(id,subject_id,issuer_id,issuer_credential_id,role_id,scope,status,created_at,updated_at) VALUES($1,$2,$3,$4,'Owner',$5,'active',now(),now())",&[&Uuid::new_v4(),&issuer,&caller.subject,&caller.credential,&business_scope]).await.unwrap();
        let credential = if credential_expiry {
            let id = Uuid::new_v4();
            let (_, hash) = f.app.auth.random_token();
            preparation.execute("INSERT INTO credentials(id,owner_id,kind,hmac_key_id,verification_digest,status,conditions,expires_at,created_at,updated_at) VALUES($1,$2,'session',$3,$4,'active','{}',clock_timestamp()+interval '1 hour',now(),now())",&[&id,&issuer,&f.app.auth.key_id.as_str(),&hash]).await.unwrap();
            Some(id)
        } else {
            None
        };
        let delegated = Uuid::new_v4();
        preparation.execute("INSERT INTO grants(id,subject_id,issuer_id,issuer_credential_id,role_id,scope,status,created_at,updated_at) VALUES($1,$2,$3,$4,'Owner',$5,'active',now(),now())",&[&delegated,&caller.subject,&issuer,&credential,&business_scope]).await.unwrap();
        let paths = crate::policy::Policy::for_context(&preparation, &caller)
            .credential(caller.credential, 0)
            .await
            .unwrap();
        let original_business: Vec<_> = paths
            .iter()
            .filter(|path| {
                path.sources.contains(&delegated) && path.rule["action"] == "capability.invoke"
            })
            .cloned()
            .collect();
        let independent_business: Vec<_> = paths
            .iter()
            .filter(|path| {
                !path.sources.contains(&delegated) && path.rule["action"] == "capability.invoke"
            })
            .cloned()
            .collect();
        let proxy_paths: Vec<_> = paths
            .iter()
            .filter(|path| path.rule["action"] == "skill.read")
            .cloned()
            .collect();
        assert!(
            !original_business.is_empty(),
            "the external source must be proved by the actual Policy before exercising its expiry"
        );
        assert!(
            proxy_paths
                .iter()
                .all(|path| !path.sources.contains(&delegated)),
            "the business issuer must not be hidden inside a proxy action source"
        );
        let expires: chrono::DateTime<chrono::Utc> = if let Some(credential) = credential {
            preparation.query_one("UPDATE credentials SET expires_at=clock_timestamp()+interval '3 seconds' WHERE id=$1 RETURNING expires_at",&[&credential]).await.unwrap().get(0)
        } else {
            preparation.query_one("UPDATE accounts SET expires_at=clock_timestamp()+interval '3 seconds' WHERE id=$1 RETURNING expires_at",&[&issuer]).await.unwrap().get(0)
        };
        preparation.commit().await.unwrap();
        let tx = c.transaction().await.unwrap();
        crate::db::auth_lock(&tx, false).await.unwrap();
        let ctx = f
            .app
            .auth
            .authenticate(&tx, &f.token, "127.0.0.1".parse().unwrap(), Uuid::new_v4())
            .await
            .unwrap();
        let mut cache = host_calls::TestProofCache::default();
        let first = cache
            .revalidate(&tx, &ctx, Some(&business), &original_business)
            .await
            .unwrap();
        assert!(auth::rules_allow(
            &crate::policy::rules(&first),
            "capability.invoke",
            &business,
            ctx.peer
        ));
        let before_proxy = cache
            .revalidate(&tx, &ctx, Some(&proxy), &proxy_paths)
            .await
            .unwrap();
        assert!(auth::rules_allow(
            &crate::policy::rules(&before_proxy),
            "skill.read",
            &proxy,
            ctx.peer
        ));
        // The same transaction, same authenticated caller and same private
        // cache remain alive. Only clocks advance after the authorization fence.
        tokio::time::sleep(
            (expires - chrono::Utc::now()).to_std().unwrap_or_default() + Duration::from_millis(50),
        )
        .await;
        let expired = cache
            .revalidate(&tx, &ctx, Some(&business), &original_business)
            .await
            .unwrap();
        assert!(!auth::rules_allow(&crate::policy::rules(&expired),"capability.invoke",&business,ctx.peer),"an expired external business-only issuer cannot be replaced by a cached independent rule (issuer_credential_present={credential_expiry})");
        let independent = cache
            .revalidate(&tx, &ctx, Some(&business), &independent_business)
            .await
            .unwrap();
        assert!(
            auth::rules_allow(
                &crate::policy::rules(&independent),
                "capability.invoke",
                &business,
                ctx.peer
            ),
            "independent preexisting business authority must remain usable"
        );
        let after_proxy = cache
            .revalidate(&tx, &ctx, Some(&proxy), &proxy_paths)
            .await
            .unwrap();
        assert!(
            auth::rules_allow(
                &crate::policy::rules(&after_proxy),
                "skill.read",
                &proxy,
                ctx.peer
            ),
            "caller and proxy sources remain valid after the unrelated business issuer expires"
        );
        auth::authorize(&tx, &ctx, "skill.read", &proxy)
            .await
            .unwrap();
        tx.rollback().await.unwrap();
    }
}
