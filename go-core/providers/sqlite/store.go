// Package sqlite is Polterware Dispatch's store: one SQLite database that only
// the core (`dispatch serve`) opens.
package sqlite

import (
	"context"
	"database/sql"
	"embed"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	_ "modernc.org/sqlite"
)

//go:embed migrations/*.sql
var migrations embed.FS

type Store struct{ DB *sql.DB }

// New opens the database at path and applies the migrations it has not run.
func New(path string) (*Store, error) {
	dsn := "file:dispatchmem?mode=memory&cache=shared"
	if path != ":memory:" {
		if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
			return nil, err
		}
		dsn = fmt.Sprintf("file:%s?_pragma=busy_timeout(5000)&_pragma=foreign_keys(1)", path)
	}
	db, err := sql.Open("sqlite", dsn)
	if err != nil {
		return nil, err
	}
	if _, err := db.Exec(`PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA busy_timeout = 5000;`); err != nil {
		_ = db.Close()
		return nil, err
	}
	if err := migrate(db); err != nil {
		_ = db.Close()
		return nil, err
	}
	return &Store{DB: db}, nil
}

func (s *Store) Close() error { return s.DB.Close() }

// migrate runs each embedded migration once, in name order, recording the
// applied ones in PRAGMA user_version (the number before the underscore).
func migrate(db *sql.DB) error {
	var version int
	if err := db.QueryRow(`PRAGMA user_version`).Scan(&version); err != nil {
		return err
	}
	entries, err := migrations.ReadDir("migrations")
	if err != nil {
		return err
	}
	names := make([]string, 0, len(entries))
	for _, e := range entries {
		names = append(names, e.Name())
	}
	sort.Strings(names)
	for _, name := range names {
		var n int
		if _, err := fmt.Sscanf(name, "%d_", &n); err != nil {
			return fmt.Errorf("migration %s: no number", name)
		}
		if n <= version {
			continue
		}
		body, err := migrations.ReadFile("migrations/" + name)
		if err != nil {
			return err
		}
		tx, err := db.Begin()
		if err != nil {
			return err
		}
		if _, err := tx.Exec(string(body)); err != nil {
			_ = tx.Rollback()
			return fmt.Errorf("migration %s: %w", name, err)
		}
		if _, err := tx.Exec(fmt.Sprintf(`PRAGMA user_version = %d`, n)); err != nil {
			_ = tx.Rollback()
			return err
		}
		if err := tx.Commit(); err != nil {
			return err
		}
	}
	return nil
}

// Workspace is where runs belong; see the workspaces table.
type Workspace struct {
	ID                  string
	Name                string
	Path                string
	PolterwareProjectID string
	CreatedAt           time.Time
}

// ErrNoWorkspace is a lookup that found nothing.
var ErrNoWorkspace = errors.New("no such workspace")

func (s *Store) SaveWorkspace(w Workspace) error {
	if strings.TrimSpace(w.ID) == "" {
		return errors.New("workspace id is required")
	}
	if w.CreatedAt.IsZero() {
		w.CreatedAt = time.Now().UTC()
	}
	_, err := s.DB.Exec(`INSERT INTO workspaces (id, name, path, polterware_project_id, created_at) VALUES (?, ?, ?, ?, ?)
		ON CONFLICT(id) DO UPDATE SET name = excluded.name, path = excluded.path, polterware_project_id = excluded.polterware_project_id`,
		w.ID, w.Name, w.Path, w.PolterwareProjectID, formatDispatchTime(w.CreatedAt))
	return err
}

func (s *Store) GetWorkspace(id string) (Workspace, error) {
	return s.scanWorkspace(s.DB.QueryRow(`SELECT id, name, path, polterware_project_id, created_at FROM workspaces WHERE id = ?`, id))
}

// WorkspaceAt returns the standalone workspace whose folder holds dir: the
// longest path that is dir or one of its parents.
func (s *Store) WorkspaceAt(dir string) (Workspace, error) {
	items, err := s.ListWorkspaces()
	if err != nil {
		return Workspace{}, err
	}
	dir = filepath.Clean(dir)
	best := Workspace{}
	for _, w := range items {
		if w.Path == "" {
			continue
		}
		p := filepath.Clean(w.Path)
		if (dir == p || strings.HasPrefix(dir, p+string(filepath.Separator))) && len(p) > len(best.Path) {
			best = w
		}
	}
	if best.ID == "" {
		return Workspace{}, ErrNoWorkspace
	}
	return best, nil
}

func (s *Store) ListWorkspaces() ([]Workspace, error) {
	rows, err := s.DB.Query(`SELECT id, name, path, polterware_project_id, created_at FROM workspaces ORDER BY name COLLATE NOCASE, id`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []Workspace
	for rows.Next() {
		w, err := s.scanWorkspace(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, w)
	}
	return out, rows.Err()
}

func (s *Store) DeleteWorkspace(id string) error {
	_, err := s.DB.Exec(`DELETE FROM workspaces WHERE id = ?`, id)
	return err
}

func (s *Store) scanWorkspace(row interface{ Scan(...any) error }) (Workspace, error) {
	var w Workspace
	var created string
	if err := row.Scan(&w.ID, &w.Name, &w.Path, &w.PolterwareProjectID, &created); err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			return Workspace{}, ErrNoWorkspace
		}
		return Workspace{}, err
	}
	w.CreatedAt = parseDispatchTime(created)
	return w, nil
}

// ImportTiers copies the tier table of another dispatch database (Polterware's,
// from before Dispatch became its own app) when this store has none. The
// other database is opened read-only; a missing file or table imports nothing.
func (s *Store) ImportTiers(ctx context.Context, path string) (int, error) {
	var have int
	if err := s.DB.QueryRowContext(ctx, `SELECT COUNT(*) FROM dispatch_tiers`).Scan(&have); err != nil || have > 0 {
		return 0, err
	}
	if _, err := os.Stat(path); err != nil {
		return 0, nil
	}
	src, err := sql.Open("sqlite", fmt.Sprintf("file:%s?mode=ro&_pragma=busy_timeout(5000)", path))
	if err != nil {
		return 0, err
	}
	defer src.Close()
	var table string
	if err := src.QueryRowContext(ctx, `SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'dispatch_tiers'`).Scan(&table); err != nil {
		return 0, nil
	}
	tiers, err := (&Store{DB: src}).ListTiers(ctx)
	if err != nil || len(tiers) == 0 {
		return 0, err
	}
	return len(tiers), s.ReplaceTiers(ctx, tiers)
}
