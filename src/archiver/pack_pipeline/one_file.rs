use std::path::Path;
use std::sync::mpsc::Sender;

use crate::algorithms::{Cipher, ErrorCorrectionCode, PipelineSettings, CompressionId};
use crate::archiver::{crc32_of_artifact_and_rewind, Artifact, WalkedFile, ArchivedArtifactEntry};
use crate::error::AppError;

const IS_OPTIMIZE_ON: bool = false; // TODO:  Исправить баг!!!


/// compress -> encrypt -> fec-encode
/// Если файл после сжатия больше исходного, оставляем исходный и
/// проходим оставшиеся преобразования (encrypt -> fec).
pub fn pack_file(
    file: &WalkedFile,
    pipeline_settings: PipelineSettings,
    encode_key: &[u8],
    artifact_sender: Sender<(ArchivedArtifactEntry, Artifact)>,
) -> Result<ArchivedArtifactEntry, AppError>
{
    let mut artifact = Artifact::from_file(file.absolute_path.as_ref())?;

    let original_size = artifact.payload_size() as u64;
    let crc32 = crc32_of_artifact_and_rewind(&mut artifact, pipeline_settings)?;

    let cipher: Box<dyn Cipher> = pipeline_settings.cipher.get();
    let fec: Box<dyn ErrorCorrectionCode> = pipeline_settings.fec.get();

    let (compressed_artifact, effective_compressor_id) = compress_with_fallback(
        artifact,
        pipeline_settings.compression,
        file.absolute_path.as_ref(),
        original_size,
    )?;
    let encoded_artifact = cipher.transform(compressed_artifact, encode_key)?;
    let cooked_artifact = fec.encode(encoded_artifact)?;

    let size_after_pipeline = cooked_artifact.payload_size() as u64;
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
        .send((entry.clone(), cooked_artifact))
        .map_err(|_| AppError::Compression("Очередь записи архива недоступна".into()))?;

    Ok(entry)
}


fn compress_with_fallback(
    artifact: Artifact,
    original_compressor_id: CompressionId,
    original_path: &Path,
    original_size: u64,
) -> Result<(Artifact, CompressionId), AppError>
{
    if original_compressor_id == CompressionId::NoCompression {
        return Ok((artifact, original_compressor_id));
    }

    let compressor = original_compressor_id.get();
    let compressed_artifact = compressor.compress(artifact)?;

    if (compressed_artifact.payload_size() as u64) < original_size || !IS_OPTIMIZE_ON {
        Ok((compressed_artifact, original_compressor_id))
    } else {
        drop(compressed_artifact);
        let raw_artifact = Artifact::from_file(original_path)
            .map_err(|e| AppError::Compression(format!(
                "compress_with_fallback: Не удалось повторно открыть '{}' для отката на NoCompressor: {e}",
                original_path.display()
            )))?;
        Ok((raw_artifact, CompressionId::NoCompression))
    }
}
