-- Drop schema that no code reads any more.
--
-- core_memory: 0012 kept it for a `goat memory migrate` command that was never
-- written. The import happened anyway -- mem_index carries the rows under
-- core/identity.md, labelled "Imported from v1 core memory" -- so the table is
-- a duplicate of migrated data, not the only copy.
--
-- runtime_flags: only ever held the 'paused' key, written solely by a
-- set_paused that had no callers, so is_paused could never return true. Both
-- are gone along with the scheduler, runtime and watch gates that read them.
--
-- idx_goals_review supported only goals_due_for_review, which had no
-- production caller. goals.parent was always NULL: the goal tool's schema has
-- no parameter for it.
DROP TABLE IF EXISTS core_memory;
DROP TABLE IF EXISTS runtime_flags;
DROP INDEX IF EXISTS idx_goals_review;
ALTER TABLE goals DROP COLUMN parent;
