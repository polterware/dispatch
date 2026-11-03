use super::keychain::store;
use crate::Error;

/// Where a provider's API key comes from: an environment variable, kept as a
/// developer's override, else the macOS Keychain item the dispatch binary
/// saved from the app or `dispatch config key set`. The value is never written
/// to the store or reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    pub env: &'static str,
    /// The Keychain item's account under [`super::KEYCHAIN_SERVICE`].
    pub account: &'static str,
    /// The account an earlier Dispatch saved the key under through
    /// security(1); the first read moves it to `account`.
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
    /// Says where the key is found: "env", "keychain", or "missing". It never
    /// reads the Keychain item's value.
    pub fn status(&self) -> &'static str {
        if !env_value(self.env).is_empty() {
            return "env";
        }
        if self.account.is_empty() {
            return "missing";
        }
        let _ = self.migrate();
        if store().has(self.account).unwrap_or(false) {
            return "keychain";
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
        let v = store().get(self.account).map_err(|e| {
            Error::Message(format!("could not read the key from the Keychain: {e}"))
        })?;
        Ok(v.trim().to_owned())
    }

    /// Moves a key an earlier Dispatch saved through security(1), which that
    /// tool hands any program without asking, into an item of the dispatch
    /// binary's own, and removes the old one.
    fn migrate(&self) -> Result<(), Error> {
        if self.legacy.is_empty() {
            return Ok(());
        }
        let s = store();
        let value = s.legacy(self.legacy)?;
        if value.is_empty() {
            return Ok(());
        }
        if self.legacy == self.account {
            // One item per service and account: the old one goes first, and the value is saved again from memory.
            s.drop_legacy(self.legacy)?;
            return s.set(self.account, &value).map_err(|e| {
                Error::Message(format!(
                    "could not save the key an earlier Dispatch saved again, so save it once more: {e}"
                ))
            });
        }
        // A key saved since wins over the old one.
        if !s.has(self.account)? {
            s.set(self.account, &value)?;
        }
        s.drop_legacy(self.legacy)
    }

    /// Stores `value` as the key in the macOS Keychain, replacing any saved one.
    pub fn save(&self, value: &str) -> Result<(), Error> {
        let value = value.trim();
        if value.is_empty() {
            return Err(Error::Message("the key is empty".into()));
        }
        if self.account.is_empty() {
            return Err(Error::Message("this classifier takes no key".into()));
        }
        let s = store();
        if !self.legacy.is_empty() {
            s.drop_legacy(self.legacy)?;
        }
        s.set(self.account, value)
            .map_err(|e| Error::Message(format!("could not save the key in the Keychain: {e}")))?;
        match s.get(self.account) {
            Ok(got) if got == value => Ok(()),
            _ => Err(Error::Message("the Keychain did not keep the key".into())),
        }
    }

    /// Removes the saved key from the Keychain; a key that is not there is not an error.
    pub fn clear(&self) -> Result<(), Error> {
        if self.account.is_empty() {
            return Ok(());
        }
        let s = store();
        if !self.legacy.is_empty() {
            s.drop_legacy(self.legacy)?;
        }
        s.delete(self.account)
            .map_err(|e| Error::Message(format!("could not remove the key from the Keychain: {e}")))
    }
}
