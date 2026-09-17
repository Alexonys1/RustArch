use std::borrow::Cow;
use std::slice;

use crate::error::AppError;
use crate::archiver::Artifact;
use super::{Cipher, CipherId};


pub struct XorCipher;


impl Cipher for XorCipher {
    fn transform(&self, mut artifact: Artifact, key: &[u8]) -> Result<Artifact, AppError> {
        self.validate_key(key)?;

        if artifact.on_disk() { // Вот эта оптимизация выигрывает ~100мс, что немного. Зря делал.
            let mut output = Artifact::new_with_temp_file_suffix(&artifact, "encoded");
            let mut key_pos: usize = 0;

            while let Some(chunk) = artifact.next_chunk()? {
                let mut chunk = chunk.to_vec(); // Здесь в любом случае Borrowed

                for byte in chunk.iter_mut() {
                    *byte ^= key[key_pos];
                    key_pos += 1;

                    if key_pos == key.len() {
                        key_pos = 0;
                    }
                }
                output.write_chunk_from(&chunk)?;
            }

            Ok(output)
        }
        else {
            artifact.rewind_reading();
            let mut key_pos = 0;

            while let Some(chunk) = artifact.next_chunk()? {
                let chunk: &mut [u8] = unsafe { cow_borrowed_to_mut(chunk) };

                for byte in chunk.iter_mut() {
                    *byte ^= key[key_pos];
                    key_pos += 1;

                    if key_pos == key.len() {
                        key_pos = 0;
                    }
                }
            }

            artifact.rewind_reading();
            Ok(artifact)
        }
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


// Первая небезопасная функция! Надеюсь, здесь не будет UB.
// Если что, просто удалю эту функцию и буду копировать данные. :D
/// Возвращает мутабельный срез на данные Cow, хотя безопасно этого сделать нельзя.
unsafe fn cow_borrowed_to_mut(cow: Cow<'_, [u8]>) -> &mut [u8] {
    match cow {
        Cow::Borrowed(s) => {
            let len = s.len();
            let ptr = s.as_ptr() as *mut u8;
            unsafe { slice::from_raw_parts_mut(ptr, len) }
        }
        Cow::Owned(_) => unreachable!(),
    }
}
