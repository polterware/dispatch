-- The name a person gave a conversation in the sidebar, over the first message it shows by default.
CREATE TABLE dispatch_conversation_names (
    conversation_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
