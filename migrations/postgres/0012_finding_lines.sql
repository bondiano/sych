-- Primary anchor line span for resilient cross-revision dismissal matching.
ALTER TABLE findings ADD COLUMN start_line BIGINT;
ALTER TABLE findings ADD COLUMN end_line BIGINT;
