-- The app now draws the greeting in the device's language; assistant rows from runs always record a model.
DELETE FROM "messages" WHERE "model" IS NULL AND "message"->>'role' = 'assistant';
