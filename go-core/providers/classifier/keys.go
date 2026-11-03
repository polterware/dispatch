// Package classifier implements the difficulty classifiers Auto routes on:
// Jev (TypeSafe), OpenAI Decisions, and a cheap model through an agent CLI.
package classifier

import (
	"errors"
	"fmt"
	"os"
	"strings"
)

// Key is where a provider's API key comes from: an environment variable, kept
// as a developer's override, else the macOS Keychain item the dispatch binary
// saved from the app or `dispatch config key set`. The value is never written
// to the store or reported.
type Key struct {
	Env     string
	Account string // the Keychain item's account under KeychainService
	// legacy is the account an earlier Dispatch saved the key under through
	// security(1); the first read moves it to Account.
	legacy string
}

var (
	JevKey    = Key{Env: "TYPESAFE_API_KEY", Account: "jev", legacy: "typesafe"}
	OpenAIKey = Key{Env: "OPENAI_API_KEY", Account: "openai", legacy: "openai"}
)

// Status says where the key is found: "env", "keychain", or "missing". It
// never reads the Keychain item's value.
func (k Key) Status() string {
	if strings.TrimSpace(os.Getenv(k.Env)) != "" {
		return "env"
	}
	if k.Account == "" {
		return "missing"
	}
	_ = k.migrate()
	if ok, err := keychain.Has(k.Account); err == nil && ok {
		return "keychain"
	}
	return "missing"
}

func (k Key) read() (value, source string, err error) {
	if v := strings.TrimSpace(os.Getenv(k.Env)); v != "" {
		return v, "env", nil
	}
	if k.Account == "" {
		return "", "missing", nil
	}
	if err := k.migrate(); err != nil {
		return "", "missing", err
	}
	v, err := keychain.Get(k.Account)
	if err != nil {
		return "", "missing", fmt.Errorf("could not read the key from the Keychain: %w", err)
	}
	if v = strings.TrimSpace(v); v != "" {
		return v, "keychain", nil
	}
	return "", "missing", nil
}

// migrate moves a key an earlier Dispatch saved through security(1), which
// that tool hands any program without asking, into an item of the dispatch
// binary's own, and removes the old one.
func (k Key) migrate() error {
	if k.legacy == "" {
		return nil
	}
	value, err := keychain.Legacy(k.legacy)
	if err != nil || value == "" {
		return err
	}
	if k.legacy == k.Account {
		// One item per service and account: the old one goes first, and the value is saved again from memory.
		if err := keychain.DropLegacy(k.legacy); err != nil {
			return err
		}
		if err := keychain.Set(k.Account, value); err != nil {
			return fmt.Errorf("could not save the key an earlier Dispatch saved again, so save it once more: %w", err)
		}
		return nil
	}
	// A key saved since wins over the old one.
	if ok, err := keychain.Has(k.Account); err != nil {
		return err
	} else if !ok {
		if err := keychain.Set(k.Account, value); err != nil {
			return err
		}
	}
	return keychain.DropLegacy(k.legacy)
}

// Save stores value as the key in the macOS Keychain, replacing any saved one.
func (k Key) Save(value string) error {
	value = strings.TrimSpace(value)
	if value == "" {
		return errors.New("the key is empty")
	}
	if k.Account == "" {
		return errors.New("this classifier takes no key")
	}
	if k.legacy != "" {
		if err := keychain.DropLegacy(k.legacy); err != nil {
			return err
		}
	}
	if err := keychain.Set(k.Account, value); err != nil {
		return fmt.Errorf("could not save the key in the Keychain: %w", err)
	}
	if got, err := keychain.Get(k.Account); err != nil || got != value {
		return errors.New("the Keychain did not keep the key")
	}
	return nil
}

// Clear removes the saved key from the Keychain; a key that is not there is not an error.
func (k Key) Clear() error {
	if k.Account == "" {
		return nil
	}
	if k.legacy != "" {
		if err := keychain.DropLegacy(k.legacy); err != nil {
			return err
		}
	}
	if err := keychain.Delete(k.Account); err != nil {
		return fmt.Errorf("could not remove the key from the Keychain: %w", err)
	}
	return nil
}
