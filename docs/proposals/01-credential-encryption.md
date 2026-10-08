# Proposal 01: How to encrypt users' AWN API keys

**Status:** Accepted by Rodney, Oct 5, 2026. Date: 2026-10-02.
Touches decisions: S5, S8, S13, P6, G6, G8.

## Problem

S5 says only the poller may read users' AWN API keys, so a hacked API cannot leak them. P6 says users paste their key into a connect screen served by the API, which has to save it. With ordinary single-key (symmetric) encryption, saving needs the same key that reads, so the API would hold the key and S5 would be empty. We need a way for the API to lock a key that only the poller can unlock.

What is at stake: an AWN key gives read-only access to one person's weather station. It is low-value compared with a password or payment data, which sets how much effort this deserves.

One limit applies to every option below. The API sees the pasted key in plain text for a moment, because it must test the key against AWN (the connect flow). So S5 protects the keys already stored, not the one being pasted during an attack. Only the poller-only decryption property is in question here.

## Threats used for comparison

1. **API process compromised.** Can the attacker read stored keys?
2. **Database or backup leak.** Can a stolen dump be read?
3. **Fly secrets leak.** What does the attacker get if the secrets of one or both Fly process groups are exposed?

## Options compared

| | (a) HPKE | (b) Sealed box / age | (c) API forwards plaintext, poller encrypts | (d) AWS KMS | (e) Supabase Vault / pgsodium | (f) Shared AES-GCM key |
|---|---|---|---|---|---|---|
| API hacked: stored keys safe? | Yes | Yes | Yes for old keys, but plaintext passes through a handoff | Yes (Encrypt-only role), but the AWS credential sits on the API | No: API's DB role or SQL can ask Vault to decrypt | No |
| DB or backup leak safe? | Yes | Yes | Yes | Yes | Yes for dumps (root key is held by Supabase); no for anyone with live SQL access | Yes |
| Fly secrets leak safe? | Poller leak exposes private key; API leak exposes only the public key | Same as (a) | Same as (a) if the poller holds the key | Leaked AWS credential can decrypt while it is valid; nothing sits in Fly but a credential | Not applicable | Either process leak exposes the key |
| Effort for one developer | Low-medium | Low | Medium and risky | Medium-high | Low | Lowest |
| Rust crate maturity | `hpke`: unaudited, one external review | `age`: well regarded, no AAD | n/a (uses AES-GCM crates) | `aws-sdk-kms`: official | none needed (SQL) | `aes-gcm`: audited (NCC Group) |
| Key rotation | Manual, you script it | Manual | Manual | Built in (but see below) | Supabase-managed | Manual |
| Works offline in Docker Compose | Yes, same code, dev key pair | Yes | Yes | No, needs a stand-in (LocalStack or a second code path) | No, Vault is not in the plain Postgres image | Yes |
| Cost | $0 | $0 | $0 | About $1/month per key, requests mostly free | $0 | $0 |

### (a) HPKE (the current tentative choice)

HPKE is an IETF standard (RFC 9180) for "encrypt to someone's public key". The API holds the public key and can lock a key but not unlock it. The poller holds the private key. It supports the row-binding we want (a value that ties each ciphertext to its credential and owner, so rows cannot be swapped) as a built-in feature, not something we bolt on. A key version column handles rotation. Everything runs offline with a generated dev key pair. The cost is one library: the Rust `hpke` crate is a clean RustCrypto-based implementation, but its own documentation says it has not had a formal audit. Cloudflare reviewed version 0.8 and found no issues. A formally verified alternative exists (`hpke-rs` with a libcrux backend) if that matters.

### (b) Sealed boxes (libsodium) or `age`

These also let one side lock and the other unlock, and they are simple. Neither has an "associated data" input, so row-binding has to be faked by putting `credential_id || owner_id` inside the secret and checking it after decrypting. That works and is easy to get right, but it is a workaround. `age` is a file-encryption format with extra header overhead and is aimed at files, not database columns. Sealed boxes are fine technically, but they do the same job as (a) with a weaker fit, and the Rust ecosystem for them is smaller. Not worth switching from (a) unless HPKE's crate worries you.

### (c) API passes plaintext to the poller

The API would hand the pasted key to the poller (a database queue row, or a call over Fly's private network), and the poller would encrypt it with one AES-GCM key. This keeps S5 for stored keys, but it adds a new moving part: either plaintext sits in a database table for a while, which breaks the "keys never sit in plain text in the database" idea and ends up in backups, or a new internal service has to be built, authenticated and kept running. If the poller is down, saving fails. It is more work and more risk than (a) for the same result. Rejected.

### (d) AWS KMS

The API gets an IAM user that can only Encrypt, and the poller one that can only Decrypt, so no key material ever lives in Fly. KMS keeps the key, handles rotation and gives an audit trail in CloudTrail. [Pricing](https://aws.amazon.com/kms/pricing/): $1 per month per key, $0.03 per 10,000 requests, with 20,000 requests free each month. A poller that decrypts at startup and caches in memory would cost about $1 a month. Downsides: the poller can no longer start or recover if AWS is unreachable; offline Docker Compose needs a second code path or an emulator, which is exactly the "works differently in production" risk we want to avoid; and the two IAM credentials still have to be stored as Fly secrets, so a Fly secrets leak still gives an attacker a way to decrypt, plus ongoing IAM care for a solo developer. Strong, but more moving parts than the threat justifies. A reasonable upgrade later, since the stored format could change without changing the schema much.

### (e) Supabase Vault / pgsodium

Supabase now says it does not recommend pgsodium and its column encryption feature, and that pgsodium is "pending deprecation" ([pgsodium docs](https://supabase.com/docs/guides/database/extensions/pgsodium), [discussion #27109](https://github.com/orgs/supabase/discussions/27109)). Vault itself is separate and supported. Vault keeps its root key outside your database, so a stolen dump is unreadable ([Vault docs](https://supabase.com/docs/guides/database/vault)). But the same docs say anyone who can query the `vault.decrypted_secrets` view sees plain text. Any role with that access, including the API's if it is allowed to insert, can read everything, which defeats S5. It also does not exist in the plain-Postgres local setup, and it ties us to Supabase. Rejected.

### (f) One shared AES-GCM key in both processes

Simplest by far and fully offline. It is exactly the weakness S5 was written to remove: a hacked API can read every key, and the "separate database logins" fix is bypassed because the key is in the same process. For a five-person family app some owners might accept this, but the cost of doing better (a) is small, so it is not the right place to cut corners.

## Recommendation

Adopt **(a) HPKE**, as the orchestrator chose, with these specifics:

- Base mode, X25519 + HKDF-SHA256 + AES-256-GCM, via the Rust `hpke` crate (RustCrypto primitives). Pin the exact version and add a known-answer test from RFC 9180 test vectors, so a crate bug or upgrade is caught.
- Associated data = `credential_id || owner_id`, as already specified. Add a unit test that moving ciphertext between rows fails to decrypt.
- Store `api_key_ciphertext`, `encapsulated_key`, `key_version`. The poller decrypts when it starts or when a credential changes, keeps keys in memory (`secrecy`), and never logs them.
- The public key is not secret, so it can live in config in every environment. The private key is a Fly secret on the poller only, and a generated dev key pair goes in `.env.example` (local only, rejected in production, same rule as the JWT dev secret).
- The poller (not the API) runs the re-encryption job for rotation.

Why:

1. It is the only simple option that makes S5 true and P6 workable together. (b) does the same with a worse fit, (f) breaks S5, (c) adds risk.
2. It needs no outside service, costs nothing and behaves the same locally and in production.
3. It satisfies S8 (fresh randomness per encryption, row-binding, key version) with no custom crypto: the encapsulation step makes every ciphertext unique.
4. Realistic threat coverage: API compromise cannot read stored keys; a database or backup leak (S13) is unreadable; a poller secrets leak is the one real exposure, and it would also be one for (d).
5. If the crate's lack of a formal audit is unacceptable, swapping to `hpke-rs` (libcrux) or (b) later is a contained change, because only one module and a version number change.

## What changes in the docs if accepted

- **Design Doc, "Credential handling"**: the first bullet already matches. Add one sentence: "The API holds only the public key; a stored key can be read only by the poller, but the API sees a key in plain text while checking it during connect."
- **Design Doc, "Key rotation"**: no change.
- **00-conventions.md "Credential encryption"**: add the crate-version pin, the RFC 9180 test-vector test, the row-swap test, the rule that the poller decrypts once into memory, and the dev key pair rule in the environment variable table (`CREDENTIALS_PUBLIC_KEY` for api and poller-tests, `CREDENTIALS_PRIVATE_KEY` poller only, rejected if equal to the committed dev key when `APP_ENV=production`).
- **Design Review Findings, S5 and S8**: add "Resolved via HPKE; see proposal 01". S13 stays as is.
- **Milestone checklist**: add a "private key never reaches the API image's environment" check to the Fly config review, in the same phase as the poller going live.
- If (d) or another option is chosen instead, the Design Doc's local development section also needs an offline stand-in plan.

## Open risks

- **Unaudited crate.** `hpke` has had an informal review only. Mitigated by pinning, known-answer tests and the small usage surface. Revisit if the app grows.
- **API still sees the pasted key.** A compromised API can steal keys as users paste them, and can also overwrite a stored ciphertext with its own (it holds the public key). Mitigation: the API's database role can insert but not update ciphertext columns (use delete and re-insert via the connect flow), and every credential change goes in the audit log.
- **Poller secrets leak.** If the poller's Fly secrets leak with a database dump, every key is readable. Keep dumps and the private key in separate places, as S13 already requires. Rotate after any suspected leak.
- **Private key loss.** Losing the private key without a copy means every user must reconnect. Store an offline backup (printed or in a password manager) and test a recovery.
- **Rotation is hand-built.** It is a script we write and must test. At five users the stakes are low, but do a rotation drill once before relying on it.
- **Blast radius is small.** An AWN key gives read-only access to weather data. If the owner later wants stronger protection (audit trail, no key in Fly), option (d) is the upgrade path, and its extra setup is not justified today.

## Sources

- [RFC 9180, Hybrid Public Key Encryption](https://www.rfc-editor.org/rfc/rfc9180.html)
- [`hpke` crate (rozbb/rust-hpke)](https://crates.io/crates/hpke), [`hpke-rs` (cryspen)](https://github.com/cryspen/hpke-rs)
- [`age` crate](https://crates.io/crates/age)
- [libsodium sealed boxes](https://doc.libsodium.org/public-key_cryptography/sealed_boxes)
- [AWS KMS pricing](https://aws.amazon.com/kms/pricing/)
- [Supabase pgsodium (pending deprecation)](https://supabase.com/docs/guides/database/extensions/pgsodium), [Supabase Vault](https://supabase.com/docs/guides/database/vault)
