use crate::archiver::Artifact;
use crate::error::AppError;
use super::core::OUTPUT_FLUSH_SIZE;


pub struct StreamingBitWriter<'a> {
    output: &'a mut Artifact,
    buf: Vec<u8>,
    acc: u64,
    nbits: u32,
}


impl<'a> StreamingBitWriter<'a> {
    pub fn new(output: &'a mut Artifact) -> Self {
        Self { output, buf: Vec::with_capacity(OUTPUT_FLUSH_SIZE), acc: 0, nbits: 0 }
    }

    pub fn push_code(&mut self, code: u32, len: u8) -> Result<(), AppError> {
        let len = len as u32;
        // Место в аккумуляторе для нового кода начинается сразу после уже
        // накопленных nbits бит (они занимают верхние разряды)
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

    pub fn finish(mut self) -> Result<(), AppError> {
        if self.nbits > 0 {
            self.buf.push((self.acc >> 56) as u8);
        }

        if !self.buf.is_empty() {
            self.output.write_chunk_from(&self.buf)?;
        }

        Ok(())
    }
}


/// Скользящее окно бит поверх `Artifact::next_chunk()`. Не хранит `Cow`
/// как поле (см. пояснение в начале ответа) - вместо этого при переходе
/// на новый чанк берёт владение через `.into_owned()`: бесплатно для
/// File/FileWindow (там и так `Cow::Owned`), одно копирование на границу
/// чанка для Memory (не на байт).
pub struct BitWindow<'a> {
    artifact: &'a mut Artifact,
    chunk: Vec<u8>,
    chunk_pos: usize,
    acc: u32,   // валидные биты - в СТАРШИХ разрядах
    nbits: u32,
}

impl<'a> BitWindow<'a> {
    pub fn new(artifact: &'a mut Artifact) -> Self {
        Self { artifact, chunk: Vec::new(), chunk_pos: 0, acc: 0, nbits: 0 }
    }

    /// Догружает аккумулятор минимум до `want` валидных бит. Если реальные
    /// данные кончились раньше - оставшиеся "виртуальные" биты трактуются
    /// как нулевой паддинг (они и так уже нули в `acc`) - безопасно, т.к.
    /// решение "сколько символов декодировать" принимается по
    /// `original_size`, а не по количеству реально прочитанных бит.
    pub fn fill(&mut self, want: u32) -> Result<(), AppError> {
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

    pub fn peek(&self, n: u32) -> u32 {
        self.acc >> (32 - n)
    }

    pub fn consume(&mut self, n: u32) {
        self.acc <<= n;
        self.nbits = self.nbits.saturating_sub(n);
    }
}
