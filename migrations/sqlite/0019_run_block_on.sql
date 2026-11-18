-- Whether the run's trusted policy names `review.block_on` rules, recorded when the policy is read:
-- a run that fails later still fails its check closed. NULL: the policy was never read.
ALTER TABLE runs ADD COLUMN block_on INTEGER;
