//! Where the classifier keys live: `keys.json` in Dispatch's state folder, readable by the
//! person's user only (0600 in a 0700 folder), as other agent CLIs keep theirs.
//!
//! They used to be macOS Keychain items. An item the dispatch binary adds trusts only that
//! binary, and every build is another binary: the next build, the release app, or the CLI
//! could no longer read it (errSecAuthFailed, with no one to ask from the headless core). The
//! Keychain code stays to move a key saved there into the file once, when it still reads.
#![allow(unsafe_code)]

use crate::Error;
use core_foundation::{
    base::{CFType, CFTypeRef, TCFType},
    boolean::CFBoolean,
    data::{CFData, CFDataRef},
    dictionary::{CFDictionary, CFDictionaryRef},
    string::{CFString, CFStringRef},
};
use std::collections::BTreeMap;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
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

static STORE: LazyLock<RwLock<Arc<dyn SecretStore>>> =
    LazyLock::new(|| RwLock::new(Arc::new(FileSecrets::default())));

/// The folder `keys.json` lives in, set when the core starts.
static KEYS_DIR: LazyLock<RwLock<Option<PathBuf>>> = LazyLock::new(|| RwLock::new(None));

/// Points the key file at `state_dir`.
pub fn set_keys_dir(state_dir: &Path) {
    match KEYS_DIR.write() {
        Ok(mut g) => *g = Some(state_dir.to_path_buf()),
        Err(e) => *e.into_inner() = Some(state_dir.to_path_buf()),
    }
}

/// The file's name in the state folder.
pub const KEYS_FILE: &str = "keys.json";

/// The keys in `keys.json`; an account the file lacks may still be moved from the Keychain.
#[derive(Default)]
struct FileSecrets {
    /// The Keychain items each account was already looked for, once per process: a read
    /// the Keychain refuses is not asked again.
    tried: std::sync::Mutex<std::collections::HashSet<String>>,
}

impl FileSecrets {
    fn path(&self) -> Result<PathBuf, Error> {
        let dir = KEYS_DIR
            .read()
            .map_or_else(|e| e.into_inner().clone(), |g| g.clone());
        dir.map(|d| d.join(KEYS_FILE))
            .ok_or_else(|| Error::Message("the key file's folder is not set".into()))
    }

    fn load(&self) -> Result<BTreeMap<String, String>, Error> {
        let path = self.path()?;
        match std::fs::read_to_string(&path) {
            Ok(text) if text.trim().is_empty() => Ok(BTreeMap::new()),
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| Error::Message(format!("{}: {e}", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(e) => Err(e.into()),
        }
    }

    /// Writes the keys to a private temporary file and renames it over `keys.json`, so a
    /// reader never sees half a file and the file is never readable by others.
    fn save(&self, keys: &BTreeMap<String, String>) -> Result<(), Error> {
        use std::io::Write as _;
        use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
        let path = self.path()?;
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        let tmp = dir.join(format!(".{KEYS_FILE}.{}", std::process::id()));
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        f.write_all(serde_json::to_string_pretty(keys)?.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    fn keychain() -> MacKeychain {
        MacKeychain {
            service: KEYCHAIN_SERVICE,
        }
    }
}

impl SecretStore for FileSecrets {
    fn has(&self, account: &str) -> Result<bool, Error> {
        Ok(self
            .load()?
            .get(account)
            .is_some_and(|v| !v.trim().is_empty()))
    }

    fn get(&self, account: &str) -> Result<String, Error> {
        Ok(self.load()?.get(account).cloned().unwrap_or_default())
    }

    fn set(&self, account: &str, value: &str) -> Result<(), Error> {
        let mut keys = self.load()?;
        keys.insert(account.to_owned(), value.to_owned());
        self.save(&keys)
    }

    fn delete(&self, account: &str) -> Result<(), Error> {
        let mut keys = self.load()?;
        if keys.remove(account).is_some() {
            self.save(&keys)?;
        }
        Ok(())
    }

    /// A key a Keychain item still holds under `account`: the dispatch binary's own item,
    /// when this binary may read it, else one saved through security(1). Each is looked
    /// for once per process; a refused read is not an error.
    fn legacy(&self, account: &str) -> Result<String, Error> {
        let first = self
            .tried
            .lock()
            .map_or(true, |mut t| t.insert(account.to_owned()));
        if !first {
            return Ok(String::new());
        }
        let k = Self::keychain();
        if let Ok(v) = k.get(account)
            && !v.trim().is_empty()
        {
            return Ok(v);
        }
        Ok(k.legacy(account).unwrap_or_default())
    }

    /// Removes the Keychain items under `account`, as far as the Keychain allows.
    fn drop_legacy(&self, account: &str) -> Result<(), Error> {
        let k = Self::keychain();
        let _ = k.delete(account);
        let _ = k.drop_legacy(account);
        Ok(())
    }
}

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

#[cfg(test)]
mod tests {
    use super::{FileSecrets, KEYS_FILE, SecretStore, set_keys_dir};
    use std::os::unix::fs::PermissionsExt as _;

    #[test]
    fn the_key_file_keeps_keys_for_the_user_only() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("dispatch");
        set_keys_dir(&state);
        let s = FileSecrets::default();
        assert!(!s.has("jev").unwrap());
        s.set("jev", "sk-one").unwrap();
        s.set("openai", "sk-two").unwrap();
        assert_eq!(s.get("jev").unwrap(), "sk-one");
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&state.join(KEYS_FILE)), 0o600);
        assert_eq!(mode(&state), 0o700);
        s.delete("jev").unwrap();
        assert!(!s.has("jev").unwrap());
        assert_eq!(s.get("openai").unwrap(), "sk-two");
        // Nothing else is left in the folder: the temporary file was renamed over the key file.
        let names: Vec<_> = std::fs::read_dir(&state)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from(KEYS_FILE)]);
    }
}
