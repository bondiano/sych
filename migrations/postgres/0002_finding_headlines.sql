-- A finding's severity and title, so lists show it without reading the result artifact.
-- Rows recorded before this migration keep NULL until a later revision touches them.
ALTER TABLE findings ADD COLUMN severity TEXT;
ALTER TABLE findings ADD COLUMN title TEXT;
