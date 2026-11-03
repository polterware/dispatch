//go:build darwin

package classifier

import (
	"errors"
	"fmt"
	"os/exec"
	"strings"
	"sync"
	"unsafe"

	"github.com/ebitengine/purego"
)

// macKeychain keeps the keys as generic passwords in the login Keychain
// through Security.framework, called with purego so the binary still builds
// with CGO_ENABLED=0. An item the dispatch binary adds has an access list that
// trusts only that binary: any other program, security(1) included, has to ask
// the person before it reads the value.
type macKeychain struct{ service string }

func systemKeychain(service string) secretStore { return macKeychain{service: service} }

const (
	errSecItemNotFound   = -25300
	errSecDuplicateItem  = -25299
	cfStringEncodingUTF8 = 0x08000100
)

// security is Security.framework and the CoreFoundation calls its queries take.
type security struct {
	cfRelease                 func(ref uintptr)
	cfDictionaryCreateMutable func(allocator uintptr, capacity int, keyCallBacks, valueCallBacks uintptr) uintptr
	cfDictionarySetValue      func(dict, key, value uintptr)
	cfDictionaryGetValue      func(dict, key uintptr) uintptr
	cfStringCreateWithBytes   func(allocator uintptr, bytes []byte, length int, encoding uint32, external uint8) uintptr
	cfStringGetCString        func(str uintptr, buffer []byte, size int, encoding uint32) uint8
	cfDataCreate              func(allocator uintptr, bytes []byte, length int) uintptr
	cfDataGetLength           func(data uintptr) int
	cfDataGetBytePtr          func(data uintptr) *byte
	cfEqual                   func(a, b uintptr) uint8
	secItemAdd                func(attributes uintptr, result *uintptr) int32
	secItemCopyMatching       func(query uintptr, result *uintptr) int32
	secItemUpdate             func(query, attributes uintptr) int32
	secItemDelete             func(query uintptr) int32
	secCopyErrorMessageString func(status int32, reserved uintptr) uintptr

	keyCallBacks, valueCallBacks, cfTrue uintptr

	class, classGenericPassword, attrService, attrAccount, attrDescription uintptr
	attrAccessible, accessibleWhenUnlockedThisDeviceOnly                   uintptr
	valueData, returnData, returnAttributes, matchLimit, matchLimitOne     uintptr
}

var loaded struct {
	once sync.Once
	sec  *security
	err  error
}

func loadSecurity() (*security, error) {
	loaded.once.Do(func() { loaded.sec, loaded.err = openSecurity() })
	return loaded.sec, loaded.err
}

func openSecurity() (s *security, err error) {
	defer func() {
		// RegisterLibFunc panics on a missing symbol.
		if r := recover(); r != nil {
			s, err = nil, fmt.Errorf("could not load Security.framework: %v", r)
		}
	}()
	cf, err := purego.Dlopen("/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation", purego.RTLD_NOW|purego.RTLD_GLOBAL)
	if err != nil {
		return nil, fmt.Errorf("could not load CoreFoundation: %w", err)
	}
	sf, err := purego.Dlopen("/System/Library/Frameworks/Security.framework/Security", purego.RTLD_NOW|purego.RTLD_GLOBAL)
	if err != nil {
		return nil, fmt.Errorf("could not load Security.framework: %w", err)
	}
	s = &security{}
	purego.RegisterLibFunc(&s.cfRelease, cf, "CFRelease")
	purego.RegisterLibFunc(&s.cfDictionaryCreateMutable, cf, "CFDictionaryCreateMutable")
	purego.RegisterLibFunc(&s.cfDictionarySetValue, cf, "CFDictionarySetValue")
	purego.RegisterLibFunc(&s.cfDictionaryGetValue, cf, "CFDictionaryGetValue")
	purego.RegisterLibFunc(&s.cfStringCreateWithBytes, cf, "CFStringCreateWithBytes")
	purego.RegisterLibFunc(&s.cfStringGetCString, cf, "CFStringGetCString")
	purego.RegisterLibFunc(&s.cfDataCreate, cf, "CFDataCreate")
	purego.RegisterLibFunc(&s.cfDataGetLength, cf, "CFDataGetLength")
	purego.RegisterLibFunc(&s.cfDataGetBytePtr, cf, "CFDataGetBytePtr")
	purego.RegisterLibFunc(&s.cfEqual, cf, "CFEqual")
	purego.RegisterLibFunc(&s.secItemAdd, sf, "SecItemAdd")
	purego.RegisterLibFunc(&s.secItemCopyMatching, sf, "SecItemCopyMatching")
	purego.RegisterLibFunc(&s.secItemUpdate, sf, "SecItemUpdate")
	purego.RegisterLibFunc(&s.secItemDelete, sf, "SecItemDelete")
	purego.RegisterLibFunc(&s.secCopyErrorMessageString, sf, "SecCopyErrorMessageString")

	// The callbacks are structs, passed by address; the constants are CFTypeRef variables, read through theirs.
	for _, c := range []struct {
		lib  uintptr
		name string
		dst  *uintptr
		read bool
	}{
		{cf, "kCFTypeDictionaryKeyCallBacks", &s.keyCallBacks, false},
		{cf, "kCFTypeDictionaryValueCallBacks", &s.valueCallBacks, false},
		{cf, "kCFBooleanTrue", &s.cfTrue, true},
		{sf, "kSecClass", &s.class, true},
		{sf, "kSecClassGenericPassword", &s.classGenericPassword, true},
		{sf, "kSecAttrService", &s.attrService, true},
		{sf, "kSecAttrAccount", &s.attrAccount, true},
		{sf, "kSecAttrDescription", &s.attrDescription, true},
		{sf, "kSecAttrAccessible", &s.attrAccessible, true},
		{sf, "kSecAttrAccessibleWhenUnlockedThisDeviceOnly", &s.accessibleWhenUnlockedThisDeviceOnly, true},
		{sf, "kSecValueData", &s.valueData, true},
		{sf, "kSecReturnData", &s.returnData, true},
		{sf, "kSecReturnAttributes", &s.returnAttributes, true},
		{sf, "kSecMatchLimit", &s.matchLimit, true},
		{sf, "kSecMatchLimitOne", &s.matchLimitOne, true},
	} {
		addr, err := purego.Dlsym(c.lib, c.name)
		if err != nil {
			return nil, fmt.Errorf("could not load %s: %w", c.name, err)
		}
		if c.read {
			addr = **(**uintptr)(unsafe.Pointer(&addr))
		}
		*c.dst = addr
	}
	return s, nil
}

// refs releases the CoreFoundation objects a call creates.
type refs struct {
	sec  *security
	held []uintptr
}

func (r *refs) own(ref uintptr) uintptr {
	if ref != 0 {
		r.held = append(r.held, ref)
	}
	return ref
}

func (r *refs) release() {
	for _, ref := range r.held {
		r.sec.cfRelease(ref)
	}
}

func (r *refs) str(s string) uintptr {
	return r.own(r.sec.cfStringCreateWithBytes(0, []byte(s), len(s), cfStringEncodingUTF8, 0))
}

func (r *refs) data(b []byte) uintptr {
	return r.own(r.sec.cfDataCreate(0, b, len(b)))
}

func (r *refs) dict(pairs ...uintptr) uintptr {
	d := r.own(r.sec.cfDictionaryCreateMutable(0, len(pairs)/2, r.sec.keyCallBacks, r.sec.valueCallBacks))
	for i := 0; i+1 < len(pairs); i += 2 {
		r.sec.cfDictionarySetValue(d, pairs[i], pairs[i+1])
	}
	return d
}

func (s *security) fail(status int32) error {
	message := ""
	if ref := s.secCopyErrorMessageString(status, 0); ref != 0 {
		buf := make([]byte, 512)
		if s.cfStringGetCString(ref, buf, len(buf), cfStringEncodingUTF8) != 0 {
			message, _, _ = strings.Cut(string(buf), "\x00")
		}
		s.cfRelease(ref)
	}
	if message == "" {
		message = "Keychain error"
	}
	return fmt.Errorf("%s (OSStatus %d)", message, status)
}

// open loads Security.framework and starts a call's objects.
func (k macKeychain) open() (*security, *refs, error) {
	if k.service == "" {
		return nil, nil, errors.New("no Keychain service")
	}
	s, err := loadSecurity()
	if err != nil {
		return nil, nil, err
	}
	return s, &refs{sec: s}, nil
}

// match is the query for this binary's item for account; extra adds pairs.
func (k macKeychain) match(s *security, r *refs, account string, extra ...uintptr) uintptr {
	return r.dict(append([]uintptr{
		s.class, s.classGenericPassword,
		s.attrService, r.str(k.service),
		s.attrAccount, r.str(account),
		s.attrDescription, r.str(keychainKind),
	}, extra...)...)
}

func (k macKeychain) Has(account string) (bool, error) {
	s, r, err := k.open()
	if err != nil {
		return false, err
	}
	defer r.release()
	var result uintptr
	status := s.secItemCopyMatching(k.match(s, r, account, s.returnAttributes, s.cfTrue, s.matchLimit, s.matchLimitOne), &result)
	r.own(result)
	switch status {
	case 0:
		return true, nil
	case errSecItemNotFound:
		return false, nil
	}
	return false, s.fail(status)
}

func (k macKeychain) Get(account string) (string, error) {
	s, r, err := k.open()
	if err != nil {
		return "", err
	}
	defer r.release()
	var result uintptr
	status := s.secItemCopyMatching(k.match(s, r, account, s.returnData, s.cfTrue, s.matchLimit, s.matchLimitOne), &result)
	r.own(result)
	switch {
	case status == errSecItemNotFound:
		return "", nil
	case status != 0:
		return "", s.fail(status)
	case result == 0:
		return "", nil
	}
	n := s.cfDataGetLength(result)
	if n <= 0 {
		return "", nil
	}
	return string(unsafe.Slice(s.cfDataGetBytePtr(result), n)), nil
}

func (k macKeychain) Set(account, value string) error {
	s, r, err := k.open()
	if err != nil {
		return err
	}
	defer r.release()
	data := r.data([]byte(value))
	status := s.secItemAdd(k.match(s, r, account,
		s.attrAccessible, s.accessibleWhenUnlockedThisDeviceOnly,
		s.valueData, data,
	), nil)
	if status == errSecDuplicateItem {
		status = s.secItemUpdate(k.match(s, r, account), r.dict(s.valueData, data))
	}
	if status != 0 {
		return s.fail(status)
	}
	return nil
}

func (k macKeychain) Delete(account string) error {
	s, r, err := k.open()
	if err != nil {
		return err
	}
	defer r.release()
	if status := s.secItemDelete(k.match(s, r, account)); status != 0 && status != errSecItemNotFound {
		return s.fail(status)
	}
	return nil
}

// hasLegacy says whether the item for account is one this binary did not
// save: it has no kind, or another one. Its attributes read without asking.
func (k macKeychain) hasLegacy(account string) (bool, error) {
	s, r, err := k.open()
	if err != nil {
		return false, err
	}
	defer r.release()
	query := r.dict(
		s.class, s.classGenericPassword,
		s.attrService, r.str(k.service),
		s.attrAccount, r.str(account),
		s.returnAttributes, s.cfTrue,
		s.matchLimit, s.matchLimitOne,
	)
	var result uintptr
	status := s.secItemCopyMatching(query, &result)
	r.own(result)
	switch {
	case status == errSecItemNotFound:
		return false, nil
	case status != 0:
		return false, s.fail(status)
	case result == 0:
		return false, nil
	}
	kind := s.cfDictionaryGetValue(result, s.attrDescription)
	return kind == 0 || s.cfEqual(kind, r.str(keychainKind)) == 0, nil
}

// Legacy reads the old item through security(1), the one program its access
// list trusts, so moving it asks nothing; it is the only call Dispatch still
// makes to that tool, and only while such an item exists.
func (k macKeychain) Legacy(account string) (string, error) {
	if ok, err := k.hasLegacy(account); err != nil || !ok {
		return "", err
	}
	out, err := exec.Command("/usr/bin/security", "find-generic-password", "-s", k.service, "-a", account, "-w").Output()
	if err != nil {
		return "", fmt.Errorf("could not read the key an earlier Dispatch saved: %w", err)
	}
	return strings.TrimSpace(string(out)), nil
}

func (k macKeychain) DropLegacy(account string) error {
	if ok, err := k.hasLegacy(account); err != nil || !ok {
		return err
	}
	if err := exec.Command("/usr/bin/security", "delete-generic-password", "-s", k.service, "-a", account).Run(); err != nil {
		return fmt.Errorf("could not remove the key an earlier Dispatch saved: %w", err)
	}
	return nil
}
