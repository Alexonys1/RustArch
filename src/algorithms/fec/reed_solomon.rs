use crate::error::AppError;
use crate::archiver::Artifact;
use super::FecId;

use super::{ErrorCorrectionCode, FecReport};


/// Порождающий многочлен GF(256): x^8 + x^4 + x^3 + x^2 + 1 = 285.
const GF_GENERATOR_POLY: u16 = 0x11D;

const OUTPUT_FLUSH_SIZE: usize = 512 * 1024;

const DATA_SYMBOLS: usize = 223;

const PARITY_SYMBOLS: usize = 32;

const CODEWORD_SYMBOLS: usize = DATA_SYMBOLS + PARITY_SYMBOLS;


/// Поле Галуа GF(256) с примитивным элементом 2.
struct Gf256 {
    /// Удвоенная (512 вместо 256) таблица степеней двойки
    exp: [u8; 512],
    /// Таблица логарифмов по основанию 2.
    log: [u8; 256],
}

impl Gf256 {
    // TODO: заменить на фиксированную таблицу на этапе компиляции
    
    /// Инициализация таблиц степеней и логарифмов:
    fn new() -> Self {
        let mut exp = [0u8; 512];
        let mut log = [0u8; 256];

        let mut x: u16 = 1;
        for i in 0..255usize {
            exp[i] = x as u8;
            log[x as usize] = i as u8;
            x <<= 1;
            if x & 0x100 != 0 {
                x ^= GF_GENERATOR_POLY;
            }
        }
        
        // Дублирование таблицы, т.к. числа дальше просто повторяются:
        for i in 255..512 {
            exp[i] = exp[i - 255];
        }

        Self { exp, log }
    }

    /// Умножение в поле Галуа
    #[inline]
    fn mul(&self, a: u8, b: u8) -> u8 {
        if a == 0 || b == 0 {
            0
        } else {
            self.exp[self.log[a as usize] as usize + self.log[b as usize] as usize]
        }
    }

    /// Деление в поле Галуа
    #[inline]
    fn div(&self, a: u8, b: u8) -> u8 {
        debug_assert!(b != 0, "деление на ноль в GF(256)");
        if a == 0 {
            0
        } else {
            let la = self.log[a as usize] as i32;
            let lb = self.log[b as usize] as i32;
            let mut diff = la - lb;
            if diff < 0 {
                diff += 255;
            }
            self.exp[diff as usize]
        }
    }

    /// Возведение в степень в поле Галуа
    #[inline]
    fn pow(&self, a: u8, n: i32) -> u8 {
        if a == 0 {
            return if n == 0 { 1 } else { 0 };
        }
        let l = self.log[a as usize] as i64;
        let mut e = (l * n as i64).rem_euclid(255);
        if e < 0 {
            e += 255;
        }
        self.exp[e as usize]
    }
}


/// Умножение многочленов
/// Многчлены хранятся с младшей степени
fn poly_mul(gf: &Gf256, a: &[u8], b: &[u8]) -> Vec<u8> {
    let mut result = vec![0u8; a.len() + b.len() - 1];
    for (i, &ai) in a.iter().enumerate() {
        if ai == 0 {
            continue;
        }
        for (j, &bj) in b.iter().enumerate() {
            if bj == 0 {
                continue;
            }
            result[i + j] ^= gf.mul(ai, bj);
        }
    }
    result
}

/// Вычисление значения многочлена
fn poly_eval_ascending(gf: &Gf256, poly: &[u8], x: u8) -> u8 {
    let mut y = 0u8;
    let mut x_pow = 1u8;
    for &c in poly {
        y ^= gf.mul(c, x_pow);
        x_pow = gf.mul(x_pow, x);
    }
    y
}

/// Вычисление значения многочлена заданного старшей степенью
fn poly_eval_descending(gf: &Gf256, poly: &[u8], x: u8) -> u8 {
    let mut y = poly[0];
    for &c in &poly[1..] {
        y = gf.mul(y, x) ^ c;
    }
    y
}

/// Взятие формальной производной многочлена
fn poly_formal_derivative(poly: &[u8]) -> Vec<u8> {
    if poly.len() <= 1 {
        return vec![0];
    }
    let mut result = vec![0u8; poly.len() - 1];
    let mut i = 1;
    while i < poly.len() {
        result[i - 1] = poly[i];
        i += 2;
    }
    result
}

//TODO: Заменить на известный для GF(256) и 285
//      оставить только для нестандартного блока
/// Вычисление порождающего многочлена (старшая степень).
fn build_generator(gf: &Gf256, nsym: usize) -> Vec<u8> {
    let mut g = vec![1u8];
    for i in 0..nsym {
        g = poly_mul_descending(gf, &g, &[1, gf.pow(2, i as i32)]);
    }
    g
}

/// Умножение для построения порождающего многочлена
fn poly_mul_descending(gf: &Gf256, a: &[u8], b: &[u8]) -> Vec<u8> {
    poly_mul(gf, a, b)
}

/// Кодирует один блок данных
/// Обычно RS(255, 223)
fn rs_encode_block(gf: &Gf256, data: &[u8], genz: &[u8]) -> Vec<u8> {
    let nsym = genz.len() - 1;
    let mut msg_out = vec![0u8; data.len() + nsym];
    msg_out[..data.len()].copy_from_slice(data);

    for i in 0..data.len() {
        let coef = msg_out[i];
        if coef != 0 {
            for (j, &g) in genz.iter().enumerate() {
                msg_out[i + j] ^= gf.mul(g, coef);
            }
        }
    }

    // В процессе деления область данных в `msg_out` перезаписывается
    // промежуточными значениями - восстанавливаем её оригинальными байтами
    msg_out[..data.len()].copy_from_slice(data);
    msg_out
}


/// Алгоритм Берлекэмпа-Мэсси
/// Находит многочлен-локатор ошибок
///
/// На каждом шаге считается невязка между текущим приближением локатора и очередным синдромом;
fn berlekamp_massey(gf: &Gf256, syndromes: &[u8]) -> (Vec<u8>, usize) {
    let nsym = syndromes.len();

    // lambda заведомо длиннее чтобы хранить переполнение степени синдромов
    let mut lambda = vec![0u8; nsym + 1];
    let mut b = vec![0u8; nsym + 1];
    lambda[0] = 1;
    b[0] = 1;

    let mut l: usize = 0;
    let mut m: usize = 1;
    let mut b_coef: u8 = 1;

    for n in 0..nsym {
        let mut delta = syndromes[n];
        for i in 1..=l {
            delta ^= gf.mul(lambda[i], syndromes[n - i]);
        }

        if delta == 0 {
            m += 1;
        } else if 2 * l <= n {
            let prev_lambda = lambda.clone();
            let scale = gf.div(delta, b_coef);
            let upper = b.len().min(lambda.len().saturating_sub(m));
            for j in 0..upper {
                lambda[j + m] ^= gf.mul(scale, b[j]);
            }
            l = n + 1 - l;
            b = prev_lambda;
            b_coef = delta;
            m = 1;
        } else {
            let scale = gf.div(delta, b_coef);
            let upper = b.len().min(lambda.len().saturating_sub(m));
            for j in 0..upper {
                lambda[j + m] ^= gf.mul(scale, b[j]);
            }
            m += 1;
        }
    }

    lambda.truncate(l + 1);
    (lambda, l)
}


/// Перебор Ченя
/// Полный перебор всех байт кодового слова в поисках корней полинома локатора.
///
/// Возвращает по каждой найденной ошибке тройку `(idx, X_l^-1, X_l)` - необходимо дальше для формулы Форни.
fn find_errors(gf: &Gf256, lambda: &[u8], n: usize) -> Vec<(usize, u8, u8)> {
    let mut result = Vec::new();
    for idx in 0..n {
        let exponent = (n - 1 - idx) as i32;
        let root = gf.pow(2, -exponent);
        if poly_eval_ascending(gf, lambda, root) == 0 {
            let x_l = gf.pow(2, exponent);
            result.push((idx, root, x_l));
        }
    }
    result
}



enum BlockStatus {
    Ok,
    Corrected,
    Uncorrectable,
}


/// Декодирование с поиском и исправлением ошибок
fn decode_block(gf: &Gf256, codeword: &mut [u8], nsym: usize) -> BlockStatus {
    // Подсчет полинома синдромов
    let syndromes: Vec<u8> = (0..nsym)
        .map(|i| poly_eval_descending(gf, codeword, gf.pow(2, i as i32)))
        .collect();

    // Если синдром ноль - данные корректны
    if syndromes.iter().all(|&s| s == 0) {
        return BlockStatus::Ok;
    }

    // Локализуем ошибки через Берлекэмпа-Мэсси
    let (lambda, errors_count) = berlekamp_massey(gf, &syndromes);
    if errors_count == 0 || 2 * errors_count > nsym {
        return BlockStatus::Uncorrectable;
    }

    // Находим полином ошибок перебором Ченя
    let errors = find_errors(gf, &lambda, codeword.len());
    if errors.len() != errors_count {
        return BlockStatus::Uncorrectable;
    }

    // Строим полином оценщик ошибок
    let omega: Vec<u8> = poly_mul(gf, &syndromes, &lambda)
        .into_iter()
        .take(nsym)
        .collect();
    let lambda_deriv = poly_formal_derivative(&lambda);

    for (idx, root, x_l) in &errors {
        let omega_val = poly_eval_ascending(gf, &omega, *root);
        let deriv_val = poly_eval_ascending(gf, &lambda_deriv, *root);
        if deriv_val == 0 {
            return BlockStatus::Uncorrectable;
        }
        // Находим полином амплитуд по формуле Форни
        let magnitude = gf.mul(*x_l, gf.div(omega_val, deriv_val));
        codeword[*idx] ^= magnitude;
    }

    // Повторно проверяем синдромы уже исправленного слова.
    let check_ok = (0..nsym)
        .all(|i| poly_eval_descending(gf, codeword, gf.pow(2, i as i32)) == 0);

    if check_ok {
        BlockStatus::Corrected
    } else {
        BlockStatus::Uncorrectable
    }
}


/// Функция чтения блока данных из артифакта
fn fill_exact(artifact: &mut Artifact, buf: &mut [u8]) -> Result<(), AppError> {
    let mut filled = 0;
    while filled < buf.len() {
        let read = artifact.read_chunk_to(&mut buf[filled..])?;
        if read == 0 {
            return Err(AppError::Fec(
                "Reed-Solomon: неожиданный конец потока при чтении блока".to_string(),
            ));
        }
        filled += read;
    }
    Ok(())
}


pub struct ReedSolomonCode;

impl ErrorCorrectionCode for ReedSolomonCode {
    fn encode(&self, mut artifact: Artifact) -> Result<Artifact, AppError> {
        let gf = Gf256::new();
        let genz = build_generator(&gf, PARITY_SYMBOLS);

        let mut output = Artifact::new_with_temp_file_suffix(&artifact, "fec_encoded");

        // Буфер ровно на один блок данных
        let mut data_buf = [0u8; DATA_SYMBOLS];
        let mut filled = 0usize;
        let mut out_buf: Vec<u8> = Vec::with_capacity(OUTPUT_FLUSH_SIZE);

        while let Some(chunk) = artifact.next_chunk()? {
            let mut chunk_pos = 0;
            while chunk_pos < chunk.len() {
                let take = (DATA_SYMBOLS - filled).min(chunk.len() - chunk_pos);
                data_buf[filled..filled + take].copy_from_slice(&chunk[chunk_pos..chunk_pos + take]);
                filled += take;
                chunk_pos += take;

                // Проверка на остаток файла
                if filled == DATA_SYMBOLS {
                    let codeword = rs_encode_block(&gf, &data_buf, &genz);
                    out_buf.extend_from_slice(&codeword);
                    filled = 0;

                    if out_buf.len() >= OUTPUT_FLUSH_SIZE {
                        output.write_chunk_from(&out_buf)?;
                        out_buf.clear();
                    }
                }
            }
        }

        if filled > 0 {
            // Дополняем укороченный RS-блок
            let codeword = rs_encode_block(&gf, &data_buf[..filled], &genz);
            out_buf.extend_from_slice(&codeword);
        }

        // Принудительный сброс буфера
        if !out_buf.is_empty() {
            output.write_chunk_from(&out_buf)?;
        }

        Ok(output)
    }

    fn decode(&self, mut artifact: Artifact) -> Result<(Artifact, FecReport), AppError> {
        let encoded_size = artifact.get_payload_size();
        let full_blocks = encoded_size / CODEWORD_SYMBOLS;
        let shortened_size = encoded_size % CODEWORD_SYMBOLS;

        if shortened_size != 0 && shortened_size <= PARITY_SYMBOLS {
            return Err(AppError::Fec(format!(
                "Reed-Solomon: некорректная длина потока {encoded_size}: остаток блока {shortened_size} должен быть 0 или {}..={} байт",
                PARITY_SYMBOLS + 1,
                CODEWORD_SYMBOLS - 1,
            )));
        }

        let blocks_count = full_blocks + usize::from(shortened_size != 0);

        let gf = Gf256::new();

        let mut output = Artifact::new_with_temp_file_suffix(&artifact, "fec_decoded");
        let mut report = FecReport::default();

        let mut out_buf: Vec<u8> = Vec::with_capacity(OUTPUT_FLUSH_SIZE);
        let mut block_buf = vec![0u8; CODEWORD_SYMBOLS];

        for block_index in 0..blocks_count {
            let block_size = if block_index < full_blocks {
                CODEWORD_SYMBOLS
            } else {
                shortened_size
            };
            let data_size = block_size - PARITY_SYMBOLS;
            block_buf.resize(block_size, 0);
            fill_exact(&mut artifact, &mut block_buf)?;

            let status = decode_block(&gf, &mut block_buf, PARITY_SYMBOLS);
            report.blocks_processed += 1;
            match status {
                BlockStatus::Ok => {}
                BlockStatus::Corrected => report.blocks_corrected += 1,
                BlockStatus::Uncorrectable => {
                    return Err(AppError::Fec(format!(
                        "Reed-Solomon: блок {} из {} содержит неисправимые ошибки",
                        block_index + 1,
                        blocks_count,
                    )));
                }
            }

            out_buf.extend_from_slice(&block_buf[..data_size]);

            if out_buf.len() >= OUTPUT_FLUSH_SIZE {
                output.write_chunk_from(&out_buf)?;
                out_buf.clear();
            }
        }

        if !out_buf.is_empty() {
            output.write_chunk_from(&out_buf)?;
        }

        Ok((output, report))
    }

    fn id(&self) -> FecId {
        FecId::ReedSolomon
    }
}
