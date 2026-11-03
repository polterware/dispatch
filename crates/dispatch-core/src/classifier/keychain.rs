//! The macOS Keychain items that hold the classifier keys: generic passwords
//! in the login Keychain through Security.framework. An item the dispatch
//! binary adds has an access list that trusts only that binary: any other
//! program, security(1) included, has to ask the person before it reads the
//! value.
#![allow(unsafe_code)]

use crate::Error;
use core_foundation::{
    base::{CFType, CFTypeRef, TCFType},
    boolean::CFBoolean,
    data::{CFData, CFDataRef},
    dictionary::{CFDictionary, CFDictionaryRef},
    string::{CFString, CFStringRef},
};
use std::ffi::c_void;
use std::process::Command;
use std::sync::{Arc, LazyLock, RwLock};

/// Names the Keychain items that hold the classifier keys.
pub const KEYCHAIN_SERVICE: &str = "Polterware Dispatch";

/// The kind of the items the dispatch binary saves itself, which tells them
/// from the ones an earlier Dispatch saved through security(1).
const KEYCHAIN_KIND: &str = "Dispatch classifier key";

/// Keeps the classifier keys: the macOS Keychain, or a fake in tests.
pub trait SecretStore: Send + Sync {
    /// Whether this binary's item for `account` exists, without reading its value.
    fn has(&self, account: &str) -> Result<bool, Error>;
    /// The item's value, empty when there is none.
    fn get(&self, account: &str) -> Result<String, Error>;
    /// Adds the item, or replaces its value.
    fn set(&self, account: &str, value: &str) -> Result<(), Error>;
    /// Removes the item; one that is not there is not an error.
    fn delete(&self, account: &str) -> Result<(), Error>;
    /// The value of an item an earlier Dispatch saved under `account` through
    /// security(1), which that tool hands any program without asking; empty
    /// when there is none.
    fn legacy(&self, account: &str) -> Result<String, Error>;
    /// Removes that item.
    fn drop_legacy(&self, account: &str) -> Result<(), Error>;
}

static STORE: LazyLock<RwLock<Arc<dyn SecretStore>>> = LazyLock::new(|| {
    RwLock::new(Arc::new(MacKeychain {
        service: KEYCHAIN_SERVICE,
    }))
});

pub(super) fn store() -> Arc<dyn SecretStore> {
    STORE
        .read()
        .map_or_else(|e| e.into_inner().clone(), |s| s.clone())
}

/// Replaces where the keys live; tests use a fake.
pub fn set_secret_store(s: Arc<dyn SecretStore>) {
    match STORE.write() {
        Ok(mut g) => *g = s,
        Err(e) => *e.into_inner() = s,
    }
}

const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;
const ERR_SEC_DUPLICATE_ITEM: i32 = -25299;

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    static kSecClass: CFStringRef;
    static kSecClassGenericPassword: CFStringRef;
    static kSecAttrService: CFStringRef;
    static kSecAttrAccount: CFStringRef;
    static kSecAttrDescription: CFStringRef;
    static kSecAttrAccessible: CFStringRef;
    static kSecAttrAccessibleWhenUnlockedThisDeviceOnly: CFStringRef;
    static kSecValueData: CFStringRef;
    static kSecReturnData: CFStringRef;
    static kSecReturnAttributes: CFStringRef;
    static kSecMatchLimit: CFStringRef;
    static kSecMatchLimitOne: CFStringRef;

    fn SecItemAdd(attributes: CFDictionaryRef, result: *mut CFTypeRef) -> i32;
    fn SecItemCopyMatching(query: CFDictionaryRef, result: *mut CFTypeRef) -> i32;
    fn SecItemUpdate(query: CFDictionaryRef, attributes: CFDictionaryRef) -> i32;
    fn SecItemDelete(query: CFDictionaryRef) -> i32;
    fn SecCopyErrorMessageString(status: i32, reserved: *mut c_void) -> CFStringRef;
}

/// A Security.framework constant as a CFType.
fn constant(r: CFStringRef) -> CFType {
    // SAFETY: the constants are static CFStrings that live for the process.
    unsafe { CFString::wrap_under_get_rule(r) }.as_CFType()
}

fn text(s: &str) -> CFType {
    CFString::new(s).as_CFType()
}

fn dict(pairs: &[(CFType, CFType)]) -> CFDictionary<CFType, CFType> {
    CFDictionary::from_CFType_pairs(pairs)
}

fn fail(status: i32) -> Error {
    // SAFETY: SecCopyErrorMessageString returns a new CFString or null.
    let r = unsafe { SecCopyErrorMessageString(status, std::ptr::null_mut()) };
    let message = if r.is_null() {
        "Keychain error".to_owned()
    } else {
        // SAFETY: r is non-null and owned by us (create rule).
        unsafe { CFString::wrap_under_create_rule(r) }.to_string()
    };
    Error::Message(format!("{message} (OSStatus {status})"))
}

/// Takes ownership of what SecItemCopyMatching returned.
fn owned(result: CFTypeRef) -> Option<CFType> {
    // SAFETY: a non-null result follows the create rule.
    (!result.is_null()).then(|| unsafe { CFType::wrap_under_create_rule(result) })
}

struct MacKeychain {
    service: &'static str,
}

impl MacKeychain {
    /// The query for this binary's item for `account`; `extra` adds pairs.
    fn matching(&self, account: &str, extra: &[(CFType, CFType)]) -> CFDictionary<CFType, CFType> {
        // SAFETY: reading framework constants.
        let mut pairs = unsafe {
            vec![
                (constant(kSecClass), constant(kSecClassGenericPassword)),
                (constant(kSecAttrService), text(self.service)),
                (constant(kSecAttrAccount), text(account)),
                (constant(kSecAttrDescription), text(KEYCHAIN_KIND)),
            ]
        };
        pairs.extend_from_slice(extra);
        dict(&pairs)
    }

    fn copy(&self, query: &CFDictionary<CFType, CFType>) -> Result<Option<CFType>, Error> {
        let mut result: CFTypeRef = std::ptr::null();
        // SAFETY: query is a valid dictionary and result an out pointer.
        let status = unsafe { SecItemCopyMatching(query.as_concrete_TypeRef(), &mut result) };
        let result = owned(result);
        match status {
            0 => Ok(result),
            ERR_SEC_ITEM_NOT_FOUND => Ok(None),
            s => Err(fail(s)),
        }
    }

    /// Whether the item for `account` is one this binary did not save: it has
    /// no kind, or another one. Its attributes read without asking.
    fn has_legacy(&self, account: &str) -> Result<bool, Error> {
        // SAFETY: reading framework constants.
        let query = dict(&unsafe {
            [
                (constant(kSecClass), constant(kSecClassGenericPassword)),
                (constant(kSecAttrService), text(self.service)),
                (constant(kSecAttrAccount), text(account)),
                (
                    constant(kSecReturnAttributes),
                    CFBoolean::true_value().as_CFType(),
                ),
                (constant(kSecMatchLimit), constant(kSecMatchLimitOne)),
            ]
        });
        let Some(found) = self.copy(&query)? else {
            return Ok(false);
        };
        let Some(attrs) = found.downcast::<CFDictionary>() else {
            return Ok(false);
        };
        // SAFETY: reading a framework constant as the key pointer.
        let key = unsafe { kSecAttrDescription } as *const c_void;
        let kind = attrs.find(key).and_then(|v| {
            // SAFETY: dictionary values are CF objects retained by the dictionary.
            unsafe { CFType::wrap_under_get_rule(*v) }.downcast::<CFString>()
        });
        Ok(kind.is_none_or(|k| k != KEYCHAIN_KIND))
    }
}

impl SecretStore for MacKeychain {
    fn has(&self, account: &str) -> Result<bool, Error> {
        // SAFETY: reading framework constants.
        let extra = unsafe {
            [
                (
                    constant(kSecReturnAttributes),
                    CFBoolean::true_value().as_CFType(),
                ),
                (constant(kSecMatchLimit), constant(kSecMatchLimitOne)),
            ]
        };
        Ok(self.copy(&self.matching(account, &extra))?.is_some())
    }

    fn get(&self, account: &str) -> Result<String, Error> {
        // SAFETY: reading framework constants.
        let extra = unsafe {
            [
                (
                    constant(kSecReturnData),
                    CFBoolean::true_value().as_CFType(),
                ),
                (constant(kSecMatchLimit), constant(kSecMatchLimitOne)),
            ]
        };
        let Some(found) = self.copy(&self.matching(account, &extra))? else {
            return Ok(String::new());
        };
        // SAFETY: with kSecReturnData and one match the result is a CFData.
        let data = unsafe { CFData::wrap_under_get_rule(found.as_CFTypeRef() as CFDataRef) };
        Ok(String::from_utf8_lossy(data.bytes()).into_owned())
    }

    fn set(&self, account: &str, value: &str) -> Result<(), Error> {
        let data = CFData::from_buffer(value.as_bytes()).as_CFType();
        // SAFETY: reading framework constants.
        let (accessible, value_key) = unsafe {
            (
                (
                    constant(kSecAttrAccessible),
                    constant(kSecAttrAccessibleWhenUnlockedThisDeviceOnly),
                ),
                constant(kSecValueData),
            )
        };
        let add = self.matching(account, &[accessible, (value_key.clone(), data.clone())]);
        // SAFETY: add is a valid dictionary; no result is asked for.
        let mut status = unsafe { SecItemAdd(add.as_concrete_TypeRef(), std::ptr::null_mut()) };
        if status == ERR_SEC_DUPLICATE_ITEM {
            let update = dict(&[(value_key, data)]);
            // SAFETY: both are valid dictionaries.
            status = unsafe {
                SecItemUpdate(
                    self.matching(account, &[]).as_concrete_TypeRef(),
                    update.as_concrete_TypeRef(),
                )
            };
        }
        if status != 0 {
            return Err(fail(status));
        }
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<(), Error> {
        // SAFETY: the query is a valid dictionary.
        let status = unsafe { SecItemDelete(self.matching(account, &[]).as_concrete_TypeRef()) };
        if status != 0 && status != ERR_SEC_ITEM_NOT_FOUND {
            return Err(fail(status));
        }
        Ok(())
    }

    /// Reads the old item through security(1), the one program its access
    /// list trusts, so moving it asks nothing; it is the only call Dispatch
    /// still makes to that tool, and only while such an item exists.
    fn legacy(&self, account: &str) -> Result<String, Error> {
        if !self.has_legacy(account)? {
            return Ok(String::new());
        }
        let out = Command::new("/usr/bin/security")
            .args([
                "find-generic-password",
                "-s",
                self.service,
                "-a",
                account,
                "-w",
            ])
            .output()?;
        if !out.status.success() {
            return Err(Error::Message(format!(
                "could not read the key an earlier Dispatch saved: {}",
                out.status
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    }

    fn drop_legacy(&self, account: &str) -> Result<(), Error> {
        if !self.has_legacy(account)? {
            return Ok(());
        }
        let status = Command::new("/usr/bin/security")
            .args(["delete-generic-password", "-s", self.service, "-a", account])
            .status()?;
        if !status.success() {
            return Err(Error::Message(format!(
                "could not remove the key an earlier Dispatch saved: {status}"
            )));
        }
        Ok(())
    }
}
