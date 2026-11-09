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
