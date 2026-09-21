use std::sync::mpsc::Sender;

use crate::algorithms::{Cipher, ErrorCorrectionCode, PipelineSettings, CompressionId, Compressor};
use crate::archiver::{crc32_of_artifact_and_rewind, Artifact, WalkedFile, ArchivedArtifactEntry};
use crate::error::AppError;

const IS_OPTIMIZE_ON: bool = true;


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

    let original_size = artifact.get_payload_size() as u64;
    let crc32 = crc32_of_artifact_and_rewind(&mut artifact, pipeline_settings)?;

    let cipher: Box<dyn Cipher> = pipeline_settings.cipher.get();
    let fec: Box<dyn ErrorCorrectionCode> = pipeline_settings.fec.get();

    let (compressed_artifact, effective_compressor_id) = compress_with_fallback(artifact, pipeline_settings.compression)?;
    let encoded_artifact = cipher.transform(compressed_artifact, encode_key)?;
    let cooked_artifact = fec.encode(encoded_artifact)?;

    let size_after_pipeline = cooked_artifact.get_payload_size() as u64;
    let entry = ArchivedArtifactEntry {
        relative_path: file.relative_path.clone(),
        original_size,
        size_after_pipeline,
        payload_offset: 0,
        pipeline: PipelineSettings {
            compression: effective_compressor_id,
            cipher: pipeline_settings.cipher,
            fec: pipeline_settings.fec,
        },
        crc32,
    };

    artifact_sender
        .send((entry, cooked_artifact))
        .map_err(|_| AppError::Compression("Очередь записи архива недоступна".into()))?;

    Ok(())
}


fn compress_with_fallback(
    artifact: Artifact,
    original_compressor_id: CompressionId,
) -> Result<(Artifact, CompressionId), AppError>
{
    if original_compressor_id == CompressionId::NoCompression {
        return Ok((artifact, original_compressor_id));
    }

    let compressor: Box<dyn Compressor> = original_compressor_id.get();
    let (compressed_artifact, effective_compressor_id) = compressor.compress(artifact)?;

    Ok((compressed_artifact, effective_compressor_id))
}
