package setup

import (
	"bytes"
	"embed"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"slices"
)

// openCodeFiles is the Dispatch plugin for OpenCode: its server part swaps
// OpenCode's subagents for Dispatch runs while delegation is on, and its
// OpenCode Desktop part shows the switch and the conversation's runs.
//
//go:embed opencode/package.json opencode/server.js opencode/rpc.js opencode/gui-experimental.cjs
var openCodeFiles embed.FS

// OpenCodeSwitch is the file in dispatch's state folder that holds the
// delegation switch; the plugin reads it on each model call.
const OpenCodeSwitch = "opencode-plugin.json"

// OpenCode is the Dispatch plugin: its folder, the global OpenCode config that
// loads it, Current, Outdated (other files or options), or Missing, and
// whether delegation is on.
type OpenCode struct {
	Path   string `json:"path"`
	Config string `json:"config"`
	State  string `json:"state"`
	On     bool   `json:"on"`
}

// openCodeEntry is the plugin's entry in the global config's plugins list.
type openCodeEntry struct {
	Package string            `json:"package"`
	Options map[string]string `json:"options"`
}

// OpenCodePluginDir is where the plugin is installed: in dispatch's state
// folder, outside the folders OpenCode loads by itself, so the global config
// entry is its only load and OpenCode Desktop loads its GUI.
func OpenCodePluginDir(stateDir string) string { return filepath.Join(stateDir, "opencode-plugin") }

// OpenCodeConfigPath is the global OpenCode config: opencode.jsonc when it
// exists, else opencode.json, under $XDG_CONFIG_HOME or ~/.config.
func OpenCodeConfigPath(home string) string {
	base := filepath.Join(home, ".config")
	if xdg := os.Getenv("XDG_CONFIG_HOME"); filepath.IsAbs(xdg) {
		base = xdg
	}
	dir := filepath.Join(base, "opencode")
	if _, err := os.Stat(filepath.Join(dir, "opencode.jsonc")); err == nil {
		return filepath.Join(dir, "opencode.jsonc")
	}
	return filepath.Join(dir, "opencode.json")
}

func openCodeWant(binary, stateDir string) openCodeEntry {
	return openCodeEntry{
		Package: OpenCodePluginDir(stateDir),
		Options: map[string]string{"binary": binary, "state": filepath.Join(stateDir, OpenCodeSwitch)},
	}
}

func readOpenCode(home, binary, stateDir string) OpenCode {
	o := OpenCode{Path: OpenCodePluginDir(stateDir), Config: OpenCodeConfigPath(home), State: Missing, On: openCodeOn(stateDir)}
	entry, found := findOpenCodeEntry(o.Config, o.Path)
	filesFound, filesCurrent := openCodeFilesState(o.Path)
	if !found && !filesFound {
		return o
	}
	o.State = Outdated
	want := openCodeWant(binary, stateDir)
	if found && filesCurrent && entry.Package == want.Package && maps(entry.Options, want.Options) {
		o.State = Current
	}
	return o
}

func maps(a, b map[string]string) bool {
	if len(a) != len(b) {
		return false
	}
	for k, v := range a {
		if b[k] != v {
			return false
		}
	}
	return true
}

func openCodeFilesState(dir string) (found, current bool) {
	entries, _ := openCodeFiles.ReadDir("opencode")
	current = true
	for _, e := range entries {
		want, _ := openCodeFiles.ReadFile("opencode/" + e.Name())
		got, err := os.ReadFile(filepath.Join(dir, e.Name()))
		if err != nil {
			current = false
			continue
		}
		found = true
		if !bytes.Equal(got, want) {
			current = false
		}
	}
	return found, current
}

func openCodeOn(stateDir string) bool {
	b, err := os.ReadFile(filepath.Join(stateDir, OpenCodeSwitch))
	if err != nil {
		return true
	}
	var body struct {
		Enabled *bool `json:"enabled"`
	}
	if json.Unmarshal(b, &body) != nil || body.Enabled == nil {
		return true
	}
	return *body.Enabled
}

// findOpenCodeEntry finds the plugins entry that loads dir, as a path string
// or as an object's package.
func findOpenCodeEntry(config, dir string) (openCodeEntry, bool) {
	src, err := os.ReadFile(config)
	if err != nil {
		return openCodeEntry{}, false
	}
	doc, err := parseJSONC(src)
	if err != nil {
		return openCodeEntry{}, false
	}
	for _, el := range doc.plugins {
		if entry, ok := decodeOpenCodeEntry(src[el.start:el.end]); ok && entry.Package == dir {
			return entry, true
		}
	}
	return openCodeEntry{}, false
}

func decodeOpenCodeEntry(raw []byte) (openCodeEntry, bool) {
	clean := stripJSONC(raw)
	var path string
	if json.Unmarshal(clean, &path) == nil {
		return openCodeEntry{Package: path}, true
	}
	var entry openCodeEntry
	if json.Unmarshal(clean, &entry) == nil && entry.Package != "" {
		return entry, true
	}
	return openCodeEntry{}, false
}

// InstallOpenCodePlugin writes the plugin over any older copy and loads it
// from the global OpenCode config, which OpenCode reloads by itself. It
// returns the plugin folder.
func InstallOpenCodePlugin(home, binary, stateDir string) (string, error) {
	dir := OpenCodePluginDir(stateDir)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return dir, err
	}
	entries, err := openCodeFiles.ReadDir("opencode")
	if err != nil {
		return dir, err
	}
	for _, e := range entries {
		body, err := openCodeFiles.ReadFile("opencode/" + e.Name())
		if err != nil {
			return dir, err
		}
		if err := writeAtomic(filepath.Join(dir, e.Name()), body); err != nil {
			return dir, err
		}
	}
	entry, err := json.Marshal(openCodeWant(binary, stateDir))
	if err != nil {
		return dir, err
	}
	return dir, editOpenCodeConfig(OpenCodeConfigPath(home), dir, entry)
}

// RemoveOpenCodePlugin drops the plugin's config entry, its folder, and the
// switch.
func RemoveOpenCodePlugin(home, stateDir string) error {
	dir := OpenCodePluginDir(stateDir)
	err := editOpenCodeConfig(OpenCodeConfigPath(home), dir, nil)
	if rmErr := os.RemoveAll(dir); rmErr != nil {
		err = errors.Join(err, rmErr)
	}
	if rmErr := os.Remove(filepath.Join(stateDir, OpenCodeSwitch)); rmErr != nil && !errors.Is(rmErr, os.ErrNotExist) {
		err = errors.Join(err, rmErr)
	}
	return err
}

// SetOpenCodeDelegation switches delegation through Dispatch on or off for
// every OpenCode session.
func SetOpenCodeDelegation(stateDir string, on bool) error {
	body, err := json.MarshalIndent(map[string]bool{"enabled": on}, "", "  ")
	if err != nil {
		return err
	}
	if err := os.MkdirAll(stateDir, 0o755); err != nil {
		return err
	}
	return writeAtomic(filepath.Join(stateDir, OpenCodeSwitch), append(body, '\n'))
}

// editOpenCodeConfig puts entry in the config's plugins list in place of the
// one that loads dir, or adds it; a nil entry removes that one. It edits only
// that list, so the rest of the file, comments included, stays as it was.
func editOpenCodeConfig(config, dir string, entry []byte) error {
	// A linked config, such as one from a dotfiles repository, is edited where it lives.
	if resolved, err := filepath.EvalSymlinks(config); err == nil {
		config = resolved
	}
	src, err := os.ReadFile(config)
	if errors.Is(err, os.ErrNotExist) {
		if entry == nil {
			return nil
		}
		src = []byte("{\n  \"$schema\": \"https://opencode.ai/config.json\"\n}\n")
	} else if err != nil {
		return err
	}
	doc, err := parseJSONC(src)
	if err != nil {
		return errors.New(config + ": " + err.Error())
	}
	at := slices.IndexFunc(doc.plugins, func(el span) bool {
		e, ok := decodeOpenCodeEntry(src[el.start:el.end])
		return ok && e.Package == dir
	})
	var out []byte
	switch {
	case entry == nil && at < 0:
		return nil
	case entry == nil:
		out = removeElement(src, doc, at)
	case at >= 0:
		out = splice(src, doc.plugins[at].start, doc.plugins[at].end, entry)
	case doc.pluginsArray == nil && doc.pluginsKey:
		return errors.New(config + `: "plugins" is not a list`)
	case doc.pluginsArray == nil:
		member := append([]byte("\n  \"plugins\": [\n    "), entry...)
		member = append(member, "\n  ]"...)
		if doc.members > 0 {
			member = append(member, ',')
		} else {
			member = append(member, '\n')
		}
		out = splice(src, doc.open+1, doc.open+1, member)
	case len(doc.plugins) == 0:
		inner := append([]byte("\n    "), entry...)
		inner = append(inner, "\n  "...)
		out = splice(src, doc.pluginsArray.start+1, doc.pluginsArray.end-1, inner)
	default:
		last := doc.plugins[len(doc.plugins)-1]
		out = splice(src, last.end, last.end, append([]byte(",\n    "), entry...))
	}
	if err := os.MkdirAll(filepath.Dir(config), 0o755); err != nil {
		return err
	}
	return writeAtomic(config, out)
}

func splice(src []byte, start, end int, with []byte) []byte {
	out := make([]byte, 0, len(src)-(end-start)+len(with))
	out = append(out, src[:start]...)
	out = append(out, with...)
	return append(out, src[end:]...)
}

// removeElement cuts plugins element i with the comma that joined it to its
// neighbor and the space before it.
func removeElement(src []byte, doc jsoncDoc, i int) []byte {
	els := doc.plugins
	start, end := els[i].start, els[i].end
	switch {
	case i+1 < len(els):
		end = els[i+1].start
	case i > 0:
		start = els[i-1].end
	default:
		start, end = doc.pluginsArray.start+1, doc.pluginsArray.end-1
	}
	return splice(src, start, end, nil)
}

// writeAtomic replaces path through a temporary file of its own beside it, so
// the app and the CLI writing at once never share one, and keeps the mode the
// file had (a user's 0600 config stays 0600).
func writeAtomic(path string, body []byte) error {
	mode := os.FileMode(0o644)
	if info, err := os.Stat(path); err == nil {
		mode = info.Mode().Perm()
	}
	tmp, err := os.CreateTemp(filepath.Dir(path), "."+filepath.Base(path)+".*.tmp")
	if err != nil {
		return err
	}
	_, err = tmp.Write(body)
	err = errors.Join(err, tmp.Chmod(mode), tmp.Close())
	if err == nil {
		err = os.Rename(tmp.Name(), path)
	}
	if err != nil {
		_ = os.Remove(tmp.Name())
	}
	return err
}
