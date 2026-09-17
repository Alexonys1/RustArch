use std::cmp::Ordering;
use std::collections::BinaryHeap;

use crate::error::AppError;
use crate::archiver::Artifact;
use super::{CompressionId, Compressor};


const ALPHABET_SIZE: usize = 256;
const OUTPUT_FLUSH_SIZE: usize = 256 * 1024;

/// Размер таблицы табличного декодирования - 2^12 = 4096 записей. Любой
/// код длиной <= TABLE_BITS декодируется ОДНИМ обращением к таблице вместо
/// побитового спуска по дереву. Коды длиннее (крайне редкий случай для
/// 256-символьного алфавита на реальных данных - потребовал бы частот,
/// растущих почти строго по числам Фибоначчи) обрабатываются медленным
/// fallback-путём - обходом дерева бит за битом, как раньше.
const TABLE_BITS: u32 = 12;
const TABLE_SIZE: usize = 1 << TABLE_BITS;

/// Код Хаффмана, упакованный как (биты, длина_в_битах) вместо `Vec<bool>`.
/// Copy-тип - ноль аллокаций на таблицу кодов (было до 256 отдельных
/// куча-аллокаций, по одной на символ).
type Code = (u32, u8);


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


/// Заполняет таблицу кодов, накапливая биты сдвигами прямо в `u32` по ходу
/// рекурсии - ни одной промежуточной аллокации (было `prefix: &mut
/// Vec<bool>` + `prefix.clone()` на каждом листе).
fn assign_codes(node: &HuffmanNode, code: u32, len: u8, codes: &mut [Option<Code>]) { // TODO: А насколько длинные коды там могут быть?
    match node {
        HuffmanNode::Leaf { symbol, .. } => {
            codes[*symbol as usize] = Some((code, len));
        }
        HuffmanNode::Internal { left, right, .. } => {
            debug_assert!(len < 32, "код Хаффмана длиннее 32 бит - экстремально маловероятно на реальных данных");
            assign_codes(left, code << 1, len + 1, codes);
            assign_codes(right, (code << 1) | 1, len + 1, codes);
        }
    }
}


// ============================================================
// ЗАПИСЬ: аккумулятор вместо побитового цикла
// ============================================================

/// Вместо накопления по одному биту (`push_bit` в цикле на каждый бит
/// кода) - заносим ВЕСЬ код одной операцией сдвига в 64-битный аккумулятор
/// и извлекаем готовые байты пока их накопилось >= 8. Порядок бит - тот
/// же MSB-first, что был раньше (бит, соответствующий корню дерева,
/// первым попадает в поток) - это не отдельная строка кода "на всякий
/// случай", а естественное следствие того, как код размещается в
/// аккумуляторе: маскировка последнего неполного байта не нужна отдельным
/// кодом, она "встроена" в природу аккумулятора - биты выше `nbits`
/// гарантированно нулевые, поэтому обрезание до u8 в конце само даёт
/// нужный нулевой паддинг.
struct StreamingBitWriter<'a> {
    output: &'a mut Artifact,
    buf: Vec<u8>,
    acc: u64,
    nbits: u32,
}

impl<'a> StreamingBitWriter<'a> {
    fn new(output: &'a mut Artifact) -> Self {
        Self { output, buf: Vec::with_capacity(OUTPUT_FLUSH_SIZE), acc: 0, nbits: 0 }
    }

    /// Записывает код целиком за одну операцию вместо цикла по битам.
    /// Корректно, пока `code` содержит ровно `len` значащих бит,
    /// левоюстированных в пределах этих `len` бит (гарантируется тем, как
    /// `assign_codes` их строит).
    fn push_code(&mut self, code: u32, len: u8) -> Result<(), AppError> {
        let len = len as u32;
        // Место в аккумуляторе для нового кода начинается сразу после уже
        // накопленных nbits бит (они занимают верхние разряды).
        self.acc |= (code as u64) << (64 - self.nbits - len);
        self.nbits += len;

        while self.nbits >= 8 {
            self.buf.push((self.acc >> 56) as u8);
            self.acc <<= 8;
            self.nbits -= 8;
            if self.buf.len() >= OUTPUT_FLUSH_SIZE {
                self.output.write_chunk_from(&self.buf)?;
                self.buf.clear();
            }
        }
        Ok(())
    }

    fn finish(mut self) -> Result<(), AppError> {
        if self.nbits > 0 {
            self.buf.push((self.acc >> 56) as u8);
        }
        if !self.buf.is_empty() {
            self.output.write_chunk_from(&self.buf)?;
        }
        Ok(())
    }
}


// ============================================================
// ЧТЕНИЕ ДЛЯ ДЕКОДИРОВАНИЯ: аккумулятор + таблица
// ============================================================

/// Готовая запись табличного декодирования: либо "код такой-то длины дал
/// вот этот символ", либо "код длиннее TABLE_BITS - идите по дереву".
#[derive(Clone, Copy)]
enum DecodeEntry {
    Symbol(u8, u8), // (символ, длина кода в битах)
    Escape,
}

/// Строит плоскую таблицу размером `TABLE_SIZE`: для каждого возможного
/// TABLE_BITS-битного окна - каким символом оно резолвится и сколько бит
/// код реально занял. Коды короче TABLE_BITS размножаются по всем
/// "безразличным" хвостовым битам (префиксное свойство кода Хаффмана
/// гарантирует, что это не создаёт неоднозначностей).
fn build_decode_table(tree: &HuffmanNode) -> Vec<DecodeEntry> {
    let mut table = vec![DecodeEntry::Escape; TABLE_SIZE];
    fill_table(tree, 0, 0, &mut table);
    table
}

fn fill_table(node: &HuffmanNode, code: u32, len: u32, table: &mut [DecodeEntry]) {
    match node {
        HuffmanNode::Leaf { symbol, .. } => {
            if len <= TABLE_BITS {
                let shift = TABLE_BITS - len;
                let base = (code as usize) << shift;
                for i in 0..(1usize << shift) {
                    table[base + i] = DecodeEntry::Symbol(*symbol, len as u8);
                }
            }
            // len > TABLE_BITS: соответствующий узел не укладывается в
            // таблицу - все ведущие к нему индексы остаются Escape
            // (не были и не будут перезаписаны), декодер пойдёт по дереву.
        }
        HuffmanNode::Internal { left, right, .. } => {
            if len < TABLE_BITS {
                fill_table(left, code << 1, len + 1, table);
                fill_table(right, (code << 1) | 1, len + 1, table);
            }
            // len == TABLE_BITS и это внутренний узел (код ещё не
            // разрешился в символ за отведённые биты) - глубже не идём,
            // это и есть Escape-случай для длинных кодов.
        }
    }
}

/// Скользящее окно бит поверх `Artifact::next_chunk()`. Не хранит `Cow`
/// как поле (см. пояснение в начале ответа) - вместо этого при переходе
/// на новый чанк берёт владение через `.into_owned()`: бесплатно для
/// File/FileWindow (там и так `Cow::Owned`), одно копирование на границу
/// чанка для Memory (не на байт).
struct BitWindow<'a> {
    artifact: &'a mut Artifact,
    chunk: Vec<u8>,
    chunk_pos: usize,
    acc: u32,   // валидные биты - в СТАРШИХ разрядах (MSB-first)
    nbits: u32,
}

impl<'a> BitWindow<'a> {
    fn new(artifact: &'a mut Artifact) -> Self {
        Self { artifact, chunk: Vec::new(), chunk_pos: 0, acc: 0, nbits: 0 }
    }

    /// Догружает аккумулятор минимум до `want` валидных бит. Если реальные
    /// данные кончились раньше - оставшиеся "виртуальные" биты трактуются
    /// как нулевой паддинг (они и так уже нули в `acc`) - безопасно, т.к.
    /// решение "сколько символов декодировать" принимается по
    /// `original_size`, а не по количеству реально прочитанных бит.
    fn fill(&mut self, want: u32) -> Result<(), AppError> {
        while self.nbits < want {
            if self.chunk_pos >= self.chunk.len() {
                match self.artifact.next_chunk()? {
                    Some(cow) => {
                        self.chunk = cow.into_owned();
                        self.chunk_pos = 0;
                    }
                    None => return Ok(()), // конец потока - остаток трактуем как нулевой паддинг
                }
                if self.chunk.is_empty() {
                    continue; // на случай пустого чанка - не должно происходить, но не зацикливаемся
                }
            }
            let byte = self.chunk[self.chunk_pos];
            self.chunk_pos += 1;
            self.acc |= (byte as u32) << (24 - self.nbits);
            self.nbits += 8;
        }
        Ok(())
    }

    fn peek(&self, n: u32) -> u32 {
        self.acc >> (32 - n)
    }

    fn consume(&mut self, n: u32) {
        self.acc <<= n;
        self.nbits = self.nbits.saturating_sub(n);
    }
}

fn read_exact_from_artifact(artifact: &mut Artifact, n: usize) -> Result<Vec<u8>, AppError> {
    let mut buf = vec![0u8; n];
    let mut filled = 0;
    while filled < n {
        let read = artifact.read_chunk_to(&mut buf[filled..])?;
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


impl Compressor for HuffmanCompressor {
    fn compress(&self, mut artifact: Artifact) -> Result<Artifact, AppError> {
        // --- Первый проход: считаем частоты байт и общий размер данных ---
        // artifact.next_chunk() напрямую: для Memory-состояния это
        // Cow::Borrowed - ни одного скопированного байта на весь проход.
        let mut freqs = [0u64; ALPHABET_SIZE];
        let mut original_size: u64 = 0;

        while let Some(chunk) = artifact.next_chunk()? {
            for &b in chunk.iter() {
                freqs[b as usize] += 1;
            }
            original_size += chunk.len() as u64;
        }
        artifact.rewind_reading();

        let mut output = Artifact::new_with_temp_file_suffix(&artifact, "compressed");
        output.write_chunk_from(&original_size.to_le_bytes())?;

        let distinct: Vec<(u8, u64)> = freqs
            .iter()
            .enumerate()
            .filter(|&(_, &f)| f > 0)
            .map(|(symbol, &f)| (symbol as u8, f))
            .collect();

        output.write_chunk_from(&(distinct.len() as u32).to_le_bytes())?;
        for &(symbol, freq) in &distinct {
            output.write_chunk_from(&[symbol])?;
            output.write_chunk_from(&freq.to_le_bytes())?;
        }

        if original_size == 0 || distinct.len() <= 1 {
            return Ok(output);
        }

        let tree = build_tree(&distinct);
        let mut codes: [Option<Code>; ALPHABET_SIZE] = [None; ALPHABET_SIZE];
        assign_codes(&tree, 0, 0, &mut codes);

        // --- Второй проход: кодируем данные аккумулятором ---
        let mut writer = StreamingBitWriter::new(&mut output);
        while let Some(chunk) = artifact.next_chunk()? {
            for &b in chunk.iter() {
                let (code, len) = codes[b as usize]
                    .expect("символ отсутствует в построенной таблице кодов - баг подсчёта частот");
                writer.push_code(code, len)?;
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
                output.write_chunk_from(&block[..take])?;
                remaining -= take as u64;
            }
            return Ok(output);
        }

        let tree = build_tree(&distinct);
        let decode_table = build_decode_table(&tree);

        let mut bits = BitWindow::new(&mut artifact);
        let mut decoded_count: u64 = 0;
        let mut out_buf: Vec<u8> = Vec::with_capacity(OUTPUT_FLUSH_SIZE);

        while decoded_count < original_size {
            bits.fill(TABLE_BITS)?;
            let index = bits.peek(TABLE_BITS) as usize;

            match decode_table[index] {
                DecodeEntry::Symbol(symbol, len) => {
                    // Общий случай - один взгляд в таблицу вместо
                    // побитового спуска по дереву длиной `len` шагов.
                    bits.consume(len as u32);
                    out_buf.push(symbol);
                    decoded_count += 1;
                }
                DecodeEntry::Escape => {
                    // Редкий fallback: код длиннее TABLE_BITS - идём по
                    // дереву бит за битом, как в исходной версии.
                    let mut node = &tree;
                    loop {
                        match node {
                            HuffmanNode::Leaf { symbol, .. } => {
                                out_buf.push(*symbol);
                                decoded_count += 1;
                                break;
                            }
                            HuffmanNode::Internal { left, right, .. } => {
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
