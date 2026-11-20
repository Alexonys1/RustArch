use std::fmt;
use std::fs::File;
use std::io::{self, Write};
use std::sync::{Mutex, MutexGuard};


const WRITE_TO_FILE: bool = true;
/// Нужно обязательно учитывать относительные пути при запуске программы!
const FILE_PATH: &str = "./test_data_for_removing/statistics.txt";
static OUTPUT: Mutex<Option<File>> = Mutex::new(None);


macro_rules! statistics_println {
    ($($args:tt)+) => {{
        let _ = $crate::archiver::statistics::statistics_println_inner(format_args!($($args)+));
    }};
}

pub(crate) use statistics_println;


pub(crate) fn statistics_println_inner(args: fmt::Arguments<'_>) -> io::Result<()> {
    if !WRITE_TO_FILE {
        return writeln!(io::stdout().lock(), "{args}");
    }

    let mut output: MutexGuard<Option<File>> = OUTPUT
        .lock()
        .map_err(|_| io::Error::other("Мьютекс статистики отравлен!"))?;

    let file: &mut File = match &mut *output {
        Some(file) => file,
        slot @ None => slot.insert(File::create(FILE_PATH)?),
    };

    writeln!(file, "{args}")
}
