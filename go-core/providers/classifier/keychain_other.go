//go:build !darwin

package classifier

import "errors"

// noKeychain stands in where there is no macOS Keychain: no key is saved, and
// saving one is refused, so only the environment variable provides it.
type noKeychain struct{}

func systemKeychain(string) secretStore { return noKeychain{} }

var errNoKeychain = errors.New("saving a key needs the macOS Keychain; set the environment variable instead")

func (noKeychain) Has(string) (bool, error)      { return false, nil }
func (noKeychain) Get(string) (string, error)    { return "", nil }
func (noKeychain) Set(string, string) error      { return errNoKeychain }
func (noKeychain) Delete(string) error           { return nil }
func (noKeychain) Legacy(string) (string, error) { return "", nil }
func (noKeychain) DropLegacy(string) error       { return nil }
