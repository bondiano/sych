//! SQL fragments shared by several statements; macros keep every query a static string.

/// Run states that still may spend or publish.
macro_rules! active {
    () => {
        "('QUEUED', 'WAITING_EVIDENCE', 'RUNNING')"
    };
}

/// Predicate on `runs`: the fence is current, its lease alive and the run still the change's
/// desired generation. Arguments are the literal placeholders for job id, owner, epoch, now.
macro_rules! fenced {
    ($job:literal, $owner:literal, $epoch:literal, $now:literal) => {
        concat!(
            "EXISTS (SELECT 1 FROM jobs j WHERE j.id = ",
            $job,
            " AND j.owner = ",
            $owner,
            " AND j.lease_epoch = ",
            $epoch,
            " AND j.state = 'CLAIMED' AND j.lease_until >= ",
            $now,
            " AND j.run_id = runs.id)
             AND runs.generation = (SELECT c.desired_generation FROM changes c
                 WHERE c.id = runs.change_id)"
        )
    };
}

/// Predicate: repository `$repo` is one the caller may read. `$set` is bound to `,id,id,` or
/// NULL for every repository of the tenant; a bound list keeps the statement static on both
/// backends, where a generated `IN (…)` would not be.
macro_rules! visible {
    ($repo:literal, $set:literal) => {
        concat!(
            "(CAST(",
            $set,
            " AS TEXT) IS NULL OR CAST(",
            $set,
            " AS TEXT) LIKE ('%,' || CAST(",
            $repo,
            " AS TEXT) || ',%'))"
        )
    };
}

/// A run as the API shows it, with the repository its access is checked against; the caller
/// appends the `WHERE` clause.
macro_rules! run_view {
    () => {
        "SELECT u.id, u.change_id, r.connection_id, r.full_name, u.generation, u.state, u.stage,
             u.coverage, u.model_attempts, u.cost_known_micro_usd, u.unknown_cost_attempts,
             u.failure, u.created_at, u.finished_at, u.parent_run_id, u.unknown_obligations
         FROM runs u JOIN changes c ON c.id = u.change_id
         LEFT JOIN repositories r ON r.id = c.repository_id "
    };
}

/// `AND` the repository column `$repo` is set, readable (`$2`) and, if `$3` is bound, that one.
macro_rules! scoped {
    ($repo:literal) => {
        concat!(
            " AND ",
            $repo,
            " IS NOT NULL AND ",
            visible!($repo, "$2"),
            " AND (CAST($3 AS BIGINT) IS NULL OR ",
            $repo,
            " = $3)"
        )
    };
}
