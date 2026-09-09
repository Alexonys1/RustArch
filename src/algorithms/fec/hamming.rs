use crate::error::AppError;
use crate::archiver::Artifact;
use super::FecId;

use super::{ErrorCorrectionCode, FecReport};


pub struct HammingCode {
    pub data_bits: u32,
    pub parity_bits: u32,
    variant: FecId,
}


impl HammingCode {
    pub fn new_7_4() -> Self {
        Self { data_bits: 4, parity_bits: 3, variant: FecId::Hamming7_4 }
    }
    pub fn new_15_11() -> Self {
        Self { data_bits: 11, parity_bits: 4, variant: FecId::Hamming15_11 }
    }
}


impl ErrorCorrectionCode for HammingCode {
    fn encode(&self, _artifact: Artifact) -> Result<Artifact, AppError> {
        Err(AppError::NotImplemented("Hamming::encode"))
    }
    fn decode(&self, _artifact: Artifact) -> Result<(Artifact, FecReport), AppError> {
        Err(AppError::NotImplemented("Hamming::decode"))
    }
    fn id(&self) -> FecId {
        self.variant
    }
}
