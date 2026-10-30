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
