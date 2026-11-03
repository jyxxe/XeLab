-- Keep migrations 1..12 immutable. Format changes need the complete original
-- available-backup registration, not only a foreign key or caller schema label.
ALTER TABLE volume_schema_jobs ADD COLUMN backup_registration jsonb;
ALTER TABLE volume_schema_jobs ADD CONSTRAINT schema_backup_registration_size CHECK(
  backup_registration IS NULL OR
  (jsonb_typeof(backup_registration)='object'
   AND octet_length(backup_registration::text)<=4194304)
);
ALTER TABLE volume_schema_jobs ADD CONSTRAINT schema_backup_registration_binding CHECK(
  (kind='bootstrap' AND backup_registration IS NULL) OR
  (kind<>'bootstrap' AND backup_registration IS NOT NULL AND (
    backup_registration->>'credential'=credential_id::text
    AND backup_registration->>'subject'=subject_id::text
    AND backup_registration->'expected'->>'backup'=backup_id::text
    AND backup_registration->'expected'->>'deployment'=deployment_id::text
    AND backup_registration->'expected'->'installation'->'manifest'->>'data_schema_version'=
      CASE WHEN kind='upgrade' THEN source_schema ELSE target_schema END
    AND backup_registration->'expected'->'installation'->'manifest'->>'plugin_id'=
      installation_metadata->'manifest'->>'plugin_id'
    AND backup_registration->'expected'->'installation'->'manifest'->>'publisher_id'=
      installation_metadata->'manifest'->>'publisher_id'
  ) IS TRUE)
);
