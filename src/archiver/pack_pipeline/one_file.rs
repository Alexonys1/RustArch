use crate::algorithms::{Cipher, Compressor, ErrorCorrectionCode, PipelineSettings};
use crate::archiver::{crc32_of_artifact_and_rewind, Artifact, ArtifactAfterPipeline, WalkedFile};
use crate::error::AppError;


/// compress -> encrypt -> fec-encode
pub fn pack_file(file: &WalkedFile, pipeline_settings: PipelineSettings, encode_key: &[u8]) -> Result<ArtifactAfterPipeline, AppError> {
    let mut artifact: Artifact = Artifact::from_file(&file.absolute_path)?;

    let original_size: u64 = artifact.payload_size() as u64;
    let crc32: u32 = crc32_of_artifact_and_rewind(&mut artifact)?;

    let compressor: Box<dyn Compressor> = pipeline_settings.compression.get();
    let cipher: Box<dyn Cipher> = pipeline_settings.cipher.get();
    let fec: Box<dyn ErrorCorrectionCode> = pipeline_settings.fec.get();

    let compressed_artifact: Artifact = compressor.compress(artifact)?;
    let encoded_artifact: Artifact = cipher.transform(compressed_artifact, encode_key)?;
    let cooked_artifact: Artifact = fec.encode(encoded_artifact)?;

    let size_after_pipeline: u64 = cooked_artifact.payload_size() as u64;

    Ok(ArtifactAfterPipeline {
        relative_path: file.relative_path.clone(),
        original_size,
        size_after_pipeline,
        pipeline: pipeline_settings,
        crc32,
        payload: cooked_artifact,
    })
}
