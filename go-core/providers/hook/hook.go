// Package hook manages the Claude Code SessionStart hook that points agents at the dispatch skill, and the enabled flag.
package hook

import (
	"bytes"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

const (
	// HookSuffix follows the binary in the hook command (`dispatch hook`).
	HookSuffix = " hook"

	// legacySuffix is the hook Polterware registered before Dispatch became its
	// own app (`polterware dispatch hook`); it is still ours, so Install
	// replaces it and Uninstall removes it.
	legacySuffix = " dispatch hook"

	// SessionStart is the Claude Code hook event name.
	SessionStart = "SessionStart"

	// DefaultTimeout is the timeout in seconds for the session start hook.
	DefaultTimeout = 10

	// ContextMessage is emitted to Claude Code session context on startup/resume/clear/compact
	// when dispatch is enabled. It is factual and concise (<= 400 chars).
	ContextMessage = "Dispatch is active in this environment. The `dispatch` skill explains how to delegate tasks to other coding agents (Auto picks the model; a tier easy|normal|hard pins the table); read it before delegating. Keep doing trivial tasks directly.\n"
)

// EnabledPath returns the path to the enabled flag file.
func EnabledPath(stateDir string) string { return filepath.Join(stateDir, "enabled") }

// IsEnabled reports whether the dispatch enabled flag exists.
func IsEnabled(stateDir string) bool {
	_, err := os.Stat(EnabledPath(stateDir))
	return err == nil
}

// SetEnabled creates or removes the enabled flag file.
func SetEnabled(stateDir string, enabled bool) error {
	p := EnabledPath(stateDir)
	if enabled {
		if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
			return err
		}
		return os.WriteFile(p, nil, 0o644)
	}
	err := os.Remove(p)
	if err != nil && !os.IsNotExist(err) {
		return err
	}
	return nil
}

// ContextBlock returns the text injected into the session start hook.
func ContextBlock() string {
	return ContextMessage
}

// SettingsPath returns the Claude settings path: $CLAUDE_CONFIG_DIR/settings.json
// if set, else <userHome>/.claude/settings.json.
func SettingsPath(userHome string) string {
	if ccd := os.Getenv("CLAUDE_CONFIG_DIR"); ccd != "" {
		return filepath.Join(ccd, "settings.json")
	}
	if userHome == "" {
		if u, err := os.UserHomeDir(); err == nil {
			userHome = u
		}
	}
	return filepath.Join(userHome, ".claude", "settings.json")
}

// HookCommand returns the full command string for binaryPath.
func HookCommand(binaryPath string) string {
	binaryPath = strings.TrimSpace(binaryPath)
	if strings.HasSuffix(binaryPath, HookSuffix) {
		return binaryPath
	}
	return shellQuote(binaryPath) + HookSuffix
}

// shellQuote single-quotes p only when it has characters a shell would split
// or expand, so ordinary paths stay readable and old hook entries still match.
func shellQuote(p string) string {
	if p != "" && !strings.ContainsAny(p, " \t\n\"'`$&;|<>()*?[]{}!\\#~") {
		return p
	}
	return "'" + strings.ReplaceAll(p, "'", `'\''`) + "'"
}

// IsHookCommand reports whether cmd is recognized as our session hook command:
// a dispatch binary followed by HookSuffix, or the legacy Polterware one.
func IsHookCommand(cmd string) bool {
	cmd = strings.Trim(strings.TrimSpace(cmd), "'")
	if strings.HasSuffix(cmd, legacySuffix) {
		return true
	}
	bin, ok := strings.CutSuffix(cmd, HookSuffix)
	if !ok {
		return false
	}
	bin = strings.TrimRight(bin, "'")
	return filepath.Base(bin) == "dispatch"
}

// IsInstalled reports whether the SessionStart hook is registered in settingsPath.
func IsInstalled(settingsPath string) (bool, error) {
	root, err := readSettings(settingsPath)
	if err != nil {
		return false, err
	}
	hooks, ok := root["hooks"].(map[string]any)
	if !ok {
		return false, nil
	}
	sessionStart, ok := hooks[SessionStart].([]any)
	if !ok {
		return false, nil
	}
	for _, g := range sessionStart {
		group, ok := g.(map[string]any)
		if !ok {
			continue
		}
		innerHooks, ok := group["hooks"].([]any)
		if !ok {
			continue
		}
		for _, h := range innerHooks {
			action, ok := h.(map[string]any)
			if !ok {
				continue
			}
			cmd, _ := action["command"].(string)
			if IsHookCommand(cmd) {
				return true, nil
			}
		}
	}
	return false, nil
}

// Install adds or updates our SessionStart hook in settingsPath atomically and idempotently.
// It preserves unknown keys and other hooks.
func Install(settingsPath, binaryPath string) error {
	root, err := readSettings(settingsPath)
	if err != nil {
		return err
	}

	targetCmd := HookCommand(binaryPath)

	hooksVal, ok := root["hooks"]
	var hooks map[string]any
	if !ok {
		hooks = make(map[string]any)
		root["hooks"] = hooks
	} else {
		hooks, ok = hooksVal.(map[string]any)
		if !ok {
			return fmt.Errorf("malformed settings %s: 'hooks' is not an object", settingsPath)
		}
	}

	sessionStartVal, ok := hooks[SessionStart]
	var sessionStart []any
	if ok {
		sessionStart, ok = sessionStartVal.([]any)
		if !ok {
			return fmt.Errorf("malformed settings %s: 'hooks.%s' is not an array", settingsPath, SessionStart)
		}
	}

	// Check if already installed.
	for _, g := range sessionStart {
		group, ok := g.(map[string]any)
		if !ok {
			continue
		}
		innerHooks, ok := group["hooks"].([]any)
		if !ok {
			continue
		}
		for _, h := range innerHooks {
			action, ok := h.(map[string]any)
			if !ok {
				continue
			}
			cmd, _ := action["command"].(string)
			if IsHookCommand(cmd) {
				timeoutNum, _ := action["timeout"].(json.Number)
				timeoutInt, _ := action["timeout"].(int)
				isSameTimeout := timeoutInt == DefaultTimeout || timeoutNum.String() == fmt.Sprintf("%d", DefaultTimeout)
				if cmd == targetCmd && isSameTimeout {
					return nil // Already installed and matches exactly
				}
				// Update command and timeout in-place.
				action["command"] = targetCmd
				action["timeout"] = DefaultTimeout
				return writeSettingsAtomic(settingsPath, root)
			}
		}
	}

	// Not found: add a new hook group to SessionStart.
	newAction := map[string]any{
		"type":    "command",
		"command": targetCmd,
		"timeout": DefaultTimeout,
	}
	newGroup := map[string]any{
		"hooks": []any{newAction},
	}
	sessionStart = append(sessionStart, newGroup)
	hooks[SessionStart] = sessionStart

	return writeSettingsAtomic(settingsPath, root)
}

// Uninstall removes our SessionStart hook from settingsPath atomically.
// It preserves all other hooks, other hook events, and unknown keys.
func Uninstall(settingsPath string) error {
	root, err := readSettings(settingsPath)
	if err != nil {
		return err
	}
	if len(root) == 0 {
		return nil
	}

	hooksVal, ok := root["hooks"]
	if !ok {
		return nil
	}
	hooks, ok := hooksVal.(map[string]any)
	if !ok {
		return nil
	}

	sessionStartVal, ok := hooks[SessionStart]
	if !ok {
		return nil
	}
	sessionStart, ok := sessionStartVal.([]any)
	if !ok {
		return nil
	}

	modified := false
	var newSessionStart []any
	for _, g := range sessionStart {
		group, ok := g.(map[string]any)
		if !ok {
			newSessionStart = append(newSessionStart, g)
			continue
		}
		innerHooks, ok := group["hooks"].([]any)
		if !ok {
			newSessionStart = append(newSessionStart, g)
			continue
		}
		var newInner []any
		for _, h := range innerHooks {
			action, ok := h.(map[string]any)
			if !ok {
				newInner = append(newInner, h)
				continue
			}
			cmd, _ := action["command"].(string)
			if IsHookCommand(cmd) {
				modified = true
				continue
			}
			newInner = append(newInner, h)
		}
		if len(newInner) > 0 {
			group["hooks"] = newInner
			newSessionStart = append(newSessionStart, group)
		} else {
			// Dropped our group
			modified = true
		}
	}

	if !modified {
		return nil
	}

	if len(newSessionStart) > 0 {
		hooks[SessionStart] = newSessionStart
	} else {
		delete(hooks, SessionStart)
		if len(hooks) == 0 {
			delete(root, "hooks")
		}
	}

	return writeSettingsAtomic(settingsPath, root)
}

func readSettings(path string) (map[string]any, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		if os.IsNotExist(err) {
			return make(map[string]any), nil
		}
		return nil, err
	}
	if len(bytes.TrimSpace(data)) == 0 {
		return make(map[string]any), nil
	}
	dec := json.NewDecoder(bytes.NewReader(data))
	dec.UseNumber()
	var root map[string]any
	if err := dec.Decode(&root); err != nil {
		return nil, fmt.Errorf("malformed settings %s: %w", path, err)
	}
	if root == nil {
		root = make(map[string]any)
	}
	return root, nil
}

func writeSettingsAtomic(path string, root map[string]any) error {
	dir := filepath.Dir(path)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return err
	}
	b, err := json.MarshalIndent(root, "", "  ")
	if err != nil {
		return err
	}
	b = append(b, '\n')

	tmp, err := os.CreateTemp(dir, "settings-*.json.tmp")
	if err != nil {
		return err
	}
	tmpName := tmp.Name()
	defer func() {
		if tmpName != "" {
			_ = os.Remove(tmpName)
		}
	}()

	if _, err := tmp.Write(b); err != nil {
		_ = tmp.Close()
		return err
	}
	if err := tmp.Sync(); err != nil {
		_ = tmp.Close()
		return err
	}
	if err := tmp.Close(); err != nil {
		return err
	}
	if err := os.Rename(tmpName, path); err != nil {
		return err
	}
	tmpName = ""
	return nil
}

// RegisteredCommand returns the command of our SessionStart hook in
// settingsPath, or "" when it is not registered.
func RegisteredCommand(settingsPath string) string {
	root, err := readSettings(settingsPath)
	if err != nil {
		return ""
	}
	hooks, _ := root["hooks"].(map[string]any)
	groups, _ := hooks[SessionStart].([]any)
	for _, g := range groups {
		group, _ := g.(map[string]any)
		inner, _ := group["hooks"].([]any)
		for _, h := range inner {
			action, _ := h.(map[string]any)
			if cmd, _ := action["command"].(string); IsHookCommand(cmd) {
				return cmd
			}
		}
	}
	return ""
}
