// Package runstate owns the small files that let local Polterware Dispatch processes
// find the core: core.json (endpoint, token, build), core.lock (one
// core per database), and core.log. Files live in <db dir>/run and are private
// to the user.
package runstate

import (
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"syscall"
	"time"
)

const coreFile = "core.json"

// Core describes the running core (`dispatch serve`).
type Core struct {
	PID      int    `json:"pid"`
	Endpoint string `json:"endpoint"`
	Token    string `json:"token"`
	DBPath   string `json:"dbPath"`
	// Build identifies the core binary (see coreclient.BuildID).
	Build     string    `json:"build"`
	StartedAt time.Time `json:"startedAt"`
}

// Dir returns the run-state directory for a database path.
func Dir(dbPath string) string {
	return filepath.Join(filepath.Dir(dbPath), "run")
}

func CorePath(dbPath string) string { return filepath.Join(Dir(dbPath), coreFile) }

func LogPath(dbPath string) string { return filepath.Join(Dir(dbPath), "core.log") }

func LockPath(dbPath string) string { return filepath.Join(Dir(dbPath), "core.lock") }

// ErrCoreLocked means another core already owns this database.
var ErrCoreLocked = errors.New("another dispatch core owns this database")

// LockCore takes the exclusive per-database core lock (one core per
// database). The lock lasts until the returned file is closed or the process
// exits.
func LockCore(dbPath string) (*os.File, error) {
	if err := EnsureDir(dbPath); err != nil {
		return nil, err
	}
	file, err := os.OpenFile(LockPath(dbPath), os.O_CREATE|os.O_RDWR, 0o600)
	if err != nil {
		return nil, err
	}
	if err := syscall.Flock(int(file.Fd()), syscall.LOCK_EX|syscall.LOCK_NB); err != nil {
		_ = file.Close()
		if errors.Is(err, syscall.EWOULDBLOCK) {
			return nil, ErrCoreLocked
		}
		return nil, err
	}
	return file, nil
}

// EnsureDir creates the run-state directory with user-only permissions.
func EnsureDir(dbPath string) error {
	return os.MkdirAll(Dir(dbPath), 0o700)
}

// WriteJSON writes v atomically with 0600 permissions.
func WriteJSON(path string, v any) error {
	body, err := json.MarshalIndent(v, "", "  ")
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return err
	}
	tmp, err := os.CreateTemp(filepath.Dir(path), ".tmp-*")
	if err != nil {
		return err
	}
	defer os.Remove(tmp.Name())
	if err := tmp.Chmod(0o600); err != nil {
		_ = tmp.Close()
		return err
	}
	if _, err := tmp.Write(append(body, '\n')); err != nil {
		_ = tmp.Close()
		return err
	}
	if err := tmp.Close(); err != nil {
		return err
	}
	return os.Rename(tmp.Name(), path)
}

// PIDAlive reports whether a process with pid exists.
func PIDAlive(pid int) bool {
	if pid <= 0 {
		return false
	}
	err := syscall.Kill(pid, 0)
	return err == nil || errors.Is(err, syscall.EPERM)
}

// ReadCore returns the live core for dbPath. A stale file is removed and
// reported as not found.
func ReadCore(dbPath string) (Core, bool) {
	path := CorePath(dbPath)
	body, err := os.ReadFile(path)
	if err != nil {
		return Core{}, false
	}
	var core Core
	if json.Unmarshal(body, &core) != nil || !PIDAlive(core.PID) {
		_ = os.Remove(path)
		return Core{}, false
	}
	return core, true
}

// RemoveCore deletes core.json only when it still belongs to pid.
func RemoveCore(dbPath string, pid int) {
	path := CorePath(dbPath)
	body, err := os.ReadFile(path)
	if err != nil {
		return
	}
	var core Core
	if json.Unmarshal(body, &core) == nil && core.PID != pid {
		return
	}
	_ = os.Remove(path)
}
