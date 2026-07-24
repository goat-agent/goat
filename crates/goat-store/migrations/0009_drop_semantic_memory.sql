-- Drop the semantic_memory table. Its MemoryStore trait methods
-- (search_semantic / upsert_semantic) had no callers and have been removed;
-- discrete claims will live in the memory v2 `facts` table instead.
-- 0002_memory.sql stays in place so applied-migration checksums remain valid.

DROP TABLE IF EXISTS semantic_memory;
