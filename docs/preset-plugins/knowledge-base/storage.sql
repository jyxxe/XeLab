-- Knowledge-base data_schema_version=2; SQLite 3.37+; plugin-private volume.
PRAGMA foreign_keys=ON;
PRAGMA journal_mode=WAL;
BEGIN;
CREATE TABLE kb_tables (
  id TEXT PRIMARY KEY NOT NULL CHECK(length(id)=36),
  owner_id TEXT NOT NULL CHECK(length(owner_id)=36), name TEXT NOT NULL,
  schema_revision INTEGER NOT NULL CHECK(schema_revision>0),
  data_revision INTEGER NOT NULL CHECK(data_revision>0),
  indexer_account_id TEXT, created_at TEXT NOT NULL, archived_at TEXT
) STRICT;
CREATE UNIQUE INDEX kb_live_table_names ON kb_tables(owner_id,name) WHERE archived_at IS NULL;
CREATE TABLE kb_fields (
  id TEXT PRIMARY KEY NOT NULL, table_id TEXT NOT NULL REFERENCES kb_tables(id),
  name TEXT NOT NULL, type_json TEXT NOT NULL CHECK(json_valid(type_json)),
  nullable INTEGER NOT NULL CHECK(nullable IN (0,1)),
  has_default INTEGER NOT NULL CHECK(has_default IN (0,1)),
  default_json TEXT CHECK(default_json IS NULL OR json_valid(default_json)),
  ordinal INTEGER NOT NULL, dropped_at TEXT
) STRICT;
CREATE UNIQUE INDEX kb_live_field_names ON kb_fields(table_id,name) WHERE dropped_at IS NULL;
CREATE TABLE kb_rows (
  table_id TEXT NOT NULL REFERENCES kb_tables(id), id TEXT NOT NULL,
  revision INTEGER NOT NULL CHECK(revision>0),
  cells_json TEXT NOT NULL CHECK(json_valid(cells_json)),
  created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
  PRIMARY KEY(table_id,id)
) STRICT;
CREATE TABLE kb_media (
  id TEXT PRIMARY KEY NOT NULL, table_id TEXT NOT NULL REFERENCES kb_tables(id),
  kind TEXT NOT NULL CHECK(kind IN ('image','audio','video')),
  content_type TEXT NOT NULL, size_bytes INTEGER NOT NULL CHECK(size_bytes BETWEEN 1 AND 33554432),
  digest TEXT NOT NULL, private_object_key TEXT NOT NULL UNIQUE,
  reference_count INTEGER NOT NULL DEFAULT 0 CHECK(reference_count>=0), created_at TEXT NOT NULL
) STRICT;
CREATE TABLE kb_row_media (
  table_id TEXT NOT NULL, row_id TEXT NOT NULL, media_id TEXT NOT NULL REFERENCES kb_media(id),
  PRIMARY KEY(table_id,row_id,media_id),
  FOREIGN KEY(table_id,row_id) REFERENCES kb_rows(table_id,id)
) STRICT;
CREATE TABLE kb_vector_indexes (
  field_id TEXT PRIMARY KEY NOT NULL REFERENCES kb_fields(id),
  enabled INTEGER NOT NULL DEFAULT 0 CHECK(enabled IN (0,1)),
  generation INTEGER NOT NULL DEFAULT 0 CHECK(generation>=0),
  index_revision INTEGER NOT NULL DEFAULT 1 CHECK(index_revision BETWEEN 1 AND 9007199254740991),
  dimensions INTEGER NOT NULL DEFAULT 128 CHECK(dimensions IN (128,256,512,768)),
  embedding_skill_id TEXT, skill_definition_digest TEXT, embedding_space_id TEXT,
  embedding_routes_json TEXT CHECK(embedding_routes_json IS NULL OR json_valid(embedding_routes_json)),
  route_digest TEXT,
  provider_model_revision TEXT,
  state TEXT NOT NULL CHECK(state IN ('disabled','pending','building','ready','provider_unavailable','failed'))
) STRICT;
CREATE TABLE kb_vectors (
  table_id TEXT NOT NULL, row_id TEXT NOT NULL, field_id TEXT NOT NULL REFERENCES kb_fields(id),
  generation INTEGER NOT NULL, source_hash TEXT NOT NULL, embedding_space_id TEXT,
  dimensions INTEGER NOT NULL CHECK(dimensions IN (128,256,512,768)),
  value_f32le BLOB, state TEXT NOT NULL CHECK(state IN ('pending','ready','failed','skipped')),
  embedding_skill_id TEXT, skill_definition_digest TEXT, provider_installation_id TEXT,
  error_code TEXT, error_message TEXT, updated_at TEXT NOT NULL,
  PRIMARY KEY(table_id,row_id,field_id,generation),
  FOREIGN KEY(table_id,row_id) REFERENCES kb_rows(table_id,id),
  CHECK(value_f32le IS NULL OR length(value_f32le)=dimensions*4),
  CHECK(state<>'ready' OR (value_f32le IS NOT NULL AND embedding_space_id IS NOT NULL))
) STRICT;
CREATE INDEX kb_vectors_pending ON kb_vectors(field_id,generation,state,row_id);
-- Cache content watermarks change in the same transaction as vector mutations.
-- Historical generation GC may conservatively invalidate a current cache entry.
CREATE TRIGGER kb_vector_insert_revision AFTER INSERT ON kb_vectors
BEGIN
  UPDATE kb_vector_indexes SET index_revision=index_revision+1 WHERE field_id=NEW.field_id;
END;
CREATE TRIGGER kb_vector_update_revision AFTER UPDATE ON kb_vectors
BEGIN
  UPDATE kb_vector_indexes SET index_revision=index_revision+1 WHERE field_id=NEW.field_id;
  UPDATE kb_vector_indexes SET index_revision=index_revision+1
    WHERE field_id=OLD.field_id AND OLD.field_id<>NEW.field_id;
END;
CREATE TRIGGER kb_vector_delete_revision AFTER DELETE ON kb_vectors
BEGIN
  UPDATE kb_vector_indexes SET index_revision=index_revision+1 WHERE field_id=OLD.field_id;
END;
CREATE TRIGGER kb_vector_config_revision
AFTER UPDATE OF generation,enabled,dimensions,embedding_skill_id,skill_definition_digest,embedding_space_id,state,embedding_routes_json,route_digest ON kb_vector_indexes
WHEN OLD.generation IS NOT NEW.generation OR OLD.enabled IS NOT NEW.enabled
  OR OLD.dimensions IS NOT NEW.dimensions OR OLD.embedding_skill_id IS NOT NEW.embedding_skill_id
  OR OLD.skill_definition_digest IS NOT NEW.skill_definition_digest OR OLD.embedding_space_id IS NOT NEW.embedding_space_id
  OR OLD.state IS NOT NEW.state
  OR OLD.embedding_routes_json IS NOT NEW.embedding_routes_json OR OLD.route_digest IS NOT NEW.route_digest
BEGIN
  UPDATE kb_vector_indexes SET index_revision=index_revision+1 WHERE field_id=NEW.field_id;
END;
CREATE TABLE kb_operations (
  subject_id TEXT NOT NULL, capability TEXT NOT NULL, idempotency_key TEXT NOT NULL,
  input_digest TEXT NOT NULL, state TEXT NOT NULL CHECK(state IN ('running','succeeded','failed','unknown')),
  safe_result_json TEXT CHECK(safe_result_json IS NULL OR json_valid(safe_result_json)),
  error_json TEXT CHECK(error_json IS NULL OR json_valid(error_json)),
  expires_at TEXT NOT NULL, updated_at TEXT NOT NULL,
  PRIMARY KEY(subject_id,capability,idempotency_key)
) STRICT;
CREATE TABLE kb_metadata (key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL) STRICT;
-- Query caches are bounded in-memory data; never stored in this durable schema.
INSERT INTO kb_metadata(key,value) VALUES('data_schema_version','2');
COMMIT;
