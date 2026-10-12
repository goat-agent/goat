export const schema = `CREATE TABLE IF NOT EXISTS credentials (
  name TEXT PRIMARY KEY
    CHECK (length(CAST(name AS BLOB)) BETWEEN 1 AND 64 AND length(name) = length(CAST(name AS BLOB))
      AND name GLOB '[a-z0-9]*' AND name NOT GLOB '*[^a-z0-9-]*'),
  provider TEXT NOT NULL
    CHECK (length(CAST(provider AS BLOB)) BETWEEN 1 AND 64 AND length(provider) = length(CAST(provider AS BLOB))
      AND provider NOT GLOB '*[^A-Za-z0-9-]*'),
  method TEXT NOT NULL
    CHECK (length(CAST(method AS BLOB)) BETWEEN 1 AND 64 AND length(method) = length(CAST(method AS BLOB))
      AND method NOT GLOB '*[^A-Za-z0-9-]*'),
  fields TEXT NOT NULL
    CHECK (instr(fields, char(0)) = 0 AND json_valid(fields) AND json_type(fields) = 'object')
) STRICT`;
