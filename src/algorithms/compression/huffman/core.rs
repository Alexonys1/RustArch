use crate::error::AppError;
use crate::archiver::Artifact;
use crate::algorithms::compression::{CompressionId, Compressor};
use crate::algorithms::compression::huffman::bit_handlers::{BitWindow, StreamingBitWriter};
use crate::algorithms::compression::huffman::decode_table::{build_decode_table, DecodeEntry};
use super::tree_bulder::{SymbolWithFreq, NodeOfHuffmanTree, HuffmanCode, create_huffman_codes, build_tree};


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
    fn compress(&self, artifact: Artifact) -> Result<(Artifact, CompressionId), AppError> {
        self.compress_impl(artifact, true) // Роберт Мартин был бы в бешенстве от передачи флагов в функции xD
    }

    fn decompress(&self, mut artifact: Artifact) -> Result<Artifact, AppError> {
        // ============================ ЧИТАЕМ ЗАГОЛОВОК ФАЙЛА ==========================
        let mut output = Artifact::new_with_temp_file_suffix(&artifact, "decompressed");

        let original_size: u64 = artifact.read_le_u64()?;
        let distinct_count: usize = artifact.read_le_u16()? as usize;
        if distinct_count > ALPHABET_SIZE {
            return Err(AppError::CorruptArchive(
                "Huffman: Число символов превышает размер алфавита!".into(),
            ));
        }

        let mut nonzero_freqs: Vec<SymbolWithFreq> = Vec::with_capacity(distinct_count);
        let mut seen = [false; ALPHABET_SIZE];
        let mut frequency_sum = 0u64;

        for _ in 0..distinct_count {
            let symbol: u8 = artifact.read_le_u8()?;
            let freq: u64 = artifact.read_le_u64()?;

            if freq == 0 || seen[symbol as usize] {
                return Err(AppError::CorruptArchive(
                    "Huffman: Некорректная таблица частот!".into(),
                ));
            }

            seen[symbol as usize] = true;
            frequency_sum = frequency_sum.checked_add(freq).ok_or_else(|| {
                AppError::CorruptArchive("Huffman: Переполнение суммы частот!".into())
            })?;
            nonzero_freqs.push(SymbolWithFreq { symbol, freq });
        }

        if frequency_sum != original_size {
            return Err(AppError::CorruptArchive(
                "Huffman: Сумма частот не совпадает с original_size".into(),
            ));
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
            let block_len = original_size
                .min(output.chunk_size.get() as u64)
                .max(1) as usize;
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


impl HuffmanCompressor {
    pub fn compress_always(&self, artifact: Artifact) -> Result<Artifact, AppError> {
        Ok(self.compress_impl(artifact, false)?.0)
    }

    fn compress_impl(&self, mut artifact: Artifact, allow_passthrough: bool) -> Result<(Artifact, CompressionId), AppError> {
        let mut freqs = [0_u64; ALPHABET_SIZE]; // Индекс и есть сам символ (чтобы не забыть)
        let mut original_size: u64 = 0;

        while let Some(chunk) = artifact.next_chunk()? {
            for &b in chunk.iter() {
                freqs[b as usize] += 1;
            }
            original_size += chunk.len() as u64;
        }
        artifact.rewind_reading();

        let nonzero_freqs: Vec<SymbolWithFreq> = freqs
            .into_iter()
            .enumerate()
            .filter(|&(_, f)| f > 0)
            .map(|(symbol, f)| SymbolWithFreq { symbol: symbol as u8, freq: f })
            .collect();

        // Раньше без проверок на пустой файл архиватор просто ложился :D
        if original_size == 0 || nonzero_freqs.len() <= 1 {
            let mut output = Artifact::new_with_temp_file_suffix(&artifact, "compressed");
            write_header(&mut output, original_size, &nonzero_freqs)?;
            return Ok((output, CompressionId::Huffman));
        }

        let tree: NodeOfHuffmanTree = build_tree(&nonzero_freqs);
        let codes: [HuffmanCode; ALPHABET_SIZE] = create_huffman_codes(tree);

        let header_bytes: u64 = 8 + 2 + nonzero_freqs.len() as u64 * 9;
        let encoded_bits: u64 = nonzero_freqs
            .iter()
            .map(|entry| codes[entry.symbol as usize].len as u64 * entry.freq)
            .sum();
        let encoded_size = header_bytes + encoded_bits.div_ceil(8);

        if allow_passthrough && encoded_size >= original_size {
            return Ok((artifact, CompressionId::NoCompression));
        }

        let mut output_artifact = Artifact::new_with_temp_file_suffix(&artifact, "compressed");
        write_header(&mut output_artifact, original_size, &nonzero_freqs)?;

        let mut writer = StreamingBitWriter::new(&mut output_artifact);
        while let Some(chunk) = artifact.next_chunk()? {
            for &symbol in chunk.iter() {
                let HuffmanCode { code, len } = codes[symbol as usize];
                writer.push_code(code, len)?;
            }
        }
        writer.finish()?;

        Ok((output_artifact, CompressionId::Huffman))
    }
}


fn write_header(output: &mut Artifact, original_size: u64, nonzero_freqs: &[SymbolWithFreq]) -> Result<(), AppError> {
    output.write_chunk_from(&original_size.to_le_bytes())?;
    output.write_chunk_from(&(nonzero_freqs.len() as u16).to_le_bytes())?;

    for &SymbolWithFreq { symbol, freq } in nonzero_freqs {
        output.write_chunk_from(&[symbol])?;
        output.write_chunk_from(&freq.to_le_bytes())?;
    }

    Ok(())
}
