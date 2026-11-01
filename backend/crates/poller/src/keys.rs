//! Credential decryption keys (03 section 6). Built from the raw key parts in `PollerConfig`; `KeyRing` never
//! leaves this crate (core never references poller types).

use famweather_core::config::PollerConfig;
use famweather_core::crypto::{
    decrypt_credential, CredentialPrivateKey, CryptoError, EncryptedCredential,
};
use secrecy::SecretString;
use uuid::Uuid;

/// The columns that identify and protect one stored credential.
#[derive(Clone)]
pub struct CredentialRow {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub ciphertext: Vec<u8>,
    pub encapsulated_key: Vec<u8>,
    pub key_version: i16,
}

impl std::fmt::Debug for CredentialRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialRow")
            .field("id", &self.id)
            .field("key_version", &self.key_version)
            .finish_non_exhaustive()
    }
}

pub struct KeyRing {
    current: (i16, CredentialPrivateKey),
    previous: Option<(i16, CredentialPrivateKey)>,
}

impl std::fmt::Debug for KeyRing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyRing")
            .field("current_version", &self.current.0)
            .field("previous_version", &self.previous.as_ref().map(|p| p.0))
            .finish_non_exhaustive()
    }
}

impl KeyRing {
    /// `CREDENTIALS_PRIVATE_KEY[_PREVIOUS]` with their versions. A missing current key is an error: without it
    /// no credential can be used.
    pub fn from_config(cfg: &PollerConfig) -> Result<KeyRing, CryptoError> {
        let current = cfg.current_key.clone().ok_or(CryptoError::InvalidKey)?;
        Self::new(current, cfg.previous_key.clone())
    }

    pub fn new(
        current: (i16, CredentialPrivateKey),
        previous: Option<(i16, CredentialPrivateKey)>,
    ) -> Result<KeyRing, CryptoError> {
        if previous.as_ref().is_some_and(|p| p.0 == current.0) {
            return Err(CryptoError::InvalidKey);
        }
        Ok(KeyRing { current, previous })
    }

    pub fn current_version(&self) -> i16 {
        self.current.0
    }

    pub fn current_key(&self) -> &CredentialPrivateKey {
        &self.current.1
    }

    fn key_for(&self, version: i16) -> Result<&CredentialPrivateKey, CryptoError> {
        if version == self.current.0 {
            Ok(&self.current.1)
        } else if let Some((v, k)) = &self.previous {
            if *v == version {
                return Ok(k);
            }
            Err(CryptoError::InvalidKey)
        } else {
            Err(CryptoError::InvalidKey)
        }
    }

    /// Picks the key by the row's `key_version`, then decrypts with the row's ids as AAD. An unknown version is a
    /// `CryptoError` (the credential is skipped and the job fails).
    pub fn decrypt_row(&self, row: &CredentialRow) -> Result<SecretString, CryptoError> {
        let sk = self.key_for(row.key_version)?;
        decrypt_credential(
            sk,
            &EncryptedCredential {
                ciphertext: row.ciphertext.clone(),
                encapsulated_key: row.encapsulated_key.clone(),
                key_version: row.key_version,
            },
            row.id,
            row.owner_id,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use famweather_core::crypto::{encrypt_credential, generate_keypair};
    use secrecy::ExposeSecret;

    fn row_for(
        pk: &famweather_core::crypto::CredentialPublicKey,
        v: i16,
        id: Uuid,
        owner: Uuid,
        key: &str,
    ) -> CredentialRow {
        let enc = encrypt_credential(pk, v, id, owner, &SecretString::from(key)).unwrap();
        CredentialRow {
            id,
            owner_id: owner,
            ciphertext: enc.ciphertext,
            encapsulated_key: enc.encapsulated_key,
            key_version: v,
        }
    }

    #[test]
    fn picks_the_key_by_version() {
        let (pk1, sk1) = generate_keypair();
        let (pk2, sk2) = generate_keypair();
        let ring = KeyRing::new((2, sk2), Some((1, sk1))).unwrap();
        let (id, owner) = (Uuid::new_v4(), Uuid::new_v4());
        let old = row_for(&pk1, 1, id, owner, "old-key-value-aaaa");
        let new = row_for(&pk2, 2, id, owner, "new-key-value-bbbb");
        assert_eq!(
            ring.decrypt_row(&old).unwrap().expose_secret(),
            "old-key-value-aaaa"
        );
        assert_eq!(
            ring.decrypt_row(&new).unwrap().expose_secret(),
            "new-key-value-bbbb"
        );
    }

    #[test]
    fn unknown_version_wrong_aad_and_tampering_fail() {
        let (pk1, sk1) = generate_keypair();
        let ring = KeyRing::new((1, sk1), None).unwrap();
        let (id, owner) = (Uuid::new_v4(), Uuid::new_v4());
        let ok = row_for(&pk1, 1, id, owner, "key-value-12345678");
        assert!(ring.decrypt_row(&ok).is_ok());

        let mut v3 = ok.clone();
        v3.key_version = 3;
        assert_eq!(ring.decrypt_row(&v3).unwrap_err(), CryptoError::InvalidKey);

        let mut moved = ok.clone();
        moved.id = Uuid::new_v4();
        assert_eq!(ring.decrypt_row(&moved).unwrap_err(), CryptoError::Decrypt);
        let mut other_owner = ok.clone();
        other_owner.owner_id = Uuid::new_v4();
        assert!(ring.decrypt_row(&other_owner).is_err());

        let mut tampered = ok.clone();
        tampered.ciphertext[0] ^= 1;
        assert_eq!(
            ring.decrypt_row(&tampered).unwrap_err(),
            CryptoError::Decrypt
        );
    }

    #[test]
    fn current_and_previous_must_differ_and_debug_hides_keys() {
        let (_, a) = generate_keypair();
        let (_, b) = generate_keypair();
        assert!(KeyRing::new((1, a.clone()), Some((1, b.clone()))).is_err());
        let ring = KeyRing::new((2, a), Some((1, b))).unwrap();
        let s = format!("{ring:?}");
        assert!(
            s.contains("current_version: 2") && s.contains("previous_version: Some(1)"),
            "{s}"
        );
    }
}
