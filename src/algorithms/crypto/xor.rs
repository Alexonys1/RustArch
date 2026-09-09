use crate::error::AppError;
use crate::archiver::Artifact;
use super::{Cipher, CipherId};


pub struct XorCipher;


impl Cipher for XorCipher {
    fn transform(&self, mut artifact: Artifact, key: &[u8]) -> Result<Artifact, AppError> {
        self.validate_key(key)?;

        let mut output = Artifact::new_with_temp_file_suffix(&artifact, "encoded");
        let mut key_pos: usize = 0;

        while let Some(mut chunk) = artifact.read_next_chunk()? {
            for byte in chunk.iter_mut() {
                *byte ^= key[key_pos];
                key_pos += 1;
                if key_pos == key.len() {
                    key_pos = 0;
                }
            }
            output.write_chunk(&chunk)?;
        }

        Ok(output)
    }

    fn validate_key(&self, key: &[u8]) -> Result<(), AppError> {
        if key.is_empty() {
            return Err(AppError::Crypto("XOR: Ключ не может быть пустым!".to_string()));
        }
        Ok(())
    }

    fn id(&self) -> CipherId {
        CipherId::Xor
    }
}
