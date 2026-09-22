use std::path::Path;
use std::time::Instant;

use colored::Colorize;

use rustarch::algorithms::{CipherId, CompressionId, FecId, PipelineSettings};
use rustarch::cli::{CLICommand, parse_args, run_command};
use rustarch::error::AppError;


#[allow(dead_code)]
#[derive(Clone, Copy)]
enum QuickRunMode {
    Pack,
    Unpack,
    TestGrouping,
    Release,
}

const QUICK_SETTINGS: PipelineSettings = PipelineSettings {
    compression: CompressionId::Deflate, // <========================================
    cipher: CipherId::NoCipher, // <========================================
    fec: FecId::NoFec, // <========================================
};


const QUICK_RUN_MODE: QuickRunMode = QuickRunMode::Release; // <==============================
const QUICK_SOURCE_PATH: &str = r"C:\Users\alex\Desktop\Тестовые данные для архиватора\Низкая энтропия\Текст";
const QUICK_ARCHIVE_PATH: &str = r".\test_data_for_removing\study.arch";
const QUICK_UNPACK_PATH: &str = r".\test_data_for_removing\unpacked";
const QUICK_WORKERS_FOR_GROUPING: usize = 16;


fn main() {
    #[cfg(windows)] // Для Коли. Чтобы даже в cmd.exe был цветной текст. До этого его не было
    let _ = colored::control::set_virtual_terminal(true);

    let cli_command: CLICommand = match QUICK_RUN_MODE {
        QuickRunMode::Pack => CLICommand::Pack {
            source_path: QUICK_SOURCE_PATH.into(),
            target_archive_path: QUICK_ARCHIVE_PATH.into(),
            settings: QUICK_SETTINGS,
            encode_key: (1..=255).collect(),
        },

        QuickRunMode::Unpack => CLICommand::Unpack {
            source_path: QUICK_ARCHIVE_PATH.into(),
            target_unpack_path: QUICK_UNPACK_PATH.into(),
            decode_key: (1..=255).collect(),
        },

        QuickRunMode::TestGrouping => {
            if let Err(error) = run_quick_grouping() {
                eprintln!("{} {error}", "Ошибка:".red().bold());
                std::process::exit(1);
            }
            return;
        }

        QuickRunMode::Release => match parse_args(std::env::args_os().skip(1)) {
            Ok(command) => command,
            Err(error) => {
                eprintln!("{} {error}", "Ошибка:".red().bold());
                eprintln!(
                    "{} {} {}",
                    "Используйте".dimmed(),
                    "'rustarch --help'".cyan(),
                    "для справки.".dimmed(),
                );
                std::process::exit(2);
            }
        }
    };

    let start = Instant::now();
    let execution_result = run_command(cli_command.clone());
    let elapsed = start.elapsed();

    println!(
        "\n{} {}",
        "===> TOTAL TIME:".bold(),
        format!("{}ms", elapsed.as_millis()).green(),
    );

    if let Err(error) = execution_result {
        eprintln!("{} {error}", "Ошибка:".red().bold());
        std::process::exit(1);
    }

    match cli_command {
        CLICommand::Pack { target_archive_path, .. } => {
            match std::fs::metadata(&target_archive_path) {
                Ok(metadata) => {
                    let size = format!(
                        "{:.2} GB ({} bytes)",
                        metadata.len() as f64 / 1024_f64.powi(3),
                        metadata.len(),
                    );
                    println!("{} {}", "===> TOTAL SIZE:".bold(), size.cyan());
                }

                Err(error) => eprintln!(
                    "{} не удалось прочитать размер {}: {error}",
                    "Предупреждение:".yellow().bold(),
                    format!("'{}'", target_archive_path.display()).cyan(),
                ),
            }
        }

        _ => { }
    }
}


// ===================== ВСПОМОГАТЕЛЬНЫЕ ФУНКЦИИ ДЛЯ БЫСТРОГО ТЕСТИРОВАНИЯ =========================
// Очень лень писать скрипты с разными командами для архиватора...
fn run_quick_grouping() -> Result<(), AppError> {
    use rustarch::archiver::{WalkedFile, group_files_for_workers, walk_directory_or_file};

    let started = Instant::now();
    let walked_files: Vec<WalkedFile> = walk_directory_or_file(Path::new(QUICK_SOURCE_PATH))?.files;
    let groups = group_files_for_workers(&walked_files, QUICK_WORKERS_FOR_GROUPING)?;

    println!("{} {}", "Групп:".bold(), groups.len().to_string().cyan());
    for (index, group) in groups.iter().enumerate() {
        let total_size = group.iter().try_fold(0u64, |sum, file| {
            file.get_size().map(|size| sum.saturating_add(size))
        })?;
        println!(
            "{} {}: {} файлов, {} байт",
            "Группа".bold(),
            format!("#{:<2}", index + 1).cyan(),
            format!("{:>6}", group.len()).cyan(),
            format!("{:>12}", total_size).cyan(),
        );
    }

    println!(
        "{} {}",
        "Время группировки:".bold(),
        format!("{}ms", started.elapsed().as_millis()).cyan(),
    );

    Ok(())
}
