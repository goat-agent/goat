-- Drop the signal_log table. It backed an earlier self-tick design
-- (SignalSource / SignalKind / PersonaTick) that never had a runtime
-- consumer; those types have been removed from goat-loop. 0003_signal_log.sql
-- stays in place so applied-migration checksums remain valid.

DROP TABLE IF EXISTS signal_log;
