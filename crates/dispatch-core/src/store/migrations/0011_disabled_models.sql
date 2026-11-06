-- Providers (model = '') and base models disabled in Settings > Models or `dispatch models disable`.
CREATE TABLE IF NOT EXISTS dispatch_disabled_models (
    provider TEXT NOT NULL,
    model TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (provider, model)
);
