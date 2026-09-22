use std::sync::mpsc::Sender;

use crate::algorithms::{Cipher, Compressor, ErrorCorrectionCode, PipelineSettings};
use crate::archiver::{ArchivedArtifactEntry, Artifact, WalkedFile, crc32_of_artifact_and_rewind};
use crate::error::AppError;


/// compress -> encrypt -> fec-encode
/// Алгоритмы могут на своё усмотрение сжимать или не сжимать артефакты.
/// Хорошо, что мы с Колей решили переписать трейт Compressor.
/// Просто до этого у нас была настоящая проблема с хранением заголовков в двух местах сразу.
/// Хорошо, что мы наконец-то её решили элегантным способом!
pub fn pack_file(
    file: &WalkedFile,
    pipeline_settings: PipelineSettings,
    encode_key: &[u8],
    artifact_sender: Sender<(ArchivedArtifactEntry, Artifact)>,
) -> Result<(), AppError>
{
    let mut artifact = Artifact::from_file(file.absolute_path.as_ref())?;

    let original_size: u64 = artifact.get_payload_size() as u64;
    let crc32: u32 = crc32_of_artifact_and_rewind(&mut artifact, pipeline_settings)?;

    let compressor: Box<dyn Compressor> = pipeline_settings.compression.get();
    let cipher: Box<dyn Cipher> = pipeline_settings.cipher.get();
    let fec: Box<dyn ErrorCorrectionCode> = pipeline_settings.fec.get();

    let (compressed_artifact, effective_compression) = compressor.compress(artifact)?;
    let encoded_artifact = cipher.transform(compressed_artifact, encode_key)?;
    let cooked_artifact = fec.encode(encoded_artifact)?;

    let stored_size: u64 = cooked_artifact.get_payload_size() as u64;
    let effective_pipeline = PipelineSettings {
        compression: effective_compression,
        ..pipeline_settings
    };
    let entry = ArchivedArtifactEntry {
        relative_path: file.relative_path.clone(),
        original_size,
        stored_size,
        payload_offset: 0,
        pipeline: effective_pipeline,
        crc32,
    };

    artifact_sender
        .send((entry, cooked_artifact))
        .map_err(|_| AppError::Compression("Очередь записи архива недоступна".into()))?;

    Ok(())
}
