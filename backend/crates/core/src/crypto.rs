//! Credential encryption (04 section 9; owned by 04-database).
//!
//! HPKE (RFC 9180), base mode: KEM `DHKEM(X25519, HKDF-SHA256)`, KDF `HKDF-SHA256`, AEAD `AES-256-GCM`, via the `hpke`
//! crate (exact version pinned in the workspace manifest).
//!
//! * `info` = [`HPKE_INFO`] (`family-weather/credential/v1`).
//! * AAD = the 16 raw bytes of `credential_id` followed by the 16 raw bytes of `owner_id`, so a ciphertext moved to
//!   another row fails to decrypt.
//! * `api` and `admin` hold the public key (encrypt only); only `poller` enables the cargo feature `decrypt`.
//!
//! Nothing here logs or returns key material or plaintext: [`CryptoError`] carries no data and the private key's
//! `Debug` prints `[REDACTED]`.

use base64::{engine::general_purpose::STANDARD, Engine};
use hpke::{
    aead::AesGcm256, kdf::HkdfSha256, kem::X25519HkdfSha256, Deserializable, Kem as KemTrait,
    OpModeS, Serializable,
};
use secrecy::{ExposeSecret, SecretString};
use uuid::Uuid;
use zeroize::Zeroizing;

type Kem = X25519HkdfSha256;
type Kdf = HkdfSha256;
type Aead = AesGcm256;

/// HPKE `info` (context binding), 00-conventions "Credential encryption".
pub const HPKE_INFO: &[u8] = b"family-weather/credential/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CryptoError {
    #[error("invalid credential key encoding")]
    InvalidKey,
    #[error("malformed encrypted credential")]
    InvalidCiphertext,
    #[error("credential encryption failed")]
    Encrypt,
    #[error("credential decryption failed")]
    Decrypt,
}

/// X25519 public key (32 raw bytes). Not secret.
#[derive(Clone, PartialEq, Eq)]
pub struct CredentialPublicKey([u8; 32]);

/// X25519 private key (32 raw bytes). Zeroed on drop; `Debug` prints nothing sensitive.
#[derive(Clone)]
pub struct CredentialPrivateKey(Zeroizing<[u8; 32]>);

impl CredentialPublicKey {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Standard base64 (with padding) of the raw key, the form used in `CREDENTIALS_PUBLIC_KEY`.
    pub fn to_base64(&self) -> String {
        STANDARD.encode(self.0)
    }
}

impl CredentialPrivateKey {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Standard base64 of the raw key, the form used in `CREDENTIALS_PRIVATE_KEY`.
    pub fn to_base64(&self) -> SecretString {
        SecretString::from(STANDARD.encode(*self.0))
    }
}

impl std::fmt::Debug for CredentialPublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CredentialPublicKey(..)")
    }
}

impl std::fmt::Debug for CredentialPrivateKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CredentialPrivateKey([REDACTED])")
    }
}

fn parse_32(b64: &str) -> Result<[u8; 32], CryptoError> {
    let raw = Zeroizing::new(STANDARD.decode(b64).map_err(|_| CryptoError::InvalidKey)?);
    <[u8; 32]>::try_from(raw.as_slice()).map_err(|_| CryptoError::InvalidKey)
}

/// Base64 (standard, padded) of the raw 32-byte public key.
pub fn parse_public_key(b64: &str) -> Result<CredentialPublicKey, CryptoError> {
    parse_32(b64).map(CredentialPublicKey)
}

/// Base64 (standard, padded) of the raw 32-byte private key.
pub fn parse_private_key(b64: &str) -> Result<CredentialPrivateKey, CryptoError> {
    parse_32(b64).map(|k| CredentialPrivateKey(Zeroizing::new(k)))
}

/// A freshly generated X25519 key pair (`admin keygen`, tests).
pub fn generate_keypair() -> (CredentialPublicKey, CredentialPrivateKey) {
    let (sk, pk) = Kem::gen_keypair();
    (
        CredentialPublicKey(to_array(&pk.to_bytes())),
        CredentialPrivateKey(Zeroizing::new(to_array(&sk.to_bytes()))),
    )
}

/// The public key belonging to `sk`. Always compiled: it contains no decryption code.
pub fn public_key_from_private(sk: &CredentialPrivateKey) -> CredentialPublicKey {
    // Every 32-byte string is a valid X25519 private key, so parsing cannot fail.
    let sk = <Kem as KemTrait>::PrivateKey::from_bytes(sk.as_bytes()).expect("32-byte X25519 key");
    let pk = Kem::sk_to_pk(&sk);
    CredentialPublicKey(to_array(&pk.to_bytes()))
}

/// True when `pk` is the public half of `sk` (`admin keygen --check`).
pub fn keypair_matches(sk: &CredentialPrivateKey, pk: &CredentialPublicKey) -> bool {
    public_key_from_private(sk) == *pk
}

fn to_array(bytes: &[u8]) -> [u8; 32] {
    <[u8; 32]>::try_from(bytes).expect("X25519 keys are 32 bytes")
}

/// `credential_id || owner_id`, raw UUID bytes (`Uuid::as_bytes`).
fn aad(credential_id: Uuid, owner_id: Uuid) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[..16].copy_from_slice(credential_id.as_bytes());
    out[16..].copy_from_slice(owner_id.as_bytes());
    out
}

/// What is stored in `api_key_ciphertext`, `encapsulated_key` and `key_version`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptedCredential {
    /// AEAD output: ciphertext followed by the 16-byte tag.
    pub ciphertext: Vec<u8>,
    /// The 32-byte KEM output.
    pub encapsulated_key: Vec<u8>,
    pub key_version: i16,
}

/// Encrypts an AWN API key to `pk`. A fresh ephemeral key (so a fresh AEAD key and nonce) is created per call.
pub fn encrypt_credential(
    pk: &CredentialPublicKey,
    key_version: i16,
    credential_id: Uuid,
    owner_id: Uuid,
    api_key: &SecretString,
) -> Result<EncryptedCredential, CryptoError> {
    let pk = <Kem as KemTrait>::PublicKey::from_bytes(pk.as_bytes())
        .map_err(|_| CryptoError::InvalidKey)?;
    let (encapped, ciphertext) = hpke::single_shot_seal::<Aead, Kdf, Kem>(
        &OpModeS::Base,
        &pk,
        HPKE_INFO,
        api_key.expose_secret().as_bytes(),
        &aad(credential_id, owner_id),
    )
    .map_err(|_| CryptoError::Encrypt)?;
    Ok(EncryptedCredential {
        ciphertext,
        encapsulated_key: encapped.to_bytes().to_vec(),
        key_version,
    })
}

/// Decrypts a stored credential. Compiled only with the cargo feature `decrypt` (the poller and tests).
#[cfg(feature = "decrypt")]
pub fn decrypt_credential(
    sk: &CredentialPrivateKey,
    enc: &EncryptedCredential,
    credential_id: Uuid,
    owner_id: Uuid,
) -> Result<SecretString, CryptoError> {
    use hpke::OpModeR;

    let sk = <Kem as KemTrait>::PrivateKey::from_bytes(sk.as_bytes())
        .map_err(|_| CryptoError::InvalidKey)?;
    if enc.encapsulated_key.len() != <<Kem as KemTrait>::EncappedKey as Serializable>::size() {
        return Err(CryptoError::InvalidCiphertext);
    }
    let encapped = <Kem as KemTrait>::EncappedKey::from_bytes(&enc.encapsulated_key)
        .map_err(|_| CryptoError::InvalidCiphertext)?;
    let plaintext = Zeroizing::new(
        hpke::single_shot_open::<Aead, Kdf, Kem>(
            &OpModeR::Base,
            &sk,
            &encapped,
            HPKE_INFO,
            &enc.ciphertext,
            &aad(credential_id, owner_id),
        )
        .map_err(|_| CryptoError::Decrypt)?,
    );
    let text = std::str::from_utf8(&plaintext).map_err(|_| CryptoError::Decrypt)?;
    Ok(SecretString::from(text.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hpke::{setup_receiver, OpModeR};

    const KEY: &str = "canary-secret-awnkey-0123";

    fn ids() -> (Uuid, Uuid) {
        (Uuid::new_v4(), Uuid::new_v4())
    }

    #[test]
    fn parses_32_byte_keys_only() {
        let ok = STANDARD.encode([7u8; 32]);
        assert!(parse_public_key(&ok).is_ok());
        assert!(parse_private_key(&ok).is_ok());
        assert_eq!(
            parse_public_key(&STANDARD.encode([7u8; 31])),
            Err(CryptoError::InvalidKey)
        );
        assert_eq!(
            parse_private_key("not base64!!").map(|_| ()),
            Err(CryptoError::InvalidKey)
        );
    }

    #[test]
    fn private_key_debug_is_redacted() {
        let k = parse_private_key(&STANDARD.encode([7u8; 32])).unwrap();
        assert!(format!("{k:?}").contains("[REDACTED]"));
        assert!(!format!("{:?}", k.to_base64()).contains(&STANDARD.encode([7u8; 32])));
    }

    #[test]
    fn generated_pairs_match_and_differ() {
        let (pk, sk) = generate_keypair();
        assert!(keypair_matches(&sk, &pk));
        assert_eq!(public_key_from_private(&sk), pk);
        let (pk2, sk2) = generate_keypair();
        assert_ne!(pk, pk2);
        assert!(!keypair_matches(&sk2, &pk));
        // Base64 round trip, the form the environment variables use.
        let again = parse_public_key(&pk.to_base64()).unwrap();
        assert_eq!(again, pk);
        use secrecy::ExposeSecret as _;
        let sk_again = parse_private_key(sk.to_base64().expose_secret()).unwrap();
        assert!(keypair_matches(&sk_again, &pk));
    }

    /// Plain `openssl`-style demo pair of `.env.example` (05): derived public key must match.
    #[test]
    fn demo_pair_from_env_example_matches() {
        let sk = parse_private_key("aOsR//GEEIuFGuddaSfV8M83+qE2+2fpEBFCqQfltX4=").unwrap();
        let pk = parse_public_key("RxUrKamgbeYZkmLE3mZtzRHQP3MBBU9kZKkedBhteQk=").unwrap();
        assert!(keypair_matches(&sk, &pk));
    }

    #[test]
    fn encryption_has_the_documented_shape_and_is_randomised() {
        let (pk, _sk) = generate_keypair();
        let (cid, oid) = ids();
        let key = SecretString::from(KEY.to_owned());
        let a = encrypt_credential(&pk, 3, cid, oid, &key).unwrap();
        let b = encrypt_credential(&pk, 3, cid, oid, &key).unwrap();
        assert_eq!(a.key_version, 3);
        assert_eq!(a.encapsulated_key.len(), 32);
        assert_eq!(
            a.ciphertext.len(),
            KEY.len() + 16,
            "ciphertext || 16-byte tag"
        );
        assert_ne!(
            a.encapsulated_key, b.encapsulated_key,
            "fresh encapsulation per seal"
        );
        assert_ne!(a.ciphertext, b.ciphertext);
    }

    #[cfg(feature = "decrypt")]
    mod decrypt {
        use super::*;

        const PIN_SK: [u8; 32] = [0x42; 32];
        const PIN_CID: &str = "11111111-2222-3333-4444-555555555555";
        const PIN_OID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        const PIN_PLAINTEXT: &str = "pinned-test-key-0123456789";
        const PIN_ENC: &str = "b50bdde76e6f4e0b129cf45efd3aec0081afe205ae099db705a9af1e1c77260d";
        const PIN_CT: &str =
            "7ed77938a231adbe038dd455cdb6bb950b2d0c25c1e76dbba0fff3142b67711380e752a6127fb6221c0c";

        /// Format pin: a ciphertext produced once by `encrypt_credential` (fixed key pair, ids, plaintext) must keep
        /// decrypting with `decrypt_credential`. Changing HPKE_INFO, the AAD byte order or the suite breaks it.
        #[test]
        fn pinned_ciphertext_still_decrypts() {
            let sk = parse_private_key(&STANDARD.encode(PIN_SK)).unwrap();
            let (cid, oid): (Uuid, Uuid) = (PIN_CID.parse().unwrap(), PIN_OID.parse().unwrap());
            if PIN_CT.is_empty() {
                let pk = public_key_from_private(&sk);
                let e = encrypt_credential(
                    &pk,
                    1,
                    cid,
                    oid,
                    &SecretString::from(PIN_PLAINTEXT.to_owned()),
                )
                .unwrap();
                panic!(
                    "PIN_ENC={} PIN_CT={}",
                    hex::encode(&e.encapsulated_key),
                    hex::encode(&e.ciphertext)
                );
            }
            let enc = EncryptedCredential {
                ciphertext: hex(PIN_CT),
                encapsulated_key: hex(PIN_ENC),
                key_version: 1,
            };
            assert_eq!(
                decrypt_credential(&sk, &enc, cid, oid)
                    .unwrap()
                    .expose_secret(),
                PIN_PLAINTEXT
            );
            // The AAD order is part of the format.
            assert!(decrypt_credential(&sk, &enc, oid, cid).is_err());
        }

        #[test]
        fn round_trip() {
            let (pk, sk) = generate_keypair();
            let (cid, oid) = ids();
            let enc =
                encrypt_credential(&pk, 1, cid, oid, &SecretString::from(KEY.to_owned())).unwrap();
            let back = decrypt_credential(&sk, &enc, cid, oid).unwrap();
            assert_eq!(back.expose_secret(), KEY);
        }

        #[test]
        fn a_row_swap_fails() {
            let (pk, sk) = generate_keypair();
            let (cid, oid) = ids();
            let enc =
                encrypt_credential(&pk, 1, cid, oid, &SecretString::from(KEY.to_owned())).unwrap();
            // Moved to another credential id, another owner, or both: authentication fails.
            for (c, o) in [
                (Uuid::new_v4(), oid),
                (cid, Uuid::new_v4()),
                (Uuid::new_v4(), Uuid::new_v4()),
                (oid, cid), // the two halves of the AAD swapped
            ] {
                assert_eq!(
                    decrypt_credential(&sk, &enc, c, o).map(|_| ()),
                    Err(CryptoError::Decrypt)
                );
            }
        }

        #[test]
        fn tampering_wrong_key_and_malformed_input_fail() {
            let (pk, sk) = generate_keypair();
            let (_, other_sk) = generate_keypair();
            let (cid, oid) = ids();
            let enc =
                encrypt_credential(&pk, 1, cid, oid, &SecretString::from(KEY.to_owned())).unwrap();
            assert!(decrypt_credential(&other_sk, &enc, cid, oid).is_err());
            let mut flipped = enc.clone();
            flipped.ciphertext[0] ^= 1;
            assert!(decrypt_credential(&sk, &flipped, cid, oid).is_err());
            let mut tag = enc.clone();
            *tag.ciphertext.last_mut().unwrap() ^= 1;
            assert!(decrypt_credential(&sk, &tag, cid, oid).is_err());
            let mut short = enc.clone();
            short.encapsulated_key.pop();
            assert_eq!(
                decrypt_credential(&sk, &short, cid, oid).map(|_| ()),
                Err(CryptoError::InvalidCiphertext)
            );
            let mut empty = enc;
            empty.ciphertext.clear();
            assert!(decrypt_credential(&sk, &empty, cid, oid).is_err());
        }

        #[test]
        fn plaintext_must_be_utf8_and_secrets_stay_out_of_debug() {
            let (pk, sk) = generate_keypair();
            let (cid, oid) = ids();
            let enc =
                encrypt_credential(&pk, 1, cid, oid, &SecretString::from(KEY.to_owned())).unwrap();
            let back = decrypt_credential(&sk, &enc, cid, oid).unwrap();
            assert!(!format!("{back:?}").contains(KEY));
            assert!(!format!("{:?}", CryptoError::Decrypt).contains(KEY));
        }
    }

    // ---- RFC 9180 known-answer test --------------------------------------------------------------------------
    //
    // The vector is the CFRG HPKE test vector for mode 0 (base), KEM 0x0020 DHKEM(X25519, HKDF-SHA256), KDF 0x0001
    // HKDF-SHA256, AEAD 0x0002 AES-256-GCM, taken from `test-vectors/origrfc-5f503c5.json` in the `hpke` 0.14.1
    // source (the same data set as the RFC 9180 appendix; the appendix itself lists X25519 only with AES-128-GCM
    // and ChaCha20-Poly1305, so this is the vector that matches our exact suite).

    fn hex(s: &str) -> Vec<u8> {
        hex::decode(s).expect("hex")
    }

    const INFO: &str = "4f6465206f6e2061204772656369616e2055726e";
    const IKM_R: &str = "dac33b0e9db1b59dbbea58d59a14e7b5896e9bdf98fad6891e99d1686492b9ee";
    const IKM_E: &str = "2cd7c601cefb3d42a62b04b7a9041494c06c7843818e0ce28a8f704ae7ab20f9";
    const SK_RM: &str = "497b4502664cfea5d5af0b39934dac72242a74f8480451e1aee7d6a53320333d";
    const PK_RM: &str = "430f4b9859665145a6b1ba274024487bd66f03a2dd577d7753c68d7d7d00c00c";
    const ENC: &str = "6c93e09869df3402d7bf231bf540fadd35cd56be14f97178f0954db94b7fc256";
    const PT: &str = "4265617574792069732074727574682c20747275746820626561757479";
    // (aad, ciphertext) for the sequence numbers 0 and 1
    const SEALED: [(&str, &str); 2] = [
        (
            "436f756e742d30",
            "e5d84cd531cfb583096e7cfa9641bd3079cf3a91cda813c52deb5f512be9931980a41de125a925cdad859d5b7a",
        ),
        (
            "436f756e742d31",
            "2c43aff25343fdbff864506f0818b9d87df84ea01b1a2144d23b4d40c26bf655fdf197fe40297a8aebeed5cc2d",
        ),
    ];

    #[test]
    fn rfc9180_kat_key_derivation() {
        let (sk_r, pk_r) = Kem::derive_keypair(&hex(IKM_R));
        assert_eq!(sk_r.to_bytes().to_vec(), hex(SK_RM));
        assert_eq!(pk_r.to_bytes().to_vec(), hex(PK_RM));
        let (_sk_e, pk_e) = Kem::derive_keypair(&hex(IKM_E));
        assert_eq!(pk_e.to_bytes().to_vec(), hex(ENC));
    }

    #[test]
    fn rfc9180_kat_open() {
        let sk_r = <Kem as KemTrait>::PrivateKey::from_bytes(&hex(SK_RM)).unwrap();
        let enc = <Kem as KemTrait>::EncappedKey::from_bytes(&hex(ENC)).unwrap();
        let mut ctx =
            setup_receiver::<Aead, Kdf, Kem>(&OpModeR::Base, &sk_r, &enc, &hex(INFO)).unwrap();
        for (aad, ct) in SEALED {
            let pt = ctx.open(&hex(ct), &hex(aad)).unwrap();
            assert_eq!(pt, hex(PT));
        }
    }

    /// Replaces the `hpke` random source with the vector's ephemeral input keying material, so sealing is
    /// deterministic and must reproduce the published `enc` and ciphertext bytes.
    struct FixedIkm(Vec<u8>);
    impl rand_core::TryRng for FixedIkm {
        type Error = core::convert::Infallible;
        fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
            unreachable!("only fill_bytes is used")
        }
        fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
            unreachable!("only fill_bytes is used")
        }
        fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), Self::Error> {
            assert_eq!(dest.len(), self.0.len());
            dest.copy_from_slice(&self.0);
            Ok(())
        }
    }
    impl rand_core::TryCryptoRng for FixedIkm {}

    #[test]
    fn rfc9180_kat_seal_is_reproduced() {
        let pk_r = <Kem as KemTrait>::PublicKey::from_bytes(&hex(PK_RM)).unwrap();
        let mut rng = FixedIkm(hex(IKM_E));
        let (enc, ct) = hpke::single_shot_seal_with_rng::<Aead, Kdf, Kem>(
            &OpModeS::Base,
            &pk_r,
            &hex(INFO),
            &hex(PT),
            &hex(SEALED[0].0),
            &mut rng,
        )
        .unwrap();
        assert_eq!(enc.to_bytes().to_vec(), hex(ENC));
        assert_eq!(ct, hex(SEALED[0].1));
    }
}
