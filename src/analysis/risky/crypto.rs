//! Weak cryptography in OpenSSL, CommonCrypto and the C library: broken
//! hashes and ciphers, ECB mode, and DES-era password hashing. Only the
//! calls that pick an algorithm are listed (`MD5_Init`, not every
//! `MD5_Update`), so one use is one finding. Turning off certificate checks
//! is judged from the arguments instead (`SSL_VERIFY_NONE`).

use super::Rule;
use crate::analysis::Severity;

pub(super) const RULES: &[Rule] = &[
    Rule {
        names: &[
            "DES_set_key",
            "DES_set_key_checked",
            "DES_set_key_unchecked",
            "DES_ecb_encrypt",
            "DES_ncbc_encrypt",
            "DES_ede3_cbc_encrypt",
            "RC4",
            "RC4_set_key",
            "BF_set_key",
            "EVP_des_ecb",
            "EVP_des_cbc",
            "EVP_des_ede3_cbc",
            "EVP_rc4",
            "EVP_rc2_cbc",
            "EVP_bf_cbc",
            "EVP_bf_ecb",
        ],
        severity: Severity::Medium,
        cwe: 327,
        why: "Uses a broken or retired cipher (DES, 3DES, RC4, RC2, Blowfish): data it \
              protects can be recovered.",
        fix: "Use an authenticated cipher such as AES-GCM or ChaCha20-Poly1305.",
    },
    Rule {
        names: &[
            "EVP_aes_128_ecb",
            "EVP_aes_192_ecb",
            "EVP_aes_256_ecb",
            "AES_ecb_encrypt",
        ],
        severity: Severity::Medium,
        cwe: 327,
        why: "ECB mode encrypts equal blocks to equal ciphertext, so patterns in the data \
              show through.",
        fix: "Use an authenticated mode such as AES-GCM.",
    },
    Rule {
        names: &[
            "MD4",
            "MD4_Init",
            "MD5",
            "MD5_Init",
            "SHA1",
            "SHA1_Init",
            "EVP_md4",
            "EVP_md5",
            "EVP_sha1",
            "CC_MD5",
            "CC_MD5_Init",
            "CC_SHA1",
            "CC_SHA1_Init",
        ],
        severity: Severity::Low,
        cwe: 328,
        why: "MD4, MD5 and SHA-1 have practical collision attacks: fine as a checksum, not \
              for signatures, passwords, or integrity against an attacker.",
        fix: "Use SHA-256 or better; for passwords, Argon2, scrypt or bcrypt.",
    },
    Rule {
        names: &["crypt", "crypt_r"],
        severity: Severity::Low,
        cwe: 916,
        why: "With an old-style salt it hashes passwords with DES or MD5, which are fast to \
              brute-force.",
        fix: "Use a slow password hash: Argon2, scrypt, or bcrypt ($2b$ / $y$ salts).",
    },
];
