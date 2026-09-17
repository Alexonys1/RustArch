//! Модуль `hamming` — расширенный (SECDED) код Хэмминга, реализующий
//! интерфейс [`ErrorCorrectionCode`] проекта.
//!
//! Код Хэмминга параметризован количеством бит данных/чётности
//! (`data_bits`/`parity_bits`), как того требуют уже объявленные в проекте
//! варианты [`FecId::Hamming7_4`] (классический Хэмминг (7,4): 4 бита
//! данных, 3 бита чётности) и [`FecId::Hamming15_11`] (Хэмминг (15,11): 11
//! бит данных, 4 бита чётности).
//!
//! Для честного SECDED (single error correction, double error detection) к
//! классическому "совершенному" коду Хэмминга (n = 2^m - 1, где
//! m = `parity_bits`) добавляется ОДИН дополнительный общий бит чётности на
//! весь блок, давая на выходе блоки по `data_bits + parity_bits + 1` бит: 8
//! бит для варианта 7_4 и 16 бит для варианта 15_11. Без этого бита
//! классический код Хэмминга способен ИСПРАВИТЬ одиночную ошибку, но не
//! способен НАДЁЖНО ОБНАРУЖИТЬ двукратную (может тихо "исправить" её в
//! неверное значение) - для задачи помехозащиты это недопустимо.
//!
//! Раскладка бит внутри блока - классическая: позиции 1, 2, 4, 8, ... (то
//! есть степени двойки, 1-индексация) отведены под биты чётности Хэмминга,
//! все остальные позиции от 1 до `n = data_bits + parity_bits` заняты
//! битами данных по порядку. Последний, `n + 1`-й бит блока - общий бит
//! чётности всего блока (SECDED).
//!
//! # Потоковая обработка и память
//!
//! Блоки Хэмминга полностью независимы друг от друга (в отличие, например,
//! от LZ77, которому нужно окно предыдущих данных) - это значит, что нет
//! никакой необходимости держать весь файл в памяти целиком. Реализация
//! читает и пишет данные чанками через `Artifact::read_next_chunk_with_clone` /
//! `write_chunk_from`, перекладывая биты между чтением и записью через
//! маленький (постоянного размера, не зависящего от размера файла)
//! битовый аккумулятор на регистрах `u64`/`u32` - без единой аллокации на
//! блок и без промежуточных `Vec<bool>` (каждый элемент которого в Rust
//! занимает целый байт, а не бит - на большом файле это давало honestly
//! восьмикратный перерасход памяти на пустом месте). Пиковая
//! дополнительная память ограничена размером одного чанка артефакта
//! (`chunk_size`, по умолчанию 512 КиБ) независимо от размера файла и
//! числа параллельно работающих потоков - именно это и было источником
//! чрезмерного потребления памяти (и, как следствие, свопинга/торможения)
//! при параллельной обработке больших файлов.

use crate::error::AppError;
use crate::archiver::Artifact;
use super::FecId;

use super::{ErrorCorrectionCode, FecReport};


pub struct HammingCode {
    pub data_bits: u32,
    pub parity_bits: u32,
    variant: FecId,
}


impl HammingCode {
    pub fn new_7_4() -> Self {
        Self { data_bits: 4, parity_bits: 3, variant: FecId::Hamming7_4 }
    }
    pub fn new_15_11() -> Self {
        Self { data_bits: 11, parity_bits: 4, variant: FecId::Hamming15_11 }
    }

    /// Проверяет, что `(data_bits, parity_bits)` образуют "совершенный" код
    /// Хэмминга (2^parity_bits - 1 == data_bits + parity_bits), и
    /// возвращает их как `(k, m, n)`.
    fn dimensions(&self) -> Result<(usize, usize, usize), AppError> {
        let k = self.data_bits as usize;
        let m = self.parity_bits as usize;

        if m == 0 || m >= usize::BITS as usize {
            return Err(AppError::Fec(format!(
                "Hamming: недопустимое количество проверочных бит parity_bits={m}"
            )));
        }

        let n = k + m;
        if n != (1usize << m) - 1 {
            return Err(AppError::Fec(format!(
                "Hamming: (data_bits={k}, parity_bits={m}) не образуют совершенный код - \
                 ожидалось data_bits + parity_bits == 2^parity_bits - 1"
            )));
        }

        // Кодовое слово (n + 1 бит, с учётом общего бита чётности) должно
        // помещаться в u32 - иначе битовые операции ниже переполнятся.
        // Для (7,4)/(15,11) это 8/16 бит - огромный запас.
        if n + 1 > u32::BITS as usize {
            return Err(AppError::Fec(format!(
                "Hamming: блок из {} бит (data_bits={k} + parity_bits={m} + 1) не помещается в u32",
                n + 1
            )));
        }

        Ok((k, m, n))
    }
}


fn is_power_of_two(x: usize) -> bool {
    x != 0 && (x & (x - 1)) == 0
}


/// Читает из артефакта ровно `n` байт (или возвращает ошибку, если поток
/// оборвался раньше времени).
fn read_exact_from_artifact(artifact: &mut Artifact, n: usize) -> Result<Vec<u8>, AppError> {
    let mut buf = vec![0u8; n];
    let mut filled = 0;

    while filled < n {
        let read = artifact.read_chunk_to(&mut buf[filled..])?;
        if read == 0 {
            return Err(AppError::Fec(
                "Hamming: неожиданный конец потока при чтении заголовка".to_string(),
            ));
        }
        filled += read;
    }

    Ok(buf)
}


/// Предвычисленные для конкретных (k, m) таблицы, нужные, чтобы кодировать
/// и декодировать блоки без каких-либо аллокаций и без перебора всех `n`
/// позиций на каждый блок (перебор нужен только один раз, здесь, при
/// построении таблиц - на файл, а не на блок).
struct HammingTables {
    k: usize,
    m: usize,
    n: usize,
    /// `full_masks[i]` - битовая маска позиций 1..=n (бит `pos - 1`
    /// кодового слова), у которых i-й бит номера позиции установлен в 1.
    /// Один и тот же набор масок годится и для вычисления бит чётности при
    /// кодировании (бит на позиции `2^i` в момент вычисления ещё не
    /// выставлен, то есть равен 0 - включать его в маску или нет, роли не
    /// играет), и для вычисления синдрома при декодировании.
    full_masks: Vec<u32>,
    /// 0-индексированные позиции (в пределах n-битного кодового слова, без
    /// учёта общего бита чётности), которые занимают биты ДАННЫХ, по
    /// порядку.
    data_positions: Vec<usize>,
}

impl HammingTables {
    fn build(k: usize, m: usize, n: usize) -> Self {
        let mut data_positions = Vec::with_capacity(k);
        for pos in 1..=n {
            if !is_power_of_two(pos) {
                data_positions.push(pos - 1);
            }
        }

        let mut full_masks = vec![0u32; m];
        for (i, mask) in full_masks.iter_mut().enumerate() {
            for pos in 1..=n {
                if (pos >> i) & 1 != 0 {
                    *mask |= 1 << (pos - 1);
                }
            }
        }

        Self { k, m, n, full_masks, data_positions }
    }
}


/// Кодирует `k` младших бит `data` (без учёта регистра — реально
/// используются только `tables.k` младших бит) в блок из `n + 1` бит
/// кодового слова (n = k + m — классический код Хэмминга, "+ 1" — общий
/// бит чётности SECDED). Чистая целочисленная арифметика — без единой
/// аллокации, что критично: на большом файле эта функция вызывается на
/// каждый блок (для (7,4) — на каждые 4 бита исходных данных).
fn encode_block(tables: &HammingTables, data: u32) -> u32 {
    let mut codeword: u32 = 0;

    for (i, &bit_pos) in tables.data_positions.iter().enumerate() {
        let bit = (data >> (tables.k - 1 - i)) & 1;
        codeword |= bit << bit_pos;
    }

    for i in 0..tables.m {
        let parity = (codeword & tables.full_masks[i]).count_ones() & 1;
        codeword |= parity << ((1usize << i) - 1);
    }

    let overall_parity = codeword.count_ones() & 1;
    codeword | (overall_parity << tables.n)
}


/// Результат декодирования одного блока.
enum BlockStatus {
    /// Ошибок не было.
    Ok,
    /// Была одиночная ошибка (в т.ч. в самом общем бите чётности) - исправлена.
    Corrected,
    /// Обнаружена двукратная ошибка - исправить нельзя.
    Uncorrectable,
}


/// Декодирует один принятый блок из `n + 1` бит (в младших битах `received`),
/// по возможности исправляя одиночную ошибку. Возвращает восстановленные
/// `k` бит данных (при неисправимой ошибке - как получилось, "по
/// возможности") и статус блока. Как и `encode_block` - без аллокаций.
fn decode_block(tables: &HammingTables, received: u32) -> (u32, BlockStatus) {
    let n = tables.n;
    let codeword = received & ((1u32 << n) - 1);
    let overall_parity_received = (received >> n) & 1;

    // Синдром: i-й бит синдрома - это результат i-й проверки чётности
    // (0, если группа сходится, 1 - если нет). Если ошибок среди позиций
    // 1..n нет, синдром равен 0; если есть ровно одна ошибка - синдром
    // равен номеру ошибочной позиции.
    let mut syndrome: usize = 0;
    for i in 0..tables.m {
        let bit = (codeword & tables.full_masks[i]).count_ones() & 1;
        syndrome |= (bit as usize) << i;
    }

    // Чётность по всем n + 1 принятым битам: 0 при чётном числе искажённых
    // бит (в т.ч. 0), 1 - при нечётном.
    let total_parity = (codeword.count_ones() & 1) ^ overall_parity_received;

    let (corrected, status) = match (syndrome, total_parity) {
        (0, 0) => (codeword, BlockStatus::Ok),
        // Искажён только общий бит чётности - на данные не влияет.
        (0, _) => (codeword, BlockStatus::Corrected),
        // Одиночная ошибка в позиции `pos` - исправляется инверсией бита.
        (pos, 1) => (codeword ^ (1u32 << (pos - 1)), BlockStatus::Corrected),
        // Синдром указывает на ошибку, но суммарная чётность "сходится" -
        // признак двукратной (неисправимой) ошибки.
        (_, _) => (codeword, BlockStatus::Uncorrectable),
    };

    let mut data: u32 = 0;
    for (i, &bit_pos) in tables.data_positions.iter().enumerate() {
        let bit = (corrected >> bit_pos) & 1;
        data |= bit << (tables.k - 1 - i);
    }

    (data, status)
}


/// Аккумулятор для потокового ЧТЕНИЯ бит: принимает входные байты по мере
/// поступления (в т.ч. из разных чанков артефакта) и отдаёт `width`-битные
/// значения, как только их накопилось достаточно. Хранит не более
/// `7 + max(width)` бит одновременно (для наших блоков - максимум 16+7=23),
/// поэтому `u64` - гарантированный запас без переполнения. Никогда не
/// растёт - в отличие от `Vec<bool>` на весь файл в предыдущей версии.
struct BitSource {
    bits: u64,
    count: u32,
}

impl BitSource {
    fn new() -> Self {
        Self { bits: 0, count: 0 }
    }

    fn push_byte(&mut self, byte: u8) {
        self.bits = (self.bits << 8) | byte as u64;
        self.count += 8;
    }

    /// Если накоплено >= `width` бит - извлекает и возвращает СТАРШИЕ
    /// `width` бит, иначе `None`. Не аллоцирует.
    fn try_take(&mut self, width: u32) -> Option<u32> {
        if self.count < width {
            return None;
        }
        let shift = self.count - width;
        let mask: u64 = (1u64 << width) - 1;
        let value = ((self.bits >> shift) & mask) as u32;

        self.count -= width;
        self.bits &= if self.count == 0 { 0 } else { (1u64 << self.count) - 1 };

        Some(value)
    }

    /// Возвращает оставшиеся (< width) бит, дополненные нулями СПРАВА до
    /// `width` бит - используется только один раз, для хвоста файла, длина
    /// которого не кратна `width` бит.
    fn take_remaining_padded(&mut self, width: u32) -> u32 {
        let value = (self.bits as u32) << (width - self.count);
        self.count = 0;
        self.bits = 0;
        value
    }
}


/// Симметричный `BitSource` аккумулятор для потоковой ЗАПИСИ бит: копит
/// биты и, как только накопился целый байт, откладывает его в маленький
/// (ограниченный `flush_threshold`) буфер, который периодически сбрасывается
/// в артефакт. Это и есть основной механизм, ограничивающий пиковую
/// дополнительную память константой `flush_threshold` независимо от
/// размера файла - вместо того, чтобы копить весь результат в памяти и
/// сбрасывать одним огромным `write_chunk_from` в конце.
struct BitSink {
    bits: u64,
    count: u32,
    out: Vec<u8>,
    flush_threshold: usize,
}

impl BitSink {
    fn new(flush_threshold: usize) -> Self {
        Self { bits: 0, count: 0, out: Vec::with_capacity(flush_threshold), flush_threshold }
    }

    fn push_bits(&mut self, value: u32, width: u32) {
        if width == 0 {
            return;
        }

        let mask: u64 = (1u64 << width) - 1;
        self.bits = (self.bits << width) | (value as u64 & mask);
        self.count += width;

        while self.count >= 8 {
            let shift = self.count - 8;
            self.out.push(((self.bits >> shift) & 0xFF) as u8);
            self.count -= 8;
        }
        self.bits &= if self.count == 0 { 0 } else { (1u64 << self.count) - 1 };
    }

    /// Дописывает нулями до полного байта - вызывается ровно один раз, в
    /// самом конце потока.
    fn flush_final_byte(&mut self) {
        if self.count > 0 {
            let byte = ((self.bits << (8 - self.count)) & 0xFF) as u8;
            self.out.push(byte);
            self.count = 0;
            self.bits = 0;
        }
    }

    fn maybe_flush(&mut self, output: &mut Artifact) -> Result<(), AppError> {
        if self.out.len() >= self.flush_threshold {
            output.write_chunk_from(&self.out)?;
            self.out.clear();
        }
        Ok(())
    }

    fn final_flush(&mut self, output: &mut Artifact) -> Result<(), AppError> {
        if !self.out.is_empty() {
            output.write_chunk_from(&self.out)?;
            self.out.clear();
        }
        Ok(())
    }
}


impl ErrorCorrectionCode for HammingCode {
    fn encode(&self, mut artifact: Artifact) -> Result<Artifact, AppError> {
        let (k, m, n) = self.dimensions()?;
        let tables = HammingTables::build(k, m, n);
        let block_len = (n + 1) as u32;

        // Первый проход: только считаем исходный размер в байтах, ничего
        // не накапливая в памяти (каждый чанк читается и сразу
        // отбрасывается) - это позволяет записать original_size в
        // заголовок ДО тела потока, не читая (и тем более не храня) файл
        // целиком дважды одновременно.
        let mut original_size: u64 = 0;
        while let Some(chunk) = artifact.read_next_chunk_with_clone()? {
            original_size += chunk.len() as u64;
        }
        artifact.rewind_reading();

        let mut output = Artifact::new_with_temp_file_suffix(&artifact, "fec_encoded");
        output.write_chunk_from(&original_size.to_le_bytes())?;

        // Порог сброса накопленного выходного буфера - ограничивает
        // пиковую дополнительную память константой (размером чанка), а не
        // размером файла.
        let flush_threshold = output.chunk_size.get();

        let mut source = BitSource::new();
        let mut sink = BitSink::new(flush_threshold);

        while let Some(chunk) = artifact.read_next_chunk_with_clone()? {
            for &byte in &chunk {
                source.push_byte(byte);
                while let Some(data_bits) = source.try_take(k as u32) {
                    let codeword = encode_block(&tables, data_bits);
                    sink.push_bits(codeword, block_len);
                }
            }
            sink.maybe_flush(&mut output)?;
        }

        // Хвост короче k бит (исходная длина не кратна k бит) - дополняем
        // нулями справа и кодируем как последний блок.
        if source.count > 0 {
            let data_bits = source.take_remaining_padded(k as u32);
            let codeword = encode_block(&tables, data_bits);
            sink.push_bits(codeword, block_len);
        }

        sink.flush_final_byte();
        sink.final_flush(&mut output)?;

        Ok(output)
    }

    fn decode(&self, mut artifact: Artifact) -> Result<(Artifact, FecReport), AppError> {
        let (k, m, n) = self.dimensions()?;
        let tables = HammingTables::build(k, m, n);
        let block_len = (n + 1) as u32;

        let original_size = u64::from_le_bytes(
            read_exact_from_artifact(&mut artifact, 8)?.try_into().unwrap(),
        );
        let total_data_bits = original_size * 8;

        let mut output = Artifact::new_with_temp_file_suffix(&artifact, "fec_decoded");
        let flush_threshold = output.chunk_size.get();

        let mut report = FecReport::default();
        let mut produced_bits: u64 = 0;

        let mut source = BitSource::new();
        let mut sink = BitSink::new(flush_threshold);

        'outer: while let Some(chunk) = artifact.read_next_chunk_with_clone()? {
            for &byte in &chunk {
                source.push_byte(byte);

                while let Some(received) = source.try_take(block_len) {
                    let (data_bits, status) = decode_block(&tables, received);

                    report.blocks_processed += 1;
                    match status {
                        BlockStatus::Ok => {}
                        BlockStatus::Corrected => report.blocks_corrected += 1,
                        BlockStatus::Uncorrectable => report.blocks_uncorrectable += 1,
                    }

                    // Последний блок мог быть при кодировании дополнен
                    // нулями справа - здесь отбрасываем эту дополняющую
                    // часть, беря только реально нужные (оставшиеся до
                    // original_size) старшие биты блока.
                    let remaining_bits = total_data_bits - produced_bits;
                    let bits_to_take = (k as u64).min(remaining_bits) as u32;

                    if bits_to_take > 0 {
                        let value = data_bits >> (k as u32 - bits_to_take);
                        sink.push_bits(value, bits_to_take);
                        produced_bits += bits_to_take as u64;
                    }

                    if produced_bits >= total_data_bits {
                        break 'outer;
                    }
                }
            }

            sink.maybe_flush(&mut output)?;
        }

        if produced_bits < total_data_bits {
            return Err(AppError::Fec(format!(
                "Hamming: закодированный поток короче ожидаемого - восстановлено {produced_bits} из {total_data_bits} бит"
            )));
        }

        sink.final_flush(&mut output)?;

        Ok((output, report))
    }

    fn id(&self) -> FecId {
        self.variant
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn artifact_from_bytes(bytes: &[u8]) -> Artifact {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("rustarch_hamming_test_{id}.bin"));
        std::fs::write(&path, bytes).unwrap();
        Artifact::from_file(&path).unwrap()
    }

    fn round_trip(codec: &HammingCode, data: &[u8]) {
        let encoded = codec.encode(artifact_from_bytes(data)).expect("encode не должен падать");
        let (mut decoded, report) = codec.decode(encoded).expect("decode не должен падать");

        assert_eq!(report.blocks_uncorrectable, 0, "не должно быть неисправимых блоков без внесённых ошибок");

        let mut out = Vec::new();
        decoded.rewind_reading();
        while let Some(chunk) = decoded.read_next_chunk_with_clone().unwrap() {
            out.extend_from_slice(&chunk);
        }
        assert_eq!(out, data);
    }

    #[test]
    fn round_trip_without_errors_7_4() {
        let codec = HammingCode::new_7_4();
        for case in [
            &b""[..],
            &b"\x00"[..],
            &b"\xFF"[..],
            &[0x00, 0xFF, 0xAA, 0x55, 0x0F, 0xF0][..],
            b"Media library test payload 1234567890",
        ] {
            round_trip(&codec, case);
        }
    }

    #[test]
    fn round_trip_without_errors_15_11() {
        let codec = HammingCode::new_15_11();
        for case in [
            &b""[..],
            &b"\x00"[..],
            &[0x00, 0xFF, 0xAA, 0x55, 0x0F, 0xF0][..],
            b"Media library test payload 1234567890",
        ] {
            round_trip(&codec, case);
        }
    }

    /// Проверка на файле, размер которого больше нескольких chunk_size, -
    /// именно этот путь раньше был чувствителен к перерасходу памяти.
    #[test]
    fn round_trip_multi_chunk() {
        let codec = HammingCode::new_15_11();
        let data: Vec<u8> = (0..5_000_000u32).map(|i| (i % 251) as u8).collect();
        round_trip(&codec, &data);
    }

    /// Одиночная ошибка в любой из позиций блока должна быть исправлена
    /// прозрачно, без потери данных - для обеих зарегистрированных
    /// разновидностей кода.
    #[test]
    fn single_bit_error_is_corrected_in_every_position() {
        for (k, m) in [(4usize, 3usize), (11, 4)] {
            let n = k + m;
            let block_len = n + 1;
            let tables = HammingTables::build(k, m, n);

            let patterns: Vec<u32> = vec![
                0,
                (1u32 << k) - 1,
                (0..k).fold(0u32, |acc, i| if i % 2 == 0 { acc | (1 << i) } else { acc }),
                (0..k).fold(0u32, |acc, i| if i % 3 == 0 { acc | (1 << i) } else { acc }),
            ];

            for pattern in patterns {
                let codeword = encode_block(&tables, pattern);
                for bit_pos in 0..block_len {
                    let corrupted = codeword ^ (1u32 << bit_pos);

                    let (decoded, status) = decode_block(&tables, corrupted);
                    assert!(matches!(status, BlockStatus::Corrected), "одиночная ошибка обязана исправляться (k={k}, бит {bit_pos})");
                    assert_eq!(decoded, pattern, "неверно исправлена ошибка в бите {bit_pos} (k={k})");
                }
            }
        }
    }

    /// Двукратная ошибка должна быть обнаружена как неисправимая, а не
    /// привести к молчаливому искажению данных.
    #[test]
    fn double_bit_error_is_detected_as_uncorrectable() {
        let (k, m) = (4usize, 3usize);
        let n = k + m;
        let block_len = n + 1;
        let tables = HammingTables::build(k, m, n);
        let pattern = 0b1010u32;
        let codeword = encode_block(&tables, pattern);

        let mut total = 0;
        let mut detected = 0;
        for i in 0..block_len {
            for j in (i + 1)..block_len {
                total += 1;
                let corrupted = codeword ^ (1u32 << i) ^ (1u32 << j);

                let (_, status) = decode_block(&tables, corrupted);
                if matches!(status, BlockStatus::Uncorrectable) {
                    detected += 1;
                }
            }
        }
        assert_eq!(detected, total, "не все двукратные ошибки обнаружены");
    }

    /// Внесение двукратной ошибки в реальный поток через артефакт не
    /// должно приводить к панике - `decode` обязан вернуть отчёт с
    /// ненулевым `blocks_uncorrectable` вместо падения.
    #[test]
    fn decode_reports_uncorrectable_blocks_via_artifact() {
        let codec = HammingCode::new_7_4();
        let data = [0x12u8, 0x34];

        let mut encoded_artifact = codec.encode(artifact_from_bytes(&data)).unwrap();
        let mut encoded_bytes = Vec::new();
        while let Some(chunk) = encoded_artifact.read_next_chunk_with_clone().unwrap() {
            encoded_bytes.extend_from_slice(&chunk);
        }

        // Заголовок - 8 байт original_size, дальше идут закодированные
        // блоки. Портим первый закодированный байт двукратной ошибкой.
        encoded_bytes[8] ^= 0b0000_0011;

        let (_decoded, report) = codec.decode(artifact_from_bytes(&encoded_bytes)).expect("decode не должен падать даже при неисправимой ошибке");
        assert!(report.blocks_uncorrectable > 0);
    }
}
