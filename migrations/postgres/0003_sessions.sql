-- Browser sessions of people signed in through GitHub and of API clients that exchanged their
-- token. Only the token's SHA-256 is kept. `access` is a person's repository roles at sign-in
-- (JSON); every change they make is checked with GitHub again.
CREATE TABLE sessions (
    token_sha256 TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    actor TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('USER', 'CLIENT')),
    access TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL
);

-- The repository an audited action concerns: a maintainer reads only the log of their own.
ALTER TABLE audit_events ADD COLUMN repository_id BIGINT REFERENCES repositories (id);

-- Rows written before this column: reconcile decisions, whose target names a publication
-- operation that knows its repository. Those whose operation retention already removed stay NULL.
UPDATE audit_events SET repository_id = (
    SELECT c.repository_id FROM publication_operations o
    JOIN runs u ON u.id = o.run_id JOIN changes c ON c.id = u.change_id
    WHERE audit_events.target = 'publication_operation:' || CAST(o.id AS TEXT)
) WHERE target LIKE 'publication_operation:%';
