-- Why a run's review stopped short, kept after retention prunes its result: the execution outcome
-- (`OUTCOME`, once per run), paths left unreviewed by reason (`GAP`) and UNKNOWN obligations by
-- reason code (`UNKNOWN`). Runs completed before this table have no rows.
CREATE TABLE run_reasons (
    run_id TEXT NOT NULL REFERENCES runs (id),
    kind TEXT NOT NULL CHECK (kind IN ('OUTCOME', 'GAP', 'UNKNOWN')),
    reason TEXT NOT NULL,
    occurrences INTEGER NOT NULL,
    PRIMARY KEY (run_id, kind, reason)
);

-- Coverage as the API spells it; it was written in Rust's debug form.
UPDATE runs SET coverage = CASE coverage
    WHEN 'CompleteForScope' THEN 'COMPLETE_FOR_SCOPE'
    WHEN 'Partial' THEN 'PARTIAL'
    WHEN 'Unavailable' THEN 'UNAVAILABLE'
    ELSE coverage END
WHERE coverage IS NOT NULL;
