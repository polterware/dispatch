ALTER TABLE dispatch_runs ADD COLUMN conversation_id TEXT NOT NULL DEFAULT '';
ALTER TABLE dispatch_runs ADD COLUMN caller_agent TEXT NOT NULL DEFAULT '';
CREATE INDEX dispatch_runs_conversation_created ON dispatch_runs(conversation_id, created_at);

CREATE TABLE dispatch_sections (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    position INTEGER NOT NULL,
    created_at TEXT NOT NULL
);
CREATE TABLE dispatch_section_items (
    section_id TEXT NOT NULL REFERENCES dispatch_sections(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('conversation', 'run')),
    ref TEXT NOT NULL,
    position INTEGER NOT NULL,
    PRIMARY KEY (kind, ref)
);
