-- When a job was first claimed: the queue wait behind the latency analytics. Jobs claimed before
-- this column stay NULL and are not counted.
ALTER TABLE jobs ADD COLUMN claimed_at BIGINT;
