ALTER TABLE facts ADD COLUMN audience_kind TEXT NOT NULL DEFAULT 'global' CHECK(audience_kind IN ('global', 'principal', 'shared'));
ALTER TABLE facts ADD COLUMN audience_ref TEXT CHECK((audience_kind = 'global' AND audience_ref IS NULL) OR (audience_kind != 'global' AND audience_ref IS NOT NULL AND audience_ref != ''));
ALTER TABLE mem_index ADD COLUMN audience_kind TEXT NOT NULL DEFAULT 'global' CHECK(audience_kind IN ('global', 'principal', 'shared'));
ALTER TABLE mem_index ADD COLUMN audience_ref TEXT CHECK((audience_kind = 'global' AND audience_ref IS NULL) OR (audience_kind != 'global' AND audience_ref IS NOT NULL AND audience_ref != ''));
CREATE INDEX idx_facts_audience ON facts(scope, audience_kind, audience_ref) WHERE invalid_at IS NULL;
CREATE INDEX idx_mem_index_audience ON mem_index(scope, audience_kind, audience_ref);
