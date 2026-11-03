package core

import (
	"context"
	"errors"
	"strings"

	"connectrpc.com/connect"
	"github.com/polterware/dispatch/go-core/domains/dispatch"
	dispatchv1 "github.com/polterware/dispatch/go-core/platform/gen/dispatch/v1"
	"github.com/polterware/dispatch/go-core/providers/sqlite"
)

func sectionError(err error) error {
	if errors.Is(err, sqlite.ErrUnknownSection) {
		return connect.NewError(connect.CodeNotFound, err)
	}
	if errors.Is(err, sqlite.ErrInvalidSection) {
		return invalid(err.Error())
	}
	return internal(err)
}

func sectionProto(section dispatch.Section) *dispatchv1.DispatchSection {
	out := &dispatchv1.DispatchSection{Id: section.ID, Name: section.Name, Position: int32(section.Position), CreatedAt: formatDispatchTime(section.CreatedAt)}
	for _, item := range section.Items {
		out.Items = append(out.Items, &dispatchv1.DispatchSectionItem{Kind: item.Kind, Ref: item.Ref, Position: int32(item.Position)})
	}
	return out
}

func (core *Core) ListSections(ctx context.Context, _ *connect.Request[dispatchv1.ListSectionsRequest]) (*connect.Response[dispatchv1.ListSectionsResponse], error) {
	sections, err := core.Store.ListSections(ctx)
	if err != nil {
		return nil, sectionError(err)
	}
	out := &dispatchv1.ListSectionsResponse{}
	for _, section := range sections {
		out.Sections = append(out.Sections, sectionProto(section))
	}
	return connect.NewResponse(out), nil
}

func (core *Core) SaveSection(ctx context.Context, req *connect.Request[dispatchv1.SaveSectionRequest]) (*connect.Response[dispatchv1.SaveSectionResponse], error) {
	section, err := core.Store.SaveSection(ctx, dispatch.Section{ID: strings.TrimSpace(req.Msg.GetId()), Name: req.Msg.GetName(), Position: int(req.Msg.GetPosition())})
	if err != nil {
		return nil, sectionError(err)
	}
	return connect.NewResponse(&dispatchv1.SaveSectionResponse{Section: sectionProto(section)}), nil
}

func (core *Core) DeleteSection(ctx context.Context, req *connect.Request[dispatchv1.DeleteSectionRequest]) (*connect.Response[dispatchv1.DeleteSectionResponse], error) {
	if err := core.Store.DeleteSection(ctx, strings.TrimSpace(req.Msg.GetId())); err != nil {
		return nil, sectionError(err)
	}
	return connect.NewResponse(&dispatchv1.DeleteSectionResponse{}), nil
}

func (core *Core) MoveToSection(ctx context.Context, req *connect.Request[dispatchv1.MoveToSectionRequest]) (*connect.Response[dispatchv1.MoveToSectionResponse], error) {
	if err := core.Store.MoveToSection(ctx, strings.TrimSpace(req.Msg.GetKind()), strings.TrimSpace(req.Msg.GetRef()), strings.TrimSpace(req.Msg.GetSectionId())); err != nil {
		return nil, sectionError(err)
	}
	return connect.NewResponse(&dispatchv1.MoveToSectionResponse{}), nil
}
