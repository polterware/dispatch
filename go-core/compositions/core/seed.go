package core

import (
	"context"
	"errors"
	"os"
	"path/filepath"

	"github.com/polterware/dispatch/go-core/domains/dispatch"
)

// seededFile marks a state folder whose first start already looked for
// Polterware's dispatch state, so a later reset is not undone.
const seededFile = "seeded"

// SeedFromPolterware runs once per state folder: it copies the tier table and
// the routing policy (config.json) Polterware kept before Dispatch became its
// own app. Runs and history are not imported.
func (core *Core) SeedFromPolterware(ctx context.Context, polterwareDB string) (tiers int, err error) {
	state := core.dispatchStateDir()
	marker := filepath.Join(state, seededFile)
	if _, err := os.Stat(marker); err == nil || polterwareDB == "" {
		return 0, nil
	}
	old := filepath.Join(filepath.Dir(polterwareDB), "dispatch")
	name := filepath.Base(dispatch.PolicyConfigPath(""))
	if err := copyIfMissing(filepath.Join(old, name), filepath.Join(state, name)); err != nil {
		return 0, err
	}
	if tiers, err = core.Store.ImportTiers(ctx, polterwareDB); err != nil {
		return 0, err
	}
	if err := os.MkdirAll(state, 0o700); err != nil {
		return tiers, err
	}
	return tiers, os.WriteFile(marker, nil, 0o600)
}

func copyIfMissing(from, to string) error {
	if _, err := os.Stat(to); err == nil {
		return nil
	}
	body, err := os.ReadFile(from)
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(to), 0o700); err != nil {
		return err
	}
	return os.WriteFile(to, body, 0o600)
}
