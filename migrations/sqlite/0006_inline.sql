-- Inline comments are publication objects too (spec §18.2): one per finding fingerprint, with the
-- exact provider anchor it was planned at.
ALTER TABLE publication_objects ADD COLUMN fingerprint TEXT;
ALTER TABLE publication_objects ADD COLUMN anchor TEXT;
