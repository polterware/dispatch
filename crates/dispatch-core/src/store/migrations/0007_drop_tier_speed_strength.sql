-- Routing reads only the model catalog's benchmarks, the difficulty
-- classifier, and the CLIs' quota: a tier entry has no hand-set speed or
-- strength. Every other column and row stays.
ALTER TABLE dispatch_tiers DROP COLUMN speed;
ALTER TABLE dispatch_tiers DROP COLUMN strength;
