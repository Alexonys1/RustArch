use RustArch::cli::{ CLICommand, run_pack, run_unpack };
use RustArch::algorithms::{ CipherId, CompressionId, FecId, PipelineSettings };


fn main() {
    /*let args: Vec<String> = std::env::args().skip(1).collect();

    let cli_command = match parse_args(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Ошибка: {e}");
            print_help();
            std::process::exit(1);
        }
    };
     */

    // ========== Для теста: ==========

    enum TestCLICommand { Pack, Unpack, TestGrouping }
    let test_choice = TestCLICommand::Pack; // !МЕНЯТЬ ЗДЕСЬ!   <==========================
    const SOURCE_PATH:         &str = r"C:\Users\alex\Desktop\Тестовые данные для архиватора\Смешанное\Фото + Текст";
    const TARGET_ARCHIVE_PATH: &str = r".\test_data_for_removing\study.arch";
    const UNPACK_PATH:         &str = r".\test_data_for_removing\unpacked";


    let cli_command: CLICommand = match test_choice {
        TestCLICommand::Pack => CLICommand::Pack {
            source_path: SOURCE_PATH.into(), // Важно, что эти относительные пути именно строки,
            target_archive_path: TARGET_ARCHIVE_PATH.into(), // которые можно менять
            settings: PipelineSettings {
                compression: CompressionId::Deflate,
                cipher: CipherId::NoCipher,
                fec: FecId::NoFec,
            },
            encode_key: (1..=255).collect(), // Подбирать 255-БАЙТНЫЙ ключ полным перебором - это увлекательное дело!
        },

        TestCLICommand::Unpack => CLICommand::Unpack {
            source_path: TARGET_ARCHIVE_PATH.into(),
            target_unpack_path: UNPACK_PATH.into(),
            decode_key: (1..=255).collect(),
        },

        TestCLICommand::TestGrouping => {
            use RustArch::archiver::{ group_files_for_workers, walk_directory_or_file, WalkedFile };

            let start = std::time::Instant::now();
            let walked_files: Vec<WalkedFile> = walk_directory_or_file(SOURCE_PATH).unwrap().files;
            let groups: Vec<Vec<&WalkedFile>> = group_files_for_workers(&walked_files, 16).unwrap();
            let elapsed = start.elapsed();

            println!("Walked {:#?} files", groups);

            for group in groups {
                println!(
                    "Files in group: {}\tTotal group size: {}",
                    group.len(),
                    group.iter().map(|f| f.get_size().unwrap()).sum::<u64>()
                );
            }

            println!("Time of walking and grouping: {}ms", elapsed.as_millis());

            std::process::exit(0);
        }

    };

    let start = std::time::Instant::now();
    let archive_result = match cli_command {
        CLICommand::Pack {
            source_path,
            target_archive_path,
            settings,
            encode_key,
        } => run_pack(&source_path, &target_archive_path, settings, &encode_key),

        CLICommand::Unpack {
            source_path,
            target_unpack_path,
            decode_key,
        } => run_unpack(&source_path, &target_unpack_path, &decode_key),

        _ => todo!(),
    };
    let elapsed = start.elapsed();

    let archive_size_in_gb: f64 = std::fs::metadata(r"C:\Users\alex\RustroverProjects\RustArch\test_data_for_removing\study.arch")
        .unwrap().len() as f64 / 1024_f64.powi(3);

    println!("\n===> TOTAL TIME: {}ms", elapsed.as_millis());
    println!(  "===> TOTAL SIZE: {:.2}GB", archive_size_in_gb);

    if let Err(e) = archive_result {
        eprintln!("Ошибка: {e}");
        std::process::exit(1);
    }

    //let _ = gui::run();
}
