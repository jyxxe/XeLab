-- Event visibility, ordering, retry state and bounded storage are core-owned.
-- Released metadata.sql remains immutable; cursors survive retained-event GC.
ALTER TABLE events ADD COLUMN sequence bigint NOT NULL DEFAULT 0;
WITH ordered AS (SELECT id,row_number() OVER (ORDER BY created_at,id) AS position FROM events) UPDATE events e SET sequence=o.position FROM ordered o WHERE e.id=o.id;
ALTER TABLE events ADD CONSTRAINT events_sequence_unique UNIQUE(sequence);
ALTER TABLE events ADD COLUMN published boolean NOT NULL DEFAULT false;
ALTER TABLE events ADD COLUMN storage_bytes bigint NOT NULL DEFAULT 1024 CHECK(storage_bytes>0);
ALTER TABLE subscriptions ADD COLUMN cursor_sequence bigint NOT NULL DEFAULT 0 CHECK(cursor_sequence>=0);
ALTER TABLE event_deliveries ADD COLUMN retry_at timestamptz;
ALTER TABLE event_deliveries ADD COLUMN outcome text CHECK(outcome IN ('ack','retry','dead_letter'));
ALTER TABLE event_deliveries ADD COLUMN completed_at timestamptz;
CREATE INDEX events_ordered_topic_idx ON events(topic,sequence);
CREATE TABLE subscription_authority (
  subscription_id uuid PRIMARY KEY REFERENCES subscriptions(id),
  credential_id uuid NOT NULL REFERENCES credentials(id),
  peer inet NOT NULL,
  source_paths jsonb NOT NULL CHECK(jsonb_typeof(source_paths)='array' AND octet_length(source_paths::text)<=4194304),
  browser_origin text,
  created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE event_dead_letters (
  delivery_id uuid PRIMARY KEY REFERENCES event_deliveries(id),
  subscription_id uuid NOT NULL REFERENCES subscriptions(id),
  event_id uuid NOT NULL REFERENCES events(id),
  reason text NOT NULL CHECK(reason IN ('consumer_dead_letter','maximum_attempts')),
  created_at timestamptz NOT NULL DEFAULT now(),
  expires_at timestamptz NOT NULL DEFAULT now()+interval '30 days'
);
CREATE INDEX event_dead_letters_expiry_idx ON event_dead_letters(expires_at);
CREATE TABLE event_store_limits (
  singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton),
  next_sequence bigint NOT NULL DEFAULT 1 CHECK(next_sequence>0),
  maximum_bytes bigint NOT NULL DEFAULT 268435456 CHECK(maximum_bytes>0),
  maximum_events bigint NOT NULL DEFAULT 100000 CHECK(maximum_events>0),
  maximum_deliveries bigint NOT NULL DEFAULT 200000 CHECK(maximum_deliveries>0),
  maximum_subscriptions bigint NOT NULL DEFAULT 1000 CHECK(maximum_subscriptions>0)
);

