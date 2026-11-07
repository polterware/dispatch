-- Agents turned off in Settings > Agents or `dispatch agents disable`.
CREATE TABLE IF NOT EXISTS dispatch_disabled_agents (
    name TEXT NOT NULL PRIMARY KEY
);
