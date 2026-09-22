mod reed_solomon;

pub use reed_solomon::ReedSolomonCode;

use crate::error::AppError;
use crate::archiver::Artifact;
use super::FecId;


#[derive(Debug, Clone, Copy, Default)]
pub struct FecReport {
    pub blocks_processed: u64,
    pub blocks_corrected: u64,
    pub blocks_uncorrectable: u64,
}


pub trait ErrorCorrectionCode {
    fn encode(&self, artifact: Artifact) -> Result<Artifact, AppError>;
    fn decode(&self, artifact: Artifact) -> Result<(Artifact, FecReport), AppError>;
    fn id(&self) -> FecId;
}


pub struct NoneFec;


impl ErrorCorrectionCode for NoneFec {
    fn encode(&self, artifact: Artifact) -> Result<Artifact, AppError> {
        Ok(artifact)
    }
    fn decode(&self, artifact: Artifact) -> Result<(Artifact, FecReport), AppError> {
        Ok((artifact, FecReport::default()))
    }
    fn id(&self) -> FecId {
        FecId::NoFec
    }
}
