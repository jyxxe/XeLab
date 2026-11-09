//! Offline metadata validation never grants live event-consumer authority.
use super::*;

pub(crate) async fn validate_snapshot(c: &(impl GenericClient + Sync)) -> Result<()> {
    let inconsistent: bool = c.query_one(
        "SELECT EXISTS(SELECT 1 FROM events WHERE sequence<=0) OR EXISTS(SELECT 1 FROM subscriptions s LEFT JOIN subscription_authority a ON a.subscription_id=s.id WHERE a.subscription_id IS NULL) OR EXISTS(SELECT 1 FROM event_deliveries WHERE octet_length(lease_digest)<>32 OR attempt<1 OR attempt>10 OR (state IN ('acked','dead_letter') AND completed_at IS NULL)) OR EXISTS(SELECT 1 FROM event_dead_letters l JOIN event_deliveries d ON d.id=l.delivery_id WHERE d.event_id<>l.event_id OR d.subscription_id<>l.subscription_id OR d.state<>'dead_letter' OR d.outcome<>'dead_letter') OR EXISTS(SELECT 1 FROM subscriptions s JOIN events e ON e.id=s.cursor WHERE e.sequence<>s.cursor_sequence OR e.topic<>s.spec->>'topic') OR EXISTS(SELECT 1 FROM subscriptions s WHERE s.cursor_sequence<0) OR ((EXISTS(SELECT 1 FROM events) OR EXISTS(SELECT 1 FROM subscriptions)) AND NOT EXISTS(SELECT 1 FROM event_store_limits)) OR EXISTS(SELECT 1 FROM event_store_limits WHERE next_sequence<=(SELECT COALESCE(max(sequence),0) FROM events))",
        &[],
    ).await?.get(0);
    if inconsistent {
        return Err(Error::conflict(
            "Event snapshot ordering, lease or source metadata differs",
        ));
    }
    let inconsistent: bool = c.query_one(
        "SELECT EXISTS(SELECT 1 FROM event_deliveries d JOIN subscriptions s ON s.id=d.subscription_id JOIN events e ON e.id=d.event_id WHERE e.topic<>s.spec->>'topic' OR d.attempt>COALESCE((s.spec->>'maximum_attempts')::integer,5)) OR EXISTS(SELECT 1 FROM subscription_authority a JOIN subscriptions s ON s.id=a.subscription_id JOIN credentials c ON c.id=a.credential_id WHERE c.owner_id<>s.owner_id) OR EXISTS(SELECT 1 FROM subscriptions s CROSS JOIN event_store_limits l WHERE s.cursor_sequence>=l.next_sequence) OR EXISTS(SELECT 1 FROM event_deliveries WHERE state='retry' AND completed_at IS NULL AND retry_at IS NULL) OR EXISTS(SELECT 1 FROM event_deliveries WHERE state IN ('leased','retry') AND completed_at IS NULL GROUP BY subscription_id HAVING count(*)>1) OR EXISTS(SELECT 1 FROM event_deliveries WHERE (state='leased' AND completed_at IS NOT NULL) OR (state='acked' AND outcome IS DISTINCT FROM 'ack') OR (state='dead_letter' AND outcome IS DISTINCT FROM 'dead_letter'))",
        &[],
    ).await?.get(0);
    if inconsistent {
        return Err(Error::conflict(
            "Event snapshot subscription ownership or pending delivery differs",
        ));
    }
    let mut after = (String::new(), String::new());
    loop {
        let rows = c.query("SELECT e.*,n.manifest FROM event_contracts e JOIN installations n ON n.id=e.publisher_installation_id WHERE (e.topic,e.version)>($1,$2) ORDER BY e.topic,e.version LIMIT 64", &[&after.0,&after.1]).await?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            let topic: String = row.get("topic");
            let version: String = row.get("version");
            let manifest: Value = row.get("manifest");
            validate_manifest(&manifest)?;
            if !manifest["event_topics"].as_array().is_some_and(|items| {
                items.iter().any(|spec| {
                    spec["topic"] == topic
                        && spec["version"] == version
                        && spec["schema_digest"] == row.get::<_, String>("schema_digest")
                        && spec["payload_schema"] == row.get::<_, Value>("payload_schema")
                })
            }) {
                return Err(Error::conflict(
                    "Event snapshot contract lacks signed installation provenance",
                ));
            }
            after = (topic, version);
        }
    }
    let mut after = 0i64;
    loop {
        let rows = c.query("SELECT e.*,k.payload_schema FROM events e LEFT JOIN event_contracts k ON k.topic=e.topic AND k.version=e.version WHERE e.sequence>$1 ORDER BY e.sequence LIMIT 64", &[&after]).await?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            event(&row)?;
            let schema: Option<Value> = row.get("payload_schema");
            let payload: Value = row.get("payload");
            contracts::validate_business(
                &schema.ok_or_else(|| Error::conflict("Event snapshot contract is missing"))?,
                &payload,
            )?;
            let encoded = serde_json::to_vec(&payload)?;
            if encoded.len() > 1048576
                || row.get::<_, i64>("storage_bytes") != (encoded.len() + 1024) as i64
            {
                return Err(Error::conflict("Event snapshot payload accounting differs"));
            }
            let intent = c.query("SELECT payload,delivered_at FROM outbox WHERE topic='kernel.event.publish' AND aggregate_id=$1", &[&row.get::<_,Uuid>("id")]).await?;
            if intent.len() != 1
                || intent[0].get::<_, Value>("payload")
                    != json!({"event_id":row.get::<_,Uuid>("id")})
                || intent[0]
                    .get::<_, Option<DateTime<Utc>>>("delivered_at")
                    .is_some()
                    != row.get::<_, bool>("published")
            {
                return Err(Error::conflict("Event snapshot outbox intent differs"));
            }
            for dedupe in c
                .query(
                    "SELECT payload_digest FROM event_dedupe WHERE event_id=$1",
                    &[&row.get::<_, Uuid>("id")],
                )
                .await?
            {
                if dedupe.get::<_, String>("payload_digest") != contracts::digest(&encoded) {
                    return Err(Error::conflict("Event snapshot dedupe fingerprint differs"));
                }
            }
            after = row.get("sequence");
        }
    }
    let invalid_dedupe: bool = c.query_one("SELECT EXISTS(SELECT 1 FROM event_dedupe d JOIN events e ON e.id=d.event_id WHERE d.publisher_id<>e.publisher_id OR d.topic<>e.topic OR d.version<>e.version OR d.dedupe_key<>e.dedupe_key)", &[]).await?.get(0);
    if invalid_dedupe {
        return Err(Error::conflict("Event snapshot dedupe identity differs"));
    }
    let mut after = Uuid::nil();
    loop {
        let rows = c.query("SELECT s.*,a.source_paths FROM subscriptions s JOIN subscription_authority a ON a.subscription_id=s.id WHERE s.id>$1 ORDER BY s.id LIMIT 64", &[&after]).await?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            subscription_value(&row)?;
            let paths: Vec<Path> = serde_json::from_value(row.get("source_paths"))
                .map_err(|_| Error::conflict("Event subscription source paths are invalid"))?;
            if paths.is_empty() {
                return Err(Error::conflict("Event subscription source paths are empty"));
            }
            for path in paths {
                validate("PermissionRule", &path.rule)?;
            }
            after = row.get("id");
        }
    }
    Ok(())
}

pub(crate) async fn reset_authority(c: &(impl GenericClient + Sync)) -> Result<()> {
    c.execute(
        "UPDATE subscriptions SET status='disabled',revision=revision+1,updated_at=now()",
        &[],
    )
    .await?;
    // Token digests from an old host are never valid on the fresh target, even
    // if someone later explicitly re-enables a historical subscription.
    c.execute("UPDATE event_deliveries SET lease_digest=decode(repeat('00',32),'hex'),lease_expires_at=least(lease_expires_at,now()),state=CASE WHEN state='leased' THEN 'retry' ELSE state END,retry_at=CASE WHEN state='leased' THEN now() ELSE retry_at END", &[]).await?;
    Ok(())
}

pub(crate) async fn ensure_authority_reset(c: &(impl GenericClient + Sync)) -> Result<()> {
    let unsafe_state: bool = c.query_one("SELECT EXISTS(SELECT 1 FROM subscriptions WHERE status<>'disabled') OR EXISTS(SELECT 1 FROM event_deliveries WHERE state='leased' OR lease_expires_at>now() OR lease_digest<>decode(repeat('00',32),'hex'))", &[]).await?.get(0);
    if unsafe_state {
        return Err(Error::conflict(
            "Historical event subscription or lease authority survived recovery",
        ));
    }
    Ok(())
}
