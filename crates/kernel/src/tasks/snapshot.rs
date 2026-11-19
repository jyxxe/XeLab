//! Cross-table consistency for offline task and batch metadata. Recovery revokes
//! active delegations; importing this history never restores execution authority.
use super::*;

pub(crate) async fn validate_snapshot(c: &(impl GenericClient + Sync)) -> Result<()> {
    let inconsistent: bool = c
        .query_one(
            "SELECT EXISTS (
                 SELECT FROM attempts a JOIN task_steps s ON s.id=a.step_id
                   JOIN tasks t ON t.id=s.task_id
                  WHERE a.state IN ('leased','running')
                    AND (s.state IN ('succeeded','failed','unknown','canceled')
                      OR t.state IN ('succeeded','failed','unknown','canceled'))
             ) OR EXISTS (
                 SELECT FROM skill_batch_items i JOIN skill_batches b ON b.id=i.batch_id
                  WHERE i.state='running' AND b.state IN ('succeeded','partially_succeeded','failed','canceled')
             ) OR EXISTS (
                 SELECT FROM attempts a LEFT JOIN attempt_commit_receipts r ON r.attempt_id=a.id
                  WHERE (r.attempt_id IS NOT NULL AND (a.worker_id IS NULL OR r.state<>a.state))
                     OR (a.worker_id IS NOT NULL AND a.state IN ('succeeded','failed','unknown')
                         AND r.attempt_id IS NULL)
             ) OR EXISTS (
                 SELECT FROM task_step_resolutions resolution
                   JOIN attempts a ON a.id=resolution.attempt_id JOIN task_steps s ON s.id=a.step_id
                  WHERE a.state NOT IN ('expired','unknown')
                     OR (a.number=s.attempt_count AND a.fence=s.fence AND (
                         (resolution.resolution='confirmed_succeeded' AND (s.state<>'succeeded' OR s.output IS NULL))
                         OR (resolution.resolution='confirmed_not_executed'
                             AND (s.state NOT IN ('failed','ready','canceled') OR s.output IS NOT NULL))
                     ))
                     OR NOT EXISTS (
                    SELECT FROM audit_events audit WHERE audit.request_id=resolution.request_id
                      AND audit.subject_id=resolution.subject_id AND audit.credential_id=resolution.credential_id
                      AND audit.action='resolveUnknownStep' AND audit.outcome='succeeded'
                      AND audit.resource=jsonb_build_object('kind','task','match','ids','ids',jsonb_build_array(s.task_id))
                  )
             )",
            &[],
        )
        .await?
        .get(0);
    if inconsistent
        || inconsistent_attempt_calls(c).await?
        || retry::inconsistent_snapshot(c).await?
    {
        return Err(Error::conflict(
            "Snapshot contains inconsistent task or batch execution history",
        ));
    }
    Ok(())
}

// A retained Call may outlive its lease or the deployment's current
// installation. Compare immutable historical identities and the saved target.
async fn inconsistent_attempt_calls(c: &(impl GenericClient + Sync)) -> Result<bool> {
    Ok(c.query_one(
        "SELECT EXISTS (
                 SELECT FROM task_attempt_calls b JOIN calls call ON call.id=b.call_id
                   JOIN attempts a ON a.id=b.attempt_id JOIN task_steps s ON s.id=a.step_id
                   JOIN tasks t ON t.id=s.task_id
                   JOIN execution_delegations original ON original.id=t.delegation_id
                   LEFT JOIN instances i ON i.id=call.provider_instance_id
                   LEFT JOIN execution_delegations derived ON derived.id=call.delegation_id
                   LEFT JOIN credentials issuer ON issuer.id=original.issuer_credential_id
                   LEFT JOIN skills skill ON skill.id=call.skill_id
                  WHERE a.worker_id IS NULL OR a.number NOT BETWEEN 1 AND 5
                     OR a.fence NOT BETWEEN 1 AND 9007199254740991
                     OR t.admission->'version' IS DISTINCT FROM '1'::jsonb
                     OR call.owner_id<>t.owner_id OR original.subject_id<>t.owner_id
                     OR original.run_as<>'user' OR original.service_account_id IS NOT NULL
                     OR issuer.owner_id IS DISTINCT FROM t.owner_id
                     OR call.credential_id IS DISTINCT FROM original.issuer_credential_id
                     OR call.idempotency_key IS DISTINCT FROM a.idempotency_key
                     OR call.deadline_at>t.deadline_at
                     OR i.id IS NULL OR (s.provider_deployment_id IS NOT NULL
                         AND i.deployment_id<>s.provider_deployment_id)
                     OR derived.subject_id IS DISTINCT FROM call.owner_id
                     OR derived.issuer_credential_id IS DISTINCT FROM call.credential_id
                     OR derived.run_as IS DISTINCT FROM 'user' OR derived.service_account_id IS NOT NULL
                     OR derived.expires_at>call.deadline_at
                     OR call.delegation_id IN (SELECT delegation_id FROM tasks)
                     OR call.delegation_id IN (SELECT delegation_id FROM calls
                         WHERE delegation_id IS NOT NULL GROUP BY delegation_id HAVING count(*)>1)
                     OR NOT EXISTS (
                         SELECT FROM jsonb_array_elements(CASE
                           WHEN jsonb_typeof(t.admission->'steps')='array' THEN t.admission->'steps'
                           ELSE '[]'::jsonb END) admitted
                         WHERE admitted->>'step_key'=s.step_key
                           AND admitted->'deployment_id'=coalesce(to_jsonb(s.provider_deployment_id),'null'::jsonb)
                           AND admitted->'installation_id'=CASE WHEN s.provider_deployment_id IS NULL
                               THEN 'null'::jsonb ELSE to_jsonb(b.installation_id) END
                           AND admitted->'binding'->'contract'=call.contract
                           AND ((admitted->'binding'->>'kind'='capability'
                               AND call.skill_id IS NULL AND call.skill_definition_digest IS NULL)
                             OR (admitted->'binding'->>'kind'='skill_capability'
                               AND admitted->'binding'->'skill_id'=to_jsonb(call.skill_id)
                               AND admitted->'binding'->>'definition_digest'=call.skill_definition_digest
                               AND skill.installation_id=b.installation_id))
                     )
             )",
        &[],
    ).await?.try_get(0)?)
}
