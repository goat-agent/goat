-- Drop the evaluator subsystem's model-score table. The evaluator crate and
-- its ModelScoreStore have been removed; scores were written but never read.
-- Existing 0004_evaluator.sql stays in place so sqlx's applied-migration
-- checksums remain valid on databases that already ran it.

DROP TABLE IF EXISTS model_scores;
