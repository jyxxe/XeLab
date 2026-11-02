-- Immutable admission-time provenance; revocation must not refund used calls.
CREATE TABLE call_credential_charges (
  call_id uuid NOT NULL REFERENCES calls(id),
  credential_id uuid NOT NULL REFERENCES credentials(id),
  PRIMARY KEY (call_id, credential_id)
);
CREATE INDEX call_credential_charges_credential_idx
  ON call_credential_charges(credential_id, call_id);
