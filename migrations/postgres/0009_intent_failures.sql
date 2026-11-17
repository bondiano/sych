-- Failed reads of an intent's change: each postpones the intent once more, and past the retry
-- limit the intent is consumed, so a provider that keeps failing is not polled forever.
ALTER TABLE change_intents ADD COLUMN failures BIGINT NOT NULL DEFAULT 0;
