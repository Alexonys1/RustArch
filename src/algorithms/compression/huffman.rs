use std::cmp::Ordering;
use std::collections::BinaryHeap;

use crate::error::AppError;
use crate::archiver::Artifact;
use super::{CompressionId, Compressor};
use super::utils::BufferedArtifactReader;


/// Размер алфавита - все возможные значения одного байта.
const ALPHABET_SIZE: usize = 256;

/// Порог сброса накопленного битового буфера в Artifact::write_chunk.
/// Ограничивает буфер константой независимо от размера файла - раньше
/// весь сжатый поток целиком копился в памяти до единственной записи
/// в конце (`output.write_chunk(&writer.finish())`), это и было причиной
/// расхода памяти сверх заданного бюджета: этот буфер существовал в куче
/// ДО того, как хоть один байт попадал в Artifact и мог быть учтён его
/// внутренним BudgetGuard.
const OUTPUT_FLUSH_SIZE: usize = 256 * 1024;


/// Узел дерева Хаффмана. Дерево строится и в компрессоре, и в декомпрессоре
/// заново из одной и той же частотной таблицы, поэтому важно, чтобы
/// алгоритм построения был полностью детерминированным (см. `build_tree`).
enum HuffmanNode {
    Leaf { symbol: u8, freq: u64 },
    Internal { freq: u64, left: Box<HuffmanNode>, right: Box<HuffmanNode> },
}

impl HuffmanNode {
    fn freq(&self) -> u64 {
        match self {
            HuffmanNode::Leaf { freq, .. } => *freq,
            HuffmanNode::Internal { freq, .. } => *freq,
        }
    }
}


struct HeapEntry {
    node: HuffmanNode,
    seq: u64,
}

impl PartialEq for HeapEntry {
    fn eq(&self, other: &Self) -> bool {
        self.node.freq() == other.node.freq() && self.seq == other.seq
    }
}

impl Eq for HeapEntry {}

impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        other.node.freq().cmp(&self.node.freq())
            .then_with(|| other.seq.cmp(&self.seq))
    }
}


/// Строит дерево Хаффмана по списку (символ, частота). Список ДОЛЖЕН
/// содержать минимум 2 записи - вырожденный случай одного символа
/// обрабатывается отдельно на уровне compress/decompress.
fn build_tree(distinct: &[(u8, u64)]) -> HuffmanNode {
    let mut heap: BinaryHeap<HeapEntry> = BinaryHeap::with_capacity(distinct.len());
    let mut seq: u64 = 0;

    for &(symbol, freq) in distinct {
        heap.push(HeapEntry { node: HuffmanNode::Leaf { symbol, freq }, seq });
        seq += 1;
    }

    while heap.len() > 1 {
        let a = heap.pop().unwrap();
        let b = heap.pop().unwrap();

        let merged = HuffmanNode::Internal {
            freq: a.node.freq() + b.node.freq(),
            left: Box::new(a.node),
            right: Box::new(b.node),
        };

        heap.push(HeapEntry { node: merged, seq });
        seq += 1;
    }

    heap.pop().expect("build_tree вызван с пустым списком символов").node
}


fn assign_codes(node: &HuffmanNode, prefix: &mut Vec<bool>, codes: &mut [Option<Vec<bool>>]) {
    match node {
        HuffmanNode::Leaf { symbol, .. } => {
            codes[*symbol as usize] = Some(prefix.clone());
        }
        HuffmanNode::Internal { left, right, .. } => {
            prefix.push(false);
            assign_codes(left, prefix, codes);
            prefix.pop();

            prefix.push(true);
            assign_codes(right, prefix, codes);
            prefix.pop();
        }
    }
}


/// Побитовый писатель, который сбрасывает накопленные байты в `Artifact`
/// по мере заполнения буфера, а не хранит весь сжатый поток в памяти до
/// самого конца. Держит `&mut Artifact`, поэтому его нельзя использовать
/// одновременно с чем-либо ещё, что пишет в тот же артефакт - это и не
/// нужно, он владеет выводом на всё время кодирования.
struct StreamingBitWriter<'a> {
    output: &'a mut Artifact,
    buf: Vec<u8>,
    cur: u8,
    nbits: u8,
}

impl<'a> StreamingBitWriter<'a> {
    fn new(output: &'a mut Artifact) -> Self {
        Self {
            output,
            buf: Vec::with_capacity(OUTPUT_FLUSH_SIZE),
            cur: 0,
            nbits: 0,
        }
    }

    fn push_bit(&mut self, bit: bool) -> Result<(), AppError> {
        self.cur = (self.cur << 1) | (bit as u8);
        self.nbits += 1;
        if self.nbits == 8 {
            self.buf.push(self.cur);
            self.cur = 0;
            self.nbits = 0;
            if self.buf.len() >= OUTPUT_FLUSH_SIZE {
                self.output.write_chunk(&self.buf)?;
                self.buf.clear();
            }
        }
        Ok(())
    }

    fn push_bits(&mut self, bits: &[bool]) -> Result<(), AppError> {
        for &b in bits {
            self.push_bit(b)?;
        }
        Ok(())
    }

    /// Дописывает неполный последний байт (нулями справа) и сбрасывает
    /// остаток буфера. Потребляет `self`, чтобы нельзя было случайно
    /// продолжить писать после финализации.
    fn finish(mut self) -> Result<(), AppError> {
        if self.nbits > 0 {
            self.cur <<= 8 - self.nbits;
            self.buf.push(self.cur);
        }
        if !self.buf.is_empty() {
            self.output.write_chunk(&self.buf)?;
        }
        Ok(())
    }
}


/// Побитовый читатель, симметричный `StreamingBitWriter`: дочитывает
/// байты из артефакта по мере надобности через `BufferedArtifactReader`
/// (тот же буферизованный хелпер, что и в LZSS/LZ77 - избегает и
/// накопления всего потока в памяти, и лишних seek()+read() на каждый байт).
struct StreamingBitReader<'a> {
    reader: BufferedArtifactReader<'a>,
    cur_byte: u8,
    bit_pos: u8, // 8 == "текущий байт исчерпан, нужен новый"
}

impl<'a> StreamingBitReader<'a> {
    fn new(reader: BufferedArtifactReader<'a>) -> Self {
        Self { reader, cur_byte: 0, bit_pos: 8 }
    }

    fn read_bit(&mut self) -> Result<bool, AppError> {
        if self.bit_pos == 8 {
            self.cur_byte = self.reader.read_u8()?; // CorruptArchive при обрыве потока
            self.bit_pos = 0;
        }
        let bit = (self.cur_byte >> (7 - self.bit_pos)) & 1;
        self.bit_pos += 1;
        Ok(bit != 0)
    }
}


fn read_exact_from_artifact(artifact: &mut Artifact, n: usize) -> Result<Vec<u8>, AppError> {
    let mut buf = vec![0u8; n];
    let mut filled = 0;

    while filled < n {
        let read = artifact.read_chunk(&mut buf[filled..])?;
        if read == 0 {
            return Err(AppError::CorruptArchive(
                "Huffman: неожиданный конец потока при чтении заголовка".to_string(),
            ));
        }
        filled += read;
    }

    Ok(buf)
}


pub struct HuffmanCompressor;


impl Compressor for HuffmanCompressor { // TODO: А мы можем загрузить весь файл в память, чтобы ускорить его обработку?
    fn compress(&self, mut artifact: Artifact) -> Result<Artifact, AppError> {
        // --- Первый проход: считаем частоты байт и общий размер данных ---
        // Уже был потоковым и остаётся таким - freqs занимает фиксированные
        // 256*8 = 2 КБ независимо от размера файла.
        let mut freqs = [0u64; ALPHABET_SIZE];
        let mut original_size: u64 = 0;

        while let Some(chunk) = artifact.read_next_chunk()? {
            for &b in &chunk {
                freqs[b as usize] += 1;
            }
            original_size += chunk.len() as u64;
        }
        artifact.rewind_reading();

        let mut output = Artifact::new_with_temp_file_suffix(&artifact, "compressed");

        output.write_chunk(&original_size.to_le_bytes())?;

        let distinct: Vec<(u8, u64)> = freqs
            .iter()
            .enumerate()
            .filter(|&(_, &f)| f > 0)
            .map(|(symbol, &f)| (symbol as u8, f))
            .collect();

        output.write_chunk(&(distinct.len() as u32).to_le_bytes())?;
        for &(symbol, freq) in &distinct {
            output.write_chunk(&[symbol])?;
            output.write_chunk(&freq.to_le_bytes())?;
        }

        if original_size == 0 || distinct.len() <= 1 {
            return Ok(output);
        }

        let tree = build_tree(&distinct);
        let mut codes: Vec<Option<Vec<bool>>> = vec![None; ALPHABET_SIZE];
        let mut prefix = Vec::new();
        assign_codes(&tree, &mut prefix, &mut codes);

        // --- Второй проход: кодируем данные, сбрасывая биты по мере
        // накопления, а не храня весь сжатый поток в памяти до конца. ---
        let mut writer = StreamingBitWriter::new(&mut output);
        while let Some(chunk) = artifact.read_next_chunk()? {
            for &b in &chunk {
                let code = codes[b as usize]
                    .as_ref()
                    .expect("символ отсутствует в построенной таблице кодов - баг подсчёта частот");
                writer.push_bits(code)?;
            }
        }
        writer.finish()?;

        Ok(output)
    }

    fn decompress(&self, mut artifact: Artifact) -> Result<Artifact, AppError> {
        let mut output = Artifact::new_with_temp_file_suffix(&artifact, "decompressed");

        let original_size = u64::from_le_bytes(
            read_exact_from_artifact(&mut artifact, 8)?.try_into().unwrap(),
        );
        let distinct_count = u32::from_le_bytes(
            read_exact_from_artifact(&mut artifact, 4)?.try_into().unwrap(),
        ) as usize;

        let mut distinct: Vec<(u8, u64)> = Vec::with_capacity(distinct_count);
        for _ in 0..distinct_count {
            let symbol = read_exact_from_artifact(&mut artifact, 1)?[0];
            let freq = u64::from_le_bytes(
                read_exact_from_artifact(&mut artifact, 8)?.try_into().unwrap(),
            );
            distinct.push((symbol, freq));
        }

        if original_size == 0 {
            return Ok(output);
        }

        if distinct.is_empty() {
            return Err(AppError::CorruptArchive(
                "Huffman: пустая частотная таблица при ненулевом original_size".to_string(),
            ));
        }

        if distinct.len() == 1 {
            let (symbol, _freq) = distinct[0];
            let block_len = output.chunk_size.get().min(original_size as usize).max(1);
            let block = vec![symbol; block_len];

            let mut remaining = original_size;
            while remaining > 0 {
                let take = remaining.min(block_len as u64) as usize;
                output.write_chunk(&block[..take])?;
                remaining -= take as u64;
            }

            return Ok(output);
        }

        let tree = build_tree(&distinct);

        // --- Читаем битовый поток потоково - НЕ копируем остаток
        // артефакта целиком в память, как было раньше. ---
        let mut reader = StreamingBitReader::new(BufferedArtifactReader::new(&mut artifact));
        let mut decoded_count: u64 = 0;
        let mut out_buf: Vec<u8> = Vec::with_capacity(OUTPUT_FLUSH_SIZE);

        while decoded_count < original_size {
            let mut node = &tree;
            loop {
                match node {
                    HuffmanNode::Leaf { symbol, .. } => {
                        out_buf.push(*symbol);
                        decoded_count += 1;
                        break;
                    }
                    HuffmanNode::Internal { left, right, .. } => {
                        let bit = reader.read_bit()?;
                        node = if bit { right } else { left };
                    }
                }
            }

            if out_buf.len() >= OUTPUT_FLUSH_SIZE {
                output.write_chunk(&out_buf)?;
                out_buf.clear();
            }
        }

        if !out_buf.is_empty() {
            output.write_chunk(&out_buf)?;
        }

        Ok(output)
    }

    fn id(&self) -> CompressionId {
        CompressionId::Huffman
    }
}
