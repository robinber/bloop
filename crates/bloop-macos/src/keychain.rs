//! Keychain: the API keys of the providers, as generic passwords of the service `bloop`.

use bloop_core::error::SecretError;
use bloop_core::ports::platform::SecretStore;
use secrecy::SecretString;
use secrecy::zeroize::Zeroize;
use security_framework::passwords::{PasswordOptions, generic_password};
use security_framework_sys::base::errSecItemNotFound;

const SERVICE: &str = "bloop";

/// Reads the API keys from the login keychain.
///
/// If Bloop is not in the access list of the key, macOS shows an access dialog.
/// The read blocks the calling thread until the user answers.
#[derive(Debug, Clone, Copy, Default)]
pub struct Keychain;

impl SecretStore for Keychain {
    fn get(&self, account: &str) -> Result<SecretString, SecretError> {
        let bytes = generic_password(PasswordOptions::new_generic_password(SERVICE, account))
            .map_err(|error| {
                if error.code() == errSecItemNotFound {
                    return SecretError::NotFound;
                }
                tracing::warn!(account, status = error.code(), "Keychain refused the read");
                SecretError::Unavailable
            })?;
        String::from_utf8(bytes)
            .map(SecretString::from)
            .map_err(|error| {
                error.into_bytes().zeroize();
                tracing::warn!(account, "the key in Keychain is not UTF-8 text");
                SecretError::Unavailable
            })
    }
}
