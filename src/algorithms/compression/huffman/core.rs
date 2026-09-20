use crate::error::AppError;
use crate::archiver::Artifact;
use crate::algorithms::compression::{CompressionId, Compressor};
use crate::algorithms::compression::huffman::bit_handlers::{BitWindow, StreamingBitWriter};
use crate::algorithms::compression::huffman::decode_table::{build_decode_table, DecodeEntry};
use super::tree_bulder::{build_tree, SymbolWithFreq, NodeOfHuffmanTree, HuffmanCode, create_huffman_codes};


/// Это размер небольшого буфера, в котором лежит кусочек сжатых данных.
/// При переполнении он сбрасывается в Artifact.
/// Буфер я поместил на стек, но прироста скорости это не придало, к сожалению.
pub const OUTPUT_FLUSH_SIZE: usize = 256 * 1024; // Оптимальное значение, найденное опытным путём

/// Размер таблицы табличного декодирования - 2^12 = 4096 записей. Любой
/// код длиной <= TABLE_BITS декодируется ОДНИМ обращением к таблице вместо
/// побитового спуска по дереву. Коды длиннее (крайне редкий случай) обрабатываются медленным
/// обходом дерева бит за битом.
/// Просто раньше была обычная реализация через простое дерево кодов.
/// Но обход-то дерева медленный. Поэтому для ускорения я написал плоское дерево + запасной вариант.
pub const TABLE_BITS: u32 = 12; // На тестах Battlefield получился оптимальный диапазон 11-13 бит. Дальше результаты сильно хуже. Лучше 12 бит.
pub const TABLE_SIZE: usize = 1 << TABLE_BITS; // 2^TABLE_BITS // Просто возведение в степень в Rust не очень удобное
pub const ALPHABET_SIZE: usize = 256; // 2^8


pub struct HuffmanCompressor;

impl Compressor for HuffmanCompressor {
    fn compress(&self, mut artifact: Artifact) -> Result<Artifact, AppError> {
        // ======================== СОБИРАЕМ ЧАСТОТЫ =============================
        let mut freqs = [0_u64; ALPHABET_SIZE]; // Индекс и есть сам символ (чтобы не забыть)
        let mut original_size: u64 = 0;

        while let Some(chunk) = artifact.next_chunk()? {
            for &b in chunk.iter() {
                freqs[b as usize] += 1;
            }
            original_size += chunk.len() as u64;
        }
        artifact.rewind_reading();
        // ======================== СОБИРАЕМ ЧАСТОТЫ =============================


        // =================== ЗАПИСЫВАЕМ В ЦЕЛЕВОЙ АРТЕФАКТ ТАБЛИЦУ ЧАСТОТ ========================
        let mut output_artifact = Artifact::new_with_temp_file_suffix(&artifact, "compressed");
        output_artifact.write_chunk_from(&original_size.to_le_bytes())?;

        let nonzero_freqs: Vec<SymbolWithFreq> = freqs
            .into_iter()
            .enumerate()
            .filter(|&(_, f)| f > 0)
            .map(|(symbol, f)| SymbolWithFreq { symbol: symbol as u8, freq: f })
            .collect();

        output_artifact.write_chunk_from(&(nonzero_freqs.len() as u16).to_le_bytes())?; // TODO: Было u32
        for &SymbolWithFreq { symbol, freq } in nonzero_freqs.iter() {
            output_artifact.write_chunk_from(&[symbol])?;
            output_artifact.write_chunk_from(&freq.to_le_bytes())?;
        }

        // Учитываем граничный случай. Просто раньше алгоритм из-за этого ложился :D
        if original_size == 0 || nonzero_freqs.len() <= 1 {
            return Ok(output_artifact);
        }
        // =================== ЗАПИСЫВАЕМ В ЦЕЛЕВОЙ АРТЕФАКТ ТАБЛИЦУ ЧАСТОТ ========================


        // ======================== СТРОИМ ДЕРЕВО ПО ТАБЛИЦЕ ЧАСТОТ =============================
        let tree: NodeOfHuffmanTree = build_tree(&nonzero_freqs);
        let codes: [HuffmanCode; ALPHABET_SIZE] = create_huffman_codes(tree);

        // Если файл раздувается при сжатии, то мы его и не будем сжимать! (ПРИДУМАЛ КОЛЯ)
        let mut new_artifact_size: usize = nonzero_freqs.len() * size_of::<SymbolWithFreq>() * 8;
        for SymbolWithFreq { symbol, freq } in nonzero_freqs.into_iter() {
            new_artifact_size += codes[symbol as usize].len as usize * freq as usize;
        }

        if ((new_artifact_size as f64) / 8.0).ceil() >= original_size as f64 {
            return Ok(artifact);
        }
        // ======================== СТРОИМ ДЕРЕВО ПО ТАБЛИЦЕ ЧАСТОТ =============================


        // ==================== ЧИТАЕМ АРТЕФАКТ ПОВТОРНО И КОДИРУЕМ ЕГО ============================
        let mut writer = StreamingBitWriter::new(&mut output_artifact);

        while let Some(chunk) = artifact.next_chunk()? {
            for &symbol in chunk.iter() {
                let HuffmanCode { code, len } = codes[symbol as usize];
                writer.push_code(code, len)?;
            }
        }

        writer.finish()?;

        Ok(output_artifact)
        // ==================== ЧИТАЕМ АРТЕФАКТ ПОВТОРНО И КОДИРУЕМ ЕГО ============================
    }


    fn decompress(&self, mut artifact: Artifact) -> Result<Artifact, AppError> {
        // ============================ ЧИТАЕМ ЗАГОЛОВОК ФАЙЛА ==========================
        let mut output = Artifact::new_with_temp_file_suffix(&artifact, "decompressed");

        let original_size: u64 = artifact.read_le_u64()?;
        let distinct_count: usize = artifact.read_le_u16()? as usize;

        let mut nonzero_freqs: Vec<SymbolWithFreq> = Vec::with_capacity(distinct_count);
        for _ in 0..distinct_count {
            let symbol: u8 = artifact.read_le_u8()?;
            let freq: u64 = artifact.read_le_u64()?;
            nonzero_freqs.push(SymbolWithFreq { symbol, freq });
        }

        if original_size == 0 {
            return Ok(output);
        }

        if nonzero_freqs.is_empty() {
            return Err(AppError::CorruptArchive(
                "Huffman: Пустая частотная таблица при ненулевом original_size!".into(),
            ));
        }

        if nonzero_freqs.len() == 1 {
            let symbol: u8 = nonzero_freqs[0].symbol;
            let block_len = output.chunk_size.get().min(original_size as usize).max(1);
            let block = vec![symbol; block_len];
            let mut remaining = original_size;

            while remaining > 0 {
                let take = remaining.min(block_len as u64) as usize;
                output.write_chunk_from(&block[..take])?;
                remaining -= take as u64;
            }

            return Ok(output);
        }
        // ============================ ЧИТАЕМ ЗАГОЛОВОК ФАЙЛА ==========================


        // ======== СТРОИМ ДЕРЕВО ПО ПОЛУЧЕННОМУ ЗАГОЛОВКУ И ТАБЛИЦЕ ЧАСТОТ ============
        let tree: NodeOfHuffmanTree = build_tree(&nonzero_freqs);
        let decode_table: [DecodeEntry; TABLE_SIZE] = build_decode_table(&tree);
        // ======== СТРОИМ ДЕРЕВО ПО ПОЛУЧЕННОМУ ЗАГОЛОВКУ И ТАБЛИЦЕ ЧАСТОТ ============


        // ================= ДЕКОДИРУЕМ ИСХОДНЫЙ ФАЙЛ =====================
        let mut bits = BitWindow::new(&mut artifact);
        let mut decoded_count: u64 = 0;
        let mut out_buf: Vec<u8> = Vec::with_capacity(OUTPUT_FLUSH_SIZE);

        while decoded_count < original_size {
            bits.fill(TABLE_BITS)?;
            let index = bits.peek(TABLE_BITS) as usize;

            match decode_table[index] {
                DecodeEntry::Symbol { symbol, code_len } => {
                    bits.consume(code_len as u32);
                    out_buf.push(symbol);
                    decoded_count += 1;
                }

                DecodeEntry::Escape => {
                    let mut node = &tree;
                    loop {
                        match node {
                            NodeOfHuffmanTree::Leaf { symbol, .. } => {
                                out_buf.push(*symbol);
                                decoded_count += 1;
                                break;
                            }
                            NodeOfHuffmanTree::Node { left, right, .. } => {
                                bits.fill(1)?;
                                let bit = bits.peek(1) != 0;
                                bits.consume(1);
                                node = if bit { right } else { left };
                            }
                        }
                    }
                }
            }

            if out_buf.len() >= OUTPUT_FLUSH_SIZE {
                output.write_chunk_from(&out_buf)?;
                out_buf.clear();
            }
        }

        if !out_buf.is_empty() {
            output.write_chunk_from(&out_buf)?;
        }

        Ok(output)
    }

    fn id(&self) -> CompressionId {
        CompressionId::Huffman
    }
}
