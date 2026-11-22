//! WireGuard keys (Curve25519) encoded the same way as `wg genkey`.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use boringtun::x25519::{PublicKey, StaticSecret};
use rand_core::{OsRng, RngCore};
use std::fmt;
use std::str::FromStr;

/// A 32-byte WireGuard key (private, public or pre-shared).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key([u8; 32]);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("invalid key: expected a 44-character base64 WireGuard key")]
pub struct InvalidKey;

impl Key {
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Key(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Generate a new private key, clamped exactly like `wg genkey`.
    pub fn generate_private() -> Self {
        let mut bytes = [0u8; 32];
        OsRng.fill_bytes(&mut bytes);
        bytes[0] &= 248;
        bytes[31] &= 127;
        bytes[31] |= 64;
        Key(bytes)
    }

    /// Generate a random pre-shared key (`wg genpsk`).
    pub fn generate_preshared() -> Self {
        let mut bytes = [0u8; 32];
        OsRng.fill_bytes(&mut bytes);
        Key(bytes)
    }

    /// Derive the public key from this private key.
    pub fn public_key(&self) -> Key {
        Key(PublicKey::from(&self.static_secret()).to_bytes())
    }

    pub fn to_base64(&self) -> String {
        B64.encode(self.0)
    }

    /// A short, human friendly prefix of the base64 form (for public keys only).
    pub fn short(&self) -> String {
        let mut s = self.to_base64();
        s.truncate(8);
        s.push('…');
        s
    }

    pub(crate) fn static_secret(&self) -> StaticSecret {
        StaticSecret::from(self.0)
    }

    pub(crate) fn x25519_public(&self) -> PublicKey {
        PublicKey::from(self.0)
    }
}

impl FromStr for Key {
    type Err = InvalidKey;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Windows PowerShell may prepend a BOM when piping into native programs.
        let s = s.trim().trim_start_matches('\u{feff}').trim();
        if s.len() != 44 {
            return Err(InvalidKey);
        }
        let bytes = B64.decode(s).map_err(|_| InvalidKey)?;
        let arr: [u8; 32] = bytes.try_into().map_err(|_| InvalidKey)?;
        Ok(Key(arr))
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_base64())
    }
}

impl fmt::Debug for Key {
    // Never print key material in debug output or logs.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Key(<redacted>)")
    }
}

/// Random hex string (used for local control tokens).
pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    OsRng.fill_bytes(&mut buf);
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> [u8; 32] {
        let v: Vec<u8> =
            (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect();
        v.try_into().unwrap()
    }

    #[test]
    fn rfc7748_public_key_vector() {
        // RFC 7748 section 6.1 (Alice).
        let private = Key(hex("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a"));
        let public = Key(hex("8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a"));
        assert_eq!(private.public_key(), public);
    }

    #[test]
    fn base64_roundtrip() {
        let k = Key::generate_private();
        let parsed: Key = k.to_base64().parse().unwrap();
        assert_eq!(k, parsed);
        assert_eq!(k.to_base64().len(), 44);
    }

    #[test]
    fn generated_private_keys_are_clamped() {
        let k = Key::generate_private();
        assert_eq!(k.0[0] & 7, 0);
        assert_eq!(k.0[31] & 128, 0);
        assert_eq!(k.0[31] & 64, 64);
    }

    #[test]
    fn tolerates_bom_and_newlines() {
        let k = Key::generate_private();
        let piped = format!("\u{feff}{}\r\n", k.to_base64());
        assert_eq!(piped.parse::<Key>().unwrap(), k);
    }

    #[test]
    fn rejects_bad_keys() {
        assert_eq!("abc".parse::<Key>(), Err(InvalidKey));
        assert_eq!("!".repeat(44).parse::<Key>(), Err(InvalidKey));
    }

    #[test]
    fn debug_is_redacted() {
        let k = Key::generate_private();
        assert!(!format!("{k:?}").contains(&k.to_base64()));
    }
}
