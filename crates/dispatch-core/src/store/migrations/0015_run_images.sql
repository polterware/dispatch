-- Image mode travels with the run: it routed only to agents that make images, and its brief
-- named its artifacts folder.
ALTER TABLE dispatch_runs ADD COLUMN images INTEGER NOT NULL DEFAULT 0;
