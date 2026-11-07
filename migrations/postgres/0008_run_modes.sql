-- A run is a REVIEW of the change's generation or a PLAYGROUND try a maintainer asked for: the
-- latter shares the generation it was asked at, supersedes nothing, publishes nothing and stays
-- out of lineage and analytics. So a generation is unique among reviews only.
ALTER TABLE runs ADD COLUMN mode TEXT NOT NULL DEFAULT 'REVIEW'
    CHECK (mode IN ('REVIEW', 'PLAYGROUND'));
ALTER TABLE runs DROP CONSTRAINT runs_change_id_generation_key;
CREATE UNIQUE INDEX runs_review_generation ON runs (change_id, generation) WHERE mode = 'REVIEW';
