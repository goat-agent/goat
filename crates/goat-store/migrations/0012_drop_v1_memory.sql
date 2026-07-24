-- Drop the v1 memory tables. The v1 MemoryStore (core_memory + episodic_memory
-- with in-process KNN) has been removed; memory v2 uses files + facts + a
-- derived index instead. 0001/0002 stay in place so applied-migration
-- checksums remain valid. `goat memory migrate` exports any core_memory rows
-- to files before this runs on a fresh boot, but since migrations run at boot
-- and the export is a manual CLI step, we KEEP core_memory's data path: drop
-- only after the data has been observed. For safety we drop episodic_memory
-- (never user-facing) and leave core_memory intact for `goat memory migrate`.
DROP TABLE IF EXISTS episodic_memory;
