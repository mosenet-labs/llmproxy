use aes_gcm::{
    Aes256Gcm, KeyInit,
    aead::{Aead, Generate, Nonce, Payload},
};
use base64::{Engine, engine::general_purpose::STANDARD};

use crate::{StoreError, StoreResult};

const AAD: &[u8] = b"llmproxy.provider-key.v1";

#[derive(Clone)]
pub(crate) struct KeyCipher(Aes256Gcm);

impl KeyCipher {
    pub fn new(master_key: &str) -> StoreResult<Self> {
        let bytes = STANDARD.decode(master_key).map_err(|_| {
            StoreError::Validation("主密钥必须是 32 字节随机数据的 Base64 编码".into())
        })?;
        let cipher = Aes256Gcm::new_from_slice(&bytes).map_err(|_| {
            StoreError::Validation("主密钥必须是 32 字节随机数据的 Base64 编码".into())
        })?;
        Ok(Self(cipher))
    }

    pub fn encrypt(&self, secret: &str) -> StoreResult<String> {
        let nonce = Nonce::<Aes256Gcm>::generate();
        let ciphertext = self
            .0
            .encrypt(
                &nonce,
                Payload {
                    msg: secret.as_bytes(),
                    aad: AAD,
                },
            )
            .map_err(|_| StoreError::Internal)?;
        let mut envelope = Vec::with_capacity(nonce.len() + ciphertext.len());
        envelope.extend_from_slice(&nonce);
        envelope.extend_from_slice(&ciphertext);
        Ok(format!("v1:{}", STANDARD.encode(envelope)))
    }

    pub fn decrypt(&self, encrypted: &str) -> StoreResult<String> {
        let encoded = encrypted.strip_prefix("v1:").ok_or(StoreError::Internal)?;
        let envelope = STANDARD.decode(encoded).map_err(|_| StoreError::Internal)?;
        if envelope.len() < 12 + 16 {
            return Err(StoreError::Internal);
        }
        let nonce: Nonce<Aes256Gcm> = envelope[..12]
            .try_into()
            .map_err(|_| StoreError::Internal)?;
        let plaintext = self
            .0
            .decrypt(
                &nonce,
                Payload {
                    msg: &envelope[12..],
                    aad: AAD,
                },
            )
            .map_err(|_| StoreError::Internal)?;
        String::from_utf8(plaintext).map_err(|_| StoreError::Internal)
    }

    pub fn replacement(&self, input: &str, existing: &str) -> StoreResult<String> {
        if input.is_empty() {
            Ok(existing.to_owned())
        } else {
            self.encrypt(input)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cipher(byte: u8) -> KeyCipher {
        KeyCipher::new(&STANDARD.encode([byte; 32])).unwrap()
    }

    #[test]
    fn encryption_roundtrip_uses_fresh_nonces() {
        let cipher = cipher(7);
        let first = cipher.encrypt("secret-provider-key").unwrap();
        let second = cipher.encrypt("secret-provider-key").unwrap();
        assert_ne!(first, second);
        assert!(!first.contains("secret-provider-key"));
        assert_eq!(cipher.decrypt(&first).unwrap(), "secret-provider-key");
    }

    #[test]
    fn wrong_key_and_modified_ciphertext_are_rejected() {
        let encrypted = cipher(7).encrypt("secret-provider-key").unwrap();
        assert_eq!(cipher(8).decrypt(&encrypted), Err(StoreError::Internal));
        let mut envelope = STANDARD
            .decode(encrypted.strip_prefix("v1:").unwrap())
            .unwrap();
        envelope[12] ^= 1;
        assert_eq!(
            cipher(7).decrypt(&format!("v1:{}", STANDARD.encode(envelope))),
            Err(StoreError::Internal)
        );
    }

    #[test]
    fn blank_edit_preserves_ciphertext_and_replacement_changes_it() {
        let cipher = cipher(7);
        let original = cipher.encrypt("old-secret").unwrap();
        assert_eq!(cipher.replacement("", &original).unwrap(), original);
        let replacement = cipher.replacement("new-secret", &original).unwrap();
        assert_eq!(cipher.decrypt(&replacement).unwrap(), "new-secret");
    }

    #[test]
    fn invalid_master_keys_and_envelopes_are_rejected_safely() {
        for key in ["not-base64", "", "c2hvcnQ="] {
            assert!(KeyCipher::new(key).is_err());
        }
        for value in ["secret", "v1:", "v1:garbage", "v2:abcd"] {
            assert_eq!(cipher(7).decrypt(value), Err(StoreError::Internal));
        }
    }
}
