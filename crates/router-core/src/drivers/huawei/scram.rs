//! SCRAM-SHA256 as implemented by the router's SCRAMJS login script (spec §5.3).
//! Never logs or prints the password, nonces or signatures.

use std::fmt;

use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::RouterError;

type HmacSha256 = Hmac<Sha256>;

fn hmac(key: &[u8], msg: &[u8]) -> Vec<u8> {
    let mut m = <HmacSha256 as KeyInit>::new_from_slice(key).expect("HMAC accepts any key length");
    m.update(msg);
    m.finalize().into_bytes().to_vec()
}

pub struct ScramClient {
    first_nonce_hex: String,
}

impl Default for ScramClient {
    fn default() -> Self {
        Self::new()
    }
}

impl ScramClient {
    /// 32 random bytes as 64 lowercase hex characters.
    pub fn new() -> Self {
        let bytes: [u8; 32] = rand::random();
        Self {
            first_nonce_hex: hex::encode(bytes),
        }
    }

    /// Fixed nonce, for tests only.
    pub fn with_nonce(first_nonce_hex: String) -> Self {
        Self { first_nonce_hex }
    }

    pub fn first_nonce(&self) -> &str {
        &self.first_nonce_hex
    }

    pub fn prove(
        &self,
        password: &str,
        salt_hex: &str,
        iterations: u32,
        server_nonce: &str,
    ) -> Result<ScramProof, RouterError> {
        if !server_nonce.starts_with(&self.first_nonce_hex) {
            return Err(RouterError::LoginAlgorithmMismatch);
        }
        let salt = hex::decode(salt_hex).map_err(|_| RouterError::LoginAlgorithmMismatch)?;
        let auth_message = format!("{},{},{}", self.first_nonce_hex, server_nonce, server_nonce);

        let mut salted = [0u8; 32];
        pbkdf2::pbkdf2_hmac::<Sha256>(password.as_bytes(), &salt, iterations, &mut salted);

        let client_key = hmac(b"Client Key", &salted);
        let stored_key = Sha256::digest(&client_key);
        let client_signature = hmac(auth_message.as_bytes(), &stored_key);
        let proof: Vec<u8> = client_key
            .iter()
            .zip(client_signature.iter())
            .map(|(a, b)| a ^ b)
            .collect();

        let server_key = hmac(b"Server Key", &salted);
        let server_signature = hmac(auth_message.as_bytes(), &server_key);

        Ok(ScramProof {
            client_proof_hex: hex::encode(proof),
            final_nonce: server_nonce.to_string(),
            expected_server_signature_hex: hex::encode(server_signature),
        })
    }
}

impl fmt::Debug for ScramClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ScramClient { .. }")
    }
}

pub struct ScramProof {
    pub client_proof_hex: String,
    pub final_nonce: String,
    expected_server_signature_hex: String,
}

impl ScramProof {
    /// Constant-time comparison; any mismatch is a hard `LoginAlgorithmMismatch`.
    pub fn verify_server(&self, server_signature_hex: &str) -> Result<(), RouterError> {
        let got = server_signature_hex.trim().to_ascii_lowercase();
        let ok: bool = got
            .as_bytes()
            .ct_eq(self.expected_server_signature_hex.as_bytes())
            .into();
        if ok {
            Ok(())
        } else {
            Err(RouterError::LoginAlgorithmMismatch)
        }
    }
}

impl fmt::Debug for ScramProof {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ScramProof { .. }")
    }
}
