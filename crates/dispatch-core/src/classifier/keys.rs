use super::keychain::store;
use crate::Error;

/// Where a provider's API key comes from: an environment variable, kept as a
/// developer's override, else `keys.json` in Dispatch's state folder, which the
/// app and `dispatch config key set` write. The value is never written to the
/// database or reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    pub env: &'static str,
    /// The key's name in `keys.json`, and its Keychain account under
    /// [`super::KEYCHAIN_SERVICE`] when an earlier Dispatch kept it there.
    pub account: &'static str,
    /// The account an even earlier Dispatch saved the key under through
    /// security(1).
    legacy: &'static str,
}

pub const JEV_KEY: Key = Key {
    env: "TYPESAFE_API_KEY",
    account: "jev",
    legacy: "typesafe",
};

pub const OPENAI_KEY: Key = Key {
    env: "OPENAI_API_KEY",
    account: "openai",
    legacy: "openai",
};

fn env_value(name: &str) -> String {
    std::env::var(name).unwrap_or_default().trim().to_owned()
}

impl Key {
    /// Says where the key is found: "env", "saved", or "missing".
    pub fn status(&self) -> &'static str {
        if !env_value(self.env).is_empty() {
            return "env";
        }
        if self.account.is_empty() {
            return "missing";
        }
        let _ = self.migrate();
        if store().has(self.account).unwrap_or(false) {
            return "saved";
        }
        "missing"
    }

    /// The key's value, empty when there is none.
    pub(crate) fn read(&self) -> Result<String, Error> {
        let v = env_value(self.env);
        if !v.is_empty() {
            return Ok(v);
        }
        if self.account.is_empty() {
            return Ok(String::new());
        }
        self.migrate()?;
        let v = store()
            .get(self.account)
            .map_err(|e| Error::Message(format!("could not read the saved key: {e}")))?;
        Ok(v.trim().to_owned())
    }

    /// Moves a key an earlier Dispatch kept in the Keychain, under `account` or the
    /// older `legacy` account, into the key file, and removes the Keychain item. A key
    /// saved since wins; a Keychain that refuses the read leaves the key to be saved again.
    fn migrate(&self) -> Result<(), Error> {
        let s = store();
        if s.has(self.account)? {
            return Ok(());
        }
        for name in [self.account, self.legacy] {
            if name.is_empty() {
                continue;
            }
            let value = s.legacy(name)?;
            if value.trim().is_empty() {
                continue;
            }
            s.set(self.account, value.trim())?;
            return s.drop_legacy(name);
        }
        Ok(())
    }

    /// Saves `value` as the key, replacing any saved one.
    pub fn save(&self, value: &str) -> Result<(), Error> {
        let value = value.trim();
        if value.is_empty() {
            return Err(Error::Message("the key is empty".into()));
        }
        if self.account.is_empty() {
            return Err(Error::Message("this classifier takes no key".into()));
        }
        let s = store();
        s.set(self.account, value)
            .map_err(|e| Error::Message(format!("could not save the key: {e}")))?;
        match s.get(self.account) {
            Ok(got) if got == value => {}
            _ => return Err(Error::Message("the key file did not keep the key".into())),
        }
        // A Keychain copy from an earlier Dispatch would only come back after a remove.
        for name in [self.account, self.legacy] {
            if !name.is_empty() {
                let _ = s.drop_legacy(name);
            }
        }
        Ok(())
    }

    /// Removes the saved key; a key that is not there is not an error.
    pub fn clear(&self) -> Result<(), Error> {
        if self.account.is_empty() {
            return Ok(());
        }
        let s = store();
        for name in [self.account, self.legacy] {
            if !name.is_empty() {
                let _ = s.drop_legacy(name);
            }
        }
        s.delete(self.account)
            .map_err(|e| Error::Message(format!("could not remove the key: {e}")))
    }
}
