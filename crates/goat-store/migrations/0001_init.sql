CREATE TABLE personas (
  id          TEXT PRIMARY KEY,
  slug        TEXT NOT NULL UNIQUE,
  display     TEXT NOT NULL,
  created_at  TEXT NOT NULL
);

CREATE TABLE conversations (
  id          TEXT PRIMARY KEY,
  persona_id  TEXT NOT NULL REFERENCES personas(id),
  channel     TEXT NOT NULL,
  instance    TEXT NOT NULL,
  external    TEXT NOT NULL,
  created_at  TEXT NOT NULL,
  UNIQUE(persona_id, channel, instance, external)
);
CREATE INDEX idx_conv_persona ON conversations(persona_id);

CREATE TABLE messages (
  id              TEXT PRIMARY KEY,
  conversation_id TEXT NOT NULL REFERENCES conversations(id),
  persona_id      TEXT NOT NULL REFERENCES personas(id),
  direction       TEXT NOT NULL CHECK (direction IN ('in','out')),
  body_kind       TEXT NOT NULL CHECK (body_kind IN ('text','file','reaction')),
  text            TEXT,
  attachment_ref  TEXT,
  reply_to        TEXT,
  ts              TEXT NOT NULL,
  raw             TEXT
);
CREATE INDEX idx_messages_conv_ts    ON messages(conversation_id, ts);
CREATE INDEX idx_messages_persona_ts ON messages(persona_id, ts);
