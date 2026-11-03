-- What a maintainer decided about a rule suggestion. Suggestions themselves are computed from
-- feedback and the current policy on every read; only the decision is kept, by fingerprint.
CREATE TABLE suggestion_decisions (
    tenant_id TEXT NOT NULL,
    repository_id INTEGER NOT NULL REFERENCES repositories (id),
    fingerprint TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('DISMISSED', 'DONE')),
    actor TEXT NOT NULL,
    pull_request TEXT,
    at INTEGER NOT NULL,
    PRIMARY KEY (tenant_id, repository_id, fingerprint)
);
