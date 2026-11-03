//go:build darwin

package classifier

import (
	"os"
	"os/exec"
	"strings"
	"testing"
)

// TestRealKeychain saves, reads, replaces, moves, and removes items in the
// login Keychain under a test service. It runs only with
// DISPATCH_KEYCHAIN_TEST=1, since it writes to the person's Keychain.
func TestRealKeychain(t *testing.T) {
	if os.Getenv("DISPATCH_KEYCHAIN_TEST") != "1" {
		t.Skip("set DISPATCH_KEYCHAIN_TEST=1 to use the login Keychain")
	}
	const service = "Polterware Dispatch Test"
	store := macKeychain{service: service}
	original := keychain
	keychain = store
	cleanup := func() {
		for _, account := range []string{"jev-test", "old-test"} {
			_ = store.Delete(account)
			_ = exec.Command("/usr/bin/security", "delete-generic-password", "-s", service, "-a", account).Run()
		}
	}
	cleanup()
	t.Cleanup(func() { cleanup(); keychain = original })

	if ok, err := store.Has("jev-test"); ok || err != nil {
		t.Fatalf("has before saving: %v, %v", ok, err)
	}
	if v, err := store.Get("jev-test"); v != "" || err != nil {
		t.Fatalf("get before saving: %q, %v", v, err)
	}
	if err := store.Set("jev-test", "sk-first"); err != nil {
		t.Fatal(err)
	}
	if err := store.Set("jev-test", "sk-second"); err != nil {
		t.Fatalf("replace: %v", err)
	}
	if ok, err := store.Has("jev-test"); !ok || err != nil {
		t.Fatalf("has: %v, %v", ok, err)
	}
	if v, err := store.Get("jev-test"); v != "sk-second" || err != nil {
		t.Fatalf("get: %q, %v", v, err)
	}
	if ok, err := store.hasLegacy("jev-test"); ok || err != nil {
		t.Fatalf("a native item is not legacy: %v, %v", ok, err)
	}
	attrs, err := exec.Command("/usr/bin/security", "find-generic-password", "-s", service, "-a", "jev-test").CombinedOutput()
	if err != nil || !strings.Contains(string(attrs), keychainKind) {
		t.Fatalf("the item's kind: %s, %v", attrs, err)
	}

	// An item security(1) saved moves to one of this binary's own, under the same account.
	if out, err := exec.Command("/usr/bin/security", "add-generic-password", "-s", service, "-a", "old-test", "-w", "sk-old").CombinedOutput(); err != nil {
		t.Fatalf("%s: %v", out, err)
	}
	if ok, _ := store.Has("old-test"); ok {
		t.Fatal("a legacy item must not count as saved")
	}
	k := Key{Env: "DISPATCH_KEYCHAIN_TEST_UNSET", Account: "old-test", legacy: "old-test"}
	if v, src, err := k.read(); v != "sk-old" || src != "keychain" || err != nil {
		t.Fatalf("migrated read: %q from %s, %v", v, src, err)
	}
	if ok, err := store.hasLegacy("old-test"); ok || err != nil {
		t.Fatalf("the old item is left: %v, %v", ok, err)
	}

	if err := store.Delete("jev-test"); err != nil {
		t.Fatal(err)
	}
	if err := store.Delete("jev-test"); err != nil {
		t.Fatalf("deleting a missing item: %v", err)
	}
	if ok, _ := store.Has("jev-test"); ok {
		t.Fatal("the item is still there")
	}
}
