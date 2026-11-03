-- Weekly digest deliveries: one row per tenant, weekly slot and channel, claimed before the send,
-- so several workers send a digest once. A SENDING row that never finished is an unknown outcome
-- and is not sent again; a FAILED one is retried a few times.
CREATE TABLE digest_deliveries (
    tenant_id TEXT NOT NULL,
    week_start INTEGER NOT NULL,
    channel TEXT NOT NULL CHECK (channel IN ('SLACK', 'EMAIL')),
    state TEXT NOT NULL CHECK (state IN ('SENDING', 'DELIVERED', 'FAILED')),
    attempts INTEGER NOT NULL,
    error TEXT,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (tenant_id, week_start, channel)
);
