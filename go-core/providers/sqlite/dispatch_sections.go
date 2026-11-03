package sqlite

import (
	"context"
	"database/sql"
	"errors"
	"strings"
	"time"

	"github.com/polterware/dispatch/go-core/domains/dispatch"
)

var (
	ErrUnknownSection = errors.New("no such section")
	ErrInvalidSection = errors.New("invalid section or item")
)

// ListSections reads the global sections and their ordered items in one snapshot.
func (s *Store) ListSections(ctx context.Context) ([]dispatch.Section, error) {
	rows, err := s.DB.QueryContext(ctx, `SELECT s.id, s.name, s.position, s.created_at, i.kind, i.ref, i.position
        FROM dispatch_sections s LEFT JOIN dispatch_section_items i ON i.section_id = s.id
        ORDER BY s.position, s.created_at, s.id, i.position, i.kind, i.ref`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	out := []dispatch.Section{}
	for rows.Next() {
		var section dispatch.Section
		var created string
		var kind, ref sql.NullString
		var position sql.NullInt64
		if err := rows.Scan(&section.ID, &section.Name, &section.Position, &created, &kind, &ref, &position); err != nil {
			return nil, err
		}
		if len(out) == 0 || out[len(out)-1].ID != section.ID {
			section.CreatedAt = parseDispatchTime(created)
			section.Items = []dispatch.SectionItem{}
			out = append(out, section)
		}
		if kind.Valid {
			out[len(out)-1].Items = append(out[len(out)-1].Items, dispatch.SectionItem{Kind: kind.String, Ref: ref.String, Position: int(position.Int64)})
		}
	}
	return out, rows.Err()
}

// SaveSection creates when ID is empty, otherwise renames and reorders an existing section.
func (s *Store) SaveSection(ctx context.Context, section dispatch.Section) (dispatch.Section, error) {
	section.Name = strings.TrimSpace(section.Name)
	if section.Name == "" || section.Position < 0 {
		return dispatch.Section{}, ErrInvalidSection
	}
	if section.ID == "" {
		id, err := dispatch.NewID()
		if err != nil {
			return dispatch.Section{}, err
		}
		section.ID, section.CreatedAt = id, time.Now().UTC()
		_, err = s.DB.ExecContext(ctx, `INSERT INTO dispatch_sections(id, name, position, created_at) VALUES (?, ?, ?, ?)`, section.ID, section.Name, section.Position, formatDispatchTime(section.CreatedAt))
		if err != nil {
			return dispatch.Section{}, err
		}
	} else {
		// RETURNING keeps the timestamp and update in the same statement.
		var created string
		err := s.DB.QueryRowContext(ctx, `UPDATE dispatch_sections SET name = ?, position = ? WHERE id = ? RETURNING created_at`, section.Name, section.Position, section.ID).Scan(&created)
		if errors.Is(err, sql.ErrNoRows) {
			return dispatch.Section{}, ErrUnknownSection
		}
		if err != nil {
			return dispatch.Section{}, err
		}
		section.CreatedAt = parseDispatchTime(created)
	}
	sections, err := s.ListSections(ctx)
	if err != nil {
		return dispatch.Section{}, err
	}
	for _, saved := range sections {
		if saved.ID == section.ID {
			return saved, nil
		}
	}
	return dispatch.Section{}, ErrUnknownSection
}

func (s *Store) DeleteSection(ctx context.Context, id string) error {
	if strings.TrimSpace(id) == "" {
		return ErrInvalidSection
	}
	result, err := s.DB.ExecContext(ctx, `DELETE FROM dispatch_sections WHERE id = ?`, id)
	if err != nil {
		return err
	}
	n, err := result.RowsAffected()
	if err == nil && n == 0 {
		return ErrUnknownSection
	}
	return err
}

// MoveToSection appends an item to a section; an empty sectionID removes its assignment.
// The primary key makes an item belong to at most one section across all workspaces.
func (s *Store) MoveToSection(ctx context.Context, kind, ref, sectionID string) error {
	if (kind != "conversation" && kind != "run") || strings.TrimSpace(ref) == "" {
		return ErrInvalidSection
	}
	if sectionID == "" {
		_, err := s.DB.ExecContext(ctx, `DELETE FROM dispatch_section_items WHERE kind = ? AND ref = ?`, kind, ref)
		return err
	}
	// One statement validates the destination and atomically appends or moves it.
	result, err := s.DB.ExecContext(ctx, `INSERT INTO dispatch_section_items(section_id, kind, ref, position)
        SELECT id, ?, ?, COALESCE((SELECT MAX(position) + 1 FROM dispatch_section_items WHERE section_id = ?), 0)
        FROM dispatch_sections WHERE id = ?
        ON CONFLICT(kind, ref) DO UPDATE SET section_id = excluded.section_id, position = excluded.position
        WHERE dispatch_section_items.section_id != excluded.section_id`, kind, ref, sectionID, sectionID)
	if err != nil {
		return err
	}
	n, err := result.RowsAffected()
	if err != nil || n != 0 {
		return err
	}
	var exists int
	if err := s.DB.QueryRowContext(ctx, `SELECT 1 FROM dispatch_sections WHERE id = ?`, sectionID).Scan(&exists); errors.Is(err, sql.ErrNoRows) {
		return ErrUnknownSection
	} else {
		return err
	}
}
