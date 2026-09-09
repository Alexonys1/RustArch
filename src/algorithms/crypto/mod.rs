pub mod xor;

use crate::error::AppError;
use crate::archiver::Artifact;
use super::ids::CipherId;


pub trait Cipher {
    // TODO: encode, decode нужны? Не все же алгоритмы симметричны.
    fn transform(&self, artifact: Artifact, key: &[u8]) -> Result<Artifact, AppError>;
    fn validate_key(&self, key: &[u8]) -> Result<(), AppError>;
    fn id(&self) -> CipherId;
}


pub struct NoneCipher;


impl Cipher for NoneCipher {
    fn transform(&self, artifact: Artifact, _key: &[u8]) -> Result<Artifact, AppError> {
        Ok(artifact)
    }
    fn validate_key(&self, _key: &[u8]) -> Result<(), AppError> {
        Ok(())
    }
    fn id(&self) -> CipherId {
        CipherId::NoCipher
    }
}
