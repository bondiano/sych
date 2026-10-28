-- Bounded follow-up on a tour stop (spec §21.6, roadmap M5.3).
-- A follow-up attempt is charged to its session: the follow-up cap is per tour and actor, not
-- shared by every reader of the run. NULL for analysis and tour attempts.
ALTER TABLE model_attempts ADD COLUMN scope TEXT;

-- One question and its reply in one actor's session; private, never published, and never part
-- of the tour artifact or the review result.
CREATE TABLE tour_exchanges (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    tour_id TEXT NOT NULL,
    actor TEXT NOT NULL,
    run_id TEXT NOT NULL,
    body TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX tour_exchanges_session ON tour_exchanges (tenant_id, tour_id, actor, created_at);
