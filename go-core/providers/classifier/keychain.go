package classifier

// KeychainService names the Keychain items that hold the classifier keys.
const KeychainService = "Polterware Dispatch"

// keychainKind is the kind of the items the dispatch binary saves itself,
// which tells them from the ones an earlier Dispatch saved through security(1).
const keychainKind = "Dispatch classifier key"

// secretStore keeps the classifier keys: the macOS Keychain, or a fake in tests.
type secretStore interface {
	// Has says whether this binary's item for account exists, without reading its value.
	Has(account string) (bool, error)
	// Get is the item's value, "" when there is none.
	Get(account string) (string, error)
	// Set adds the item, or replaces its value.
	Set(account, value string) error
	// Delete removes the item; one that is not there is not an error.
	Delete(account string) error
	// Legacy is the value of an item an earlier Dispatch saved under account
	// through security(1), which that tool hands any program without asking;
	// "" when there is none.
	Legacy(account string) (string, error)
	// DropLegacy removes that item.
	DropLegacy(account string) error
}

// keychain is where the keys live; tests replace it.
var keychain = systemKeychain(KeychainService)
