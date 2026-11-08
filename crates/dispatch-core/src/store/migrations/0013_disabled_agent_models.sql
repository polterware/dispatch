-- (agent, model) pairs turned off in Settings > Models or `dispatch models disable --agent`.
-- The model is the id the agent's CLI lists, without its effort suffix.
CREATE TABLE IF NOT EXISTS dispatch_disabled_agent_models (
    agent TEXT NOT NULL,
    model TEXT NOT NULL,
    PRIMARY KEY (agent, model)
);
