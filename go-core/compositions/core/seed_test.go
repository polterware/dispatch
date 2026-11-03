package core

import (
	"context"
	"database/sql"
	"os"
	"path/filepath"
	"testing"
)

func TestSeedFromPolterwareCopiesTiersAndPolicyOnce(t *testing.T) {
	core, _ := startCore(t)
	core.DBDir = t.TempDir()
	ctx := context.Background()

	old := t.TempDir()
	oldDB := filepath.Join(old, "local.db")
	db, err := sql.Open("sqlite", oldDB)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := db.Exec(`CREATE TABLE dispatch_tiers (id INTEGER PRIMARY KEY, name TEXT, provider TEXT, agent TEXT, model TEXT, speed INTEGER, strength INTEGER, position INTEGER);
		INSERT INTO dispatch_tiers (name, provider, agent, model, speed, strength, position) VALUES ('easy', 'openai', 'codex', 'gpt-x', 4, 2, 0)`); err != nil {
		t.Fatal(err)
	}
	_ = db.Close()
	if err := os.MkdirAll(filepath.Join(old, "dispatch"), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(old, "dispatch", "config.json"), []byte(`{"priority":"speed"}`), 0o600); err != nil {
		t.Fatal(err)
	}

	n, err := core.SeedFromPolterware(ctx, oldDB)
	if err != nil || n != 1 {
		t.Fatalf("seed: %d %v", n, err)
	}
	if body, err := os.ReadFile(filepath.Join(core.dispatchStateDir(), "config.json")); err != nil || string(body) != `{"priority":"speed"}` {
		t.Fatalf("policy not copied: %q %v", body, err)
	}
	tiers, err := core.Store.ListTiers(ctx)
	if err != nil || len(tiers) != 1 || tiers[0].Model != "gpt-x" {
		t.Fatalf("tiers: %+v %v", tiers, err)
	}
	// A reset after the first start is not undone.
	if err := core.Store.ReplaceTiers(ctx, nil); err != nil {
		t.Fatal(err)
	}
	if n, err := core.SeedFromPolterware(ctx, oldDB); err != nil || n != 0 {
		t.Fatalf("second seed: %d %v", n, err)
	}
}
