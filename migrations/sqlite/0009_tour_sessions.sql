-- Personal progress through a stored tour (spec §21.8, roadmap M5.2). A session belongs to one
-- actor and changes nothing shared: not the tour artifact, the recommendation, the
-- obligations or any approval.
CREATE TABLE tour_sessions (
    tenant_id TEXT NOT NULL,
    tour_id TEXT NOT NULL,
    actor TEXT NOT NULL,
    run_id TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('IN_PROGRESS', 'COMPLETED', 'DISMISSED')),
    -- JSON array of step ids whose answer was revealed.
    revealed TEXT NOT NULL DEFAULT '[]',
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (tenant_id, tour_id, actor)
);
