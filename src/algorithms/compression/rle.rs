use crate::error::AppError;
use crate::archiver::Artifact;
use super::{CompressionId, Compressor};


pub struct RleCompressor;


impl Compressor for RleCompressor {
    fn compress(&self, mut artifact: Artifact) -> Result<Artifact, AppError> {
        let mut output = Artifact::new_with_temp_file_suffix(&artifact, "compressed");
        let mut current_byte: Option<u8> = None;
        let mut current_count: u32 = 0;
        let mut out_buf: Vec<u8> = Vec::with_capacity(8192);

        fn flush_run(byte: u8, mut count: u32, out: &mut Vec<u8>) {
            while count > 0 {
                let take = count.min(255) as u8;
                out.push(take);
                out.push(byte);
                count -= take as u32;
            }
        }

        while let Some(chunk) = artifact.read_next_chunk()? {
            for &b in &chunk {
                match current_byte {
                    Some(cb) if cb == b && current_count < 255 => current_count += 1,
                    Some(cb) => {
                        flush_run(cb, current_count, &mut out_buf);
                        current_byte = Some(b);
                        current_count = 1;
                    }
                    None => {
                        current_byte = Some(b);
                        current_count = 1;
                    }
                }
            }
            if !out_buf.is_empty() {
                output.write_chunk(&out_buf)?;
                out_buf.clear();
            }
        }

        if let Some(cb) = current_byte {
            flush_run(cb, current_count, &mut out_buf);
            output.write_chunk(&out_buf)?;
        }

        Ok(output)
    }

    fn decompress(&self, mut artifact: Artifact) -> Result<Artifact, AppError> {
        let mut output = Artifact::new_with_temp_file_suffix(&artifact, "decompressed");
        let mut pending_count: Option<u8> = None;
        let mut out_buf: Vec<u8> = Vec::with_capacity(8192);

        while let Some(chunk) = artifact.read_next_chunk()? {
            for &b in &chunk {
                match pending_count.take() {
                    None => pending_count = Some(b),
                    Some(count) => out_buf.resize(out_buf.len() + count as usize, b),
                }
            }
            if !out_buf.is_empty() {
                output.write_chunk(&out_buf)?;
                out_buf.clear();
            }
        }

        if pending_count.is_some() {
            return Err(AppError::Compression(
                "RLE: Поток оборван на середине пары (count, value)!".to_string(),
            ));
        }

        Ok(output)
    }

    fn id(&self) -> CompressionId {
        CompressionId::Rle
    }
}
