package runstate

import (
	"errors"
	"path/filepath"
	"testing"
)

func TestLockCoreIsExclusive(t *testing.T) {
	db := filepath.Join(t.TempDir(), "local.db")
	first, err := LockCore(db)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := LockCore(db); !errors.Is(err, ErrCoreLocked) {
		t.Fatalf("second lock err = %v, want ErrCoreLocked", err)
	}
	_ = first.Close()
	again, err := LockCore(db)
	if err != nil {
		t.Fatalf("lock after release: %v", err)
	}
	_ = again.Close()
}
