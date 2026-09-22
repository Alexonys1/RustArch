use std::io;

use crate::error::AppError;
use crate::archiver::Artifact;
use crate::archiver::memory_budget::BudgetGuard;


/// Размер блока чтения
const INPUT_READ_SIZE: usize = 256 * 1024;


/// Оптимизация LZSS, удерживает в памяти последние 32 КиБ информации
pub struct SlidingWindow {
    buffer: Vec<u8>,
    /// Абсолютная позиция в потоке, которой соответствует buffer[0].
    base_pos: u64,
    exhausted: bool,
    window_size: usize,
    capacity: usize,
    _guard: BudgetGuard,
}

impl SlidingWindow {
    pub fn new(window_size: usize, lookahead_size: usize) -> Result<Self, AppError> {
        let capacity = window_size
            .checked_add(INPUT_READ_SIZE)
            .and_then(|v| v.checked_add(lookahead_size))
            .ok_or_else(|| AppError::Compression(
                "LZSS: Переполнение при вычислении размера скользящего окна".into(),
            ))?;
        let mut guard = BudgetGuard::default();
        if !guard.try_grow(capacity as i64) {
            return Err(AppError::Compression(format!(
                "LZSS: Не удалось зарезервировать {capacity} байт под скользящее окно - бюджет памяти исчерпан"
            )));
        }
        Ok(Self {
            buffer: Vec::with_capacity(capacity),
            base_pos: 0,
            exhausted: false,
            window_size,
            capacity,
            _guard: guard,
        })
    }

    /// Выделение памяти и чтение блока файла
    pub fn ensure_available(&mut self, artifact: &mut Artifact, pos: u64, want: usize) -> io::Result<()> {
        if self.available_after(pos) < want && !self.exhausted {
            self.compact_for(pos);
        }

        while !self.exhausted && self.available_after(pos) < want {
            let free = self.capacity.saturating_sub(self.buffer.len());
            if free == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "LZ: Недостаточная ёмкость скользящего окна!",
                ));
            }

            let to_read = free.min(INPUT_READ_SIZE);
            let old_len = self.buffer.len();
            self.buffer.resize(old_len + to_read, 0);
            let read = artifact.read_chunk_to(&mut self.buffer[old_len..])?;
            self.buffer.truncate(old_len + read);
            if read == 0 {
                self.exhausted = true;
            }
        }
        Ok(())
    }

    /// Сброс отработанной части файла
    fn compact_for(&mut self, pos: u64) {
        let keep_from = pos.saturating_sub(self.window_size as u64);
        if keep_from <= self.base_pos {
            return;
        }

        let drop_count = (keep_from - self.base_pos) as usize;
        self.buffer.copy_within(drop_count.., 0);
        self.buffer.truncate(self.buffer.len() - drop_count);
        self.base_pos = keep_from;
    }

    /// Чтение необработанной части файла после сброса
    pub fn available_after(&self, pos: u64) -> usize {
        let local = (pos - self.base_pos) as usize;
        self.buffer.len().saturating_sub(local)
    }

    pub fn byte_at(&self, pos: u64) -> u8 {
        self.buffer[(pos - self.base_pos) as usize]
    }

    /// Непрерывный срез от позиции `pos` до конца буферизованных данных
    pub fn slice_from(&self, pos: u64) -> &[u8] {
        let local = (pos - self.base_pos) as usize;
        &self.buffer[local..]
    }

    pub fn trim_if_needed(&mut self, pos: u64) {
        debug_assert!(pos >= self.base_pos);
    }
}

// ---------------------------------------------------------------------
// HashChain - поиск совпадений без HashMap/SipHash
// ---------------------------------------------------------------------

/// Размер таблицы голов хэш-цепочек.
const HASH_BITS: u32 = 16;
const HASH_SIZE: usize = 1 << HASH_BITS;

/// Значение-метка "цепочка закончилась"
const NONE: u64 = u64::MAX;

/// Классическая схема хэш-цепочек
/// head[hash] - самая свежая позиция с данным хэшем
/// prev[pos % window_size] - позиция предыдущего вхождения ТОГО ЖЕ хэша, что и в позиции pos

pub struct HashChain {
    head: Vec<u64>,
    prev: Vec<u64>,
    window_size: usize,
    _guard: BudgetGuard,
}

impl HashChain {
    pub fn new(window_size: usize) -> Result<Self, AppError> {
        let bytes = (HASH_SIZE + window_size) * std::mem::size_of::<u64>();
        let mut guard = BudgetGuard::default();
        if !guard.try_grow(bytes as i64) {
            return Err(AppError::Compression(format!(
                "LZSS: не удалось зарезервировать {bytes} байт под хэш-таблицу - бюджет памяти исчерпан"
            )));
        }
        Ok(Self {
            head: vec![NONE; HASH_SIZE],
            prev: vec![NONE; window_size],
            window_size,
            _guard: guard,
        })
    }

    fn hash3(b0: u8, b1: u8, b2: u8) -> usize {
        let v = (b0 as u32) | ((b1 as u32) << 8) | ((b2 as u32) << 16);
        ((v.wrapping_mul(2_654_435_761)) >> (32 - HASH_BITS)) as usize
    }

    /// Индексирует позицию `pos` по её 3-байтовому префиксу.
    pub fn insert(&mut self, window: &SlidingWindow, pos: u64) {
        if window.available_after(pos) < 3 {
            return;
        }
        let (b0, b1, b2) = (window.byte_at(pos), window.byte_at(pos + 1), window.byte_at(pos + 2));
        let h = Self::hash3(b0, b1, b2);
        let slot = (pos as usize) % self.window_size;
        self.prev[slot] = self.head[h];
        self.head[h] = pos;
    }

    /// Итератор по цепочке кандидатов для позиции `pos`, от самого свежего к самому старому.
    fn candidates(&self, window: &SlidingWindow, pos: u64, window_start: u64) -> impl Iterator<Item = u64> + '_ {
        let (b0, b1, b2) = (window.byte_at(pos), window.byte_at(pos + 1), window.byte_at(pos + 2));
        let h = Self::hash3(b0, b1, b2);
        let mut cur = self.head[h];
        let window_size = self.window_size;
        let prev = &self.prev;
        std::iter::from_fn(move || {
            if cur == NONE || cur < window_start {
                return None;
            }
            let result = cur;
            cur = prev[(cur as usize) % window_size];
            Some(result)
        })
    }
}

/// Сравнивает две последовательности байт и возвращает длину общего префикса (не больше `max_len`).
fn common_prefix_len(a: &[u8], b: &[u8], max_len: usize) -> usize {
    let limit = max_len.min(a.len()).min(b.len());
    let mut len = 0;

    while len + 8 <= limit {
        let wa = u64::from_le_bytes(a[len..len + 8].try_into().unwrap());
        let wb = u64::from_le_bytes(b[len..len + 8].try_into().unwrap());
        let diff = wa ^ wb;
        if diff != 0 {
            return len + (diff.trailing_zeros() / 8) as usize;
        }
        len += 8;
    }

    while len < limit && a[len] == b[len] {
        len += 1;
    }

    len
}

const NICE_MATCH_LEN: usize = 128;
const GOOD_MATCH_LEN: usize = 32;

/// Ищет самое длинное совпадение для данных, начинающихся в pos
/// Возвращает (offset, length); (0, 0), если совпадения длиной >= min_match_len не нашлось.
pub fn find_longest_match(
    window: &SlidingWindow,
    chain: &HashChain,
    pos: u64,
    window_size: u64,
    max_len: usize,
    min_match_len: usize,
    max_chain_len: usize,
) -> (usize, usize)
{
    if max_len < min_match_len || window.available_after(pos) < min_match_len {
        return (0, 0);
    }

    let window_start = pos.saturating_sub(window_size);
    let current = window.slice_from(pos);
    let mut best_len = 0usize;
    let mut best_offset = 0usize;

    if pos > window_start && window.byte_at(pos - 1) == current[0] {
        let len = common_prefix_len(window.slice_from(pos - 1), current, max_len);
        if len >= min_match_len {
            best_len = len;
            best_offset = 1;
            if len >= NICE_MATCH_LEN || len == max_len {
                return (best_offset, best_len);
            }
        }
    }

    for (checked, cand_pos) in chain.candidates(window, pos, window_start).enumerate() {
        if checked >= max_chain_len {
            break;
        }

        // После хорошего совпадения оставляем только четверть исходного бюджета цепочки.
        if best_len >= GOOD_MATCH_LEN && checked >= max_chain_len.div_ceil(4) {
            break;
        }
        if cand_pos >= pos || (best_offset == 1 && cand_pos + 1 == pos) {
            continue;
        }

        let candidate = window.slice_from(cand_pos);
        // Отбрасываем коллизии 16-битного хэша и кандидаты, которые уже
        // не могут улучшить найденную длину.
        if candidate[0] != current[0]
            || candidate[1] != current[1]
            || candidate[2] != current[2]
            || (best_len > 0 && candidate[best_len] != current[best_len])
        {
            continue;
        }

        let len = common_prefix_len(candidate, current, max_len);

        if len > best_len {
            best_len = len;
            best_offset = (pos - cand_pos) as usize;
            if best_len >= NICE_MATCH_LEN || best_len == max_len {
                break; // "достаточно длинное" совпадение - дальше не ищем
            }
        }
    }

    if best_len >= min_match_len {
        (best_offset, best_len)
    } else {
        (0, 0)
    }
}

// ---------------------------------------------------------------------
// BufferedArtifactReader - читает токены декомпрессии пачками, а не по 1-2 байта напрямую из Artifact
// ---------------------------------------------------------------------

pub struct BufferedArtifactReader<'a> {
    artifact: &'a mut Artifact,
    buf: Vec<u8>,
    pos: usize,
    end: usize,
}

impl<'a> BufferedArtifactReader<'a> {
    pub fn new(artifact: &'a mut Artifact) -> Self {
        Self {
            artifact,
            buf: vec![0; INPUT_READ_SIZE],
            pos: 0,
            end: 0,
        }
    }

    fn fill(&mut self, need: usize) -> Result<(), AppError> {
        while self.end - self.pos < need {
            if self.pos > 0 {
                self.buf.copy_within(self.pos..self.end, 0);
                self.end -= self.pos;
                self.pos = 0;
            }

            let missing = need - (self.end - self.pos);
            if self.buf.len() - self.end < missing {
                self.buf.resize((self.end + missing).max(INPUT_READ_SIZE), 0);
            }

            let read = self.artifact.read_chunk_to(&mut self.buf[self.end..])?;
            if read == 0 {
                return Err(AppError::CorruptArchive(
                    "LZ: неожиданный конец потока при чтении токена".to_string(),
                ));
            }
            self.end += read;
        }
        Ok(())
    }

    pub fn read_u8(&mut self) -> Result<u8, AppError> {
        self.fill(1)?;
        let value = self.buf[self.pos];
        self.pos += 1;
        Ok(value)
    }

    pub fn read_u16_le(&mut self) -> Result<u16, AppError> {
        self.fill(2)?;
        let value = u16::from_le_bytes([self.buf[self.pos], self.buf[self.pos + 1]]);
        self.pos += 2;
        Ok(value)
    }

    /// Добавляет ровно `n` байт в существующий выходной буфер без
    /// промежуточного Vec и без аллокации на каждый литеральный блок.
    pub fn append_exact(&mut self, mut n: usize, out: &mut Vec<u8>) -> Result<(), AppError> {
        out.reserve(n);
        while n > 0 {
            if self.pos == self.end {
                self.fill(1)?;
            }
            let take = n.min(self.end - self.pos);
            out.extend_from_slice(&self.buf[self.pos..self.pos + take]);
            self.pos += take;
            n -= take;
        }
        Ok(())
    }
}


/// Фиксированное кольцевое окно декодера.
pub struct DecodeHistory {
    data: Vec<u8>,
    write: usize,
    len: usize,
}

impl DecodeHistory {
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0);
        Self { data: vec![0; capacity], write: 0, len: 0 }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn push(&mut self, byte: u8) {
        self.data[self.write] = byte;
        self.write += 1;
        if self.write == self.data.len() {
            self.write = 0;
        }
        self.len = (self.len + 1).min(self.data.len());
    }

    pub fn extend(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.push(byte);
        }
    }

    pub fn copy_match(
        &mut self,
        offset: usize,
        length: usize,
        out: &mut Vec<u8>,
    ) -> Result<(), AppError> {
        if offset == 0 || offset > self.len {
            return Err(AppError::CorruptArchive(
                "LZ: некорректный offset ссылки назад".to_string(),
            ));
        }

        out.reserve(length);
        for _ in 0..length {
            let index = (self.write + self.data.len() - offset) % self.data.len();
            let byte = self.data[index];
            self.push(byte);
            out.push(byte);
        }
        Ok(())
    }
}
