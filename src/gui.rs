//! Модуль `gui` — минималистичное окно взаимодействия для утилиты
//! обработки файлов (сжатие / шифрование / помехоустойчивое кодирование),
//! выполненное в стиле классических ZIP-архиваторов.
//!
//! Модуль самодостаточен: достаточно вызвать [`run`], чтобы открыть
//! главное окно приложения.
//!
//! Зависимости (Cargo.toml):
//! ```toml
//! [dependencies]
//! eframe = "0.28"
//! egui   = "0.28"
//! rfd    = "0.14"
//! ```

use eframe::egui;

// ---------------------------------------------------------------------
//  Перечисления алгоритмов — у каждого есть вариант "без применения"
// ---------------------------------------------------------------------

/// Алгоритм сжатия данных.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompressionAlgorithm {
    /// Сжатие не применяется.
    #[default]
    None,
    Deflate,
    Lzma,
    Zstd,
    Bzip2,
}

impl CompressionAlgorithm {
    const ALL: [Self; 5] = [
        Self::None,
        Self::Deflate,
        Self::Lzma,
        Self::Zstd,
        Self::Bzip2,
    ];

    fn label(&self) -> &'static str {
        match self {
            Self::None => "Без сжатия",
            Self::Deflate => "Deflate",
            Self::Lzma => "LZMA",
            Self::Zstd => "Zstandard",
            Self::Bzip2 => "BZip2",
        }
    }
}

/// Алгоритм шифрования данных.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EncryptionAlgorithm {
    /// Шифрование не применяется.
    #[default]
    None,
    Aes256,
    ChaCha20,
    Twofish,
}

impl EncryptionAlgorithm {
    const ALL: [Self; 4] = [Self::None, Self::Aes256, Self::ChaCha20, Self::Twofish];

    fn label(&self) -> &'static str {
        match self {
            Self::None => "Без шифрования",
            Self::Aes256 => "AES-256",
            Self::ChaCha20 => "ChaCha20",
            Self::Twofish => "Twofish",
        }
    }
}

/// Алгоритм защиты от помех (помехоустойчивое кодирование).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NoiseProtectionAlgorithm {
    /// Защита от помех не применяется.
    #[default]
    None,
    Crc32,
    Hamming,
    ReedSolomon,
}

impl NoiseProtectionAlgorithm {
    const ALL: [Self; 4] = [Self::None, Self::Crc32, Self::Hamming, Self::ReedSolomon];

    fn label(&self) -> &'static str {
        match self {
            Self::None => "Без защиты",
            Self::Crc32 => "Контрольная сумма (CRC32)",
            Self::Hamming => "Код Хэмминга",
            Self::ReedSolomon => "Рида — Соломона",
        }
    }
}

/// Что именно выбирает пользователь через проводник: файл или директория.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum SourceKind {
    #[default]
    File,
    Directory,
}

// ---------------------------------------------------------------------
//  Тонкие настройки (открываются отдельным окном)
// ---------------------------------------------------------------------

/// Тонкие настройки обработки: уровень компрессии, объём памяти,
/// количество потоков процессора и т. п.
#[derive(Debug, Clone)]
pub struct AdvancedSettings {
    /// Уровень сжатия (1 — быстро/слабо, 9 — медленно/сильно).
    pub compression_level: u8,
    /// Размер блока обработки данных, МиБ.
    pub block_size_mib: u32,
    /// Максимальный объём оперативной памяти, выделяемой под задачу, МиБ.
    pub max_memory_mib: u32,
    /// Количество потоков процессора, используемых при обработке.
    pub cpu_threads: u32,
    /// Избыточность помехоустойчивого кода в процентах (актуально,
    /// если выбран алгоритм защиты от помех).
    pub redundancy_percent: u8,
}

impl Default for AdvancedSettings {
    fn default() -> Self {
        Self {
            compression_level: 5,
            block_size_mib: 4,
            max_memory_mib: 512,
            cpu_threads: std::thread::available_parallelism()
                .map(|n| n.get() as u32)
                .unwrap_or(4),
            redundancy_percent: 10,
        }
    }
}

// ---------------------------------------------------------------------
//  Основное состояние приложения
// ---------------------------------------------------------------------

pub struct ArchiverApp {
    /// Путь, выбранный пользователем через системный проводник.
    source_path: Option<std::path::PathBuf>,
    /// Режим выбора: файл или директория.
    source_kind: SourceKind,

    compression: CompressionAlgorithm,
    encryption: EncryptionAlgorithm,
    noise_protection: NoiseProtectionAlgorithm,

    /// Тонкие настройки, редактируемые в отдельном окне.
    settings: AdvancedSettings,
    /// Флаг видимости окна настроек.
    settings_window_open: bool,
    /// Черновик настроек, редактируемый в открытом окне (применяется
    /// по кнопке «Сохранить», отбрасывается по «Отмена»).
    settings_draft: AdvancedSettings,

    /// Логический максимум потоков CPU, доступных на машине —
    /// используется как верхняя граница слайдера.
    max_available_threads: u32,
}

impl Default for ArchiverApp {
    fn default() -> Self {
        let settings = AdvancedSettings::default();
        let max_available_threads = std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(4)
            .max(1);
        Self {
            source_path: None,
            source_kind: SourceKind::File,
            compression: CompressionAlgorithm::default(),
            encryption: EncryptionAlgorithm::default(),
            noise_protection: NoiseProtectionAlgorithm::default(),
            settings_draft: settings.clone(),
            settings,
            settings_window_open: false,
            max_available_threads,
        }
    }
}

impl ArchiverApp {
    pub fn new() -> Self {
        Self::default()
    }

    /// Открыть системный проводник для выбора файла.
    fn pick_file(&mut self) {
        if let Some(path) = rfd::FileDialog::new().set_title("Выбор файла").pick_file() {
            self.source_path = Some(path);
            self.source_kind = SourceKind::File;
        }
    }

    /// Открыть системный проводник для выбора директории.
    fn pick_directory(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Выбор директории")
            .pick_folder()
        {
            self.source_path = Some(path);
            self.source_kind = SourceKind::Directory;
        }
    }

    /// Отрисовка блока радиокнопок с единым заголовком-рамкой —
    /// общий вид для всех трёх групп алгоритмов (минималистичный стиль).
    fn algorithm_group<T: PartialEq + Copy>(
        ui: &mut egui::Ui,
        title: &str,
        options: &[T],
        selected: &mut T,
        label: impl Fn(&T) -> &'static str,
    ) {
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::symmetric(10.0, 8.0))
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(title).strong());
                    ui.add_space(4.0);
                    for opt in options {
                        ui.radio_value(selected, *opt, label(opt));
                    }
                });
            });
    }

    /// Отдельное окно тонких настроек: параметры алгоритмов,
    /// максимум памяти и количество потоков CPU.
    fn show_settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.settings_window_open;
        egui::Window::new("Настройки")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_min_width(320.0);

                ui.label(egui::RichText::new("Тонкая настройка алгоритмов").strong());
                ui.add_space(6.0);

                egui::Grid::new("fine_tuning_grid")
                    .num_columns(2)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("Уровень сжатия:");
                        ui.add(
                            egui::Slider::new(&mut self.settings_draft.compression_level, 1..=9)
                                .text("1 — быстро, 9 — сильно"),
                        );
                        ui.end_row();

                        ui.label("Размер блока (МиБ):");
                        ui.add(egui::Slider::new(
                            &mut self.settings_draft.block_size_mib,
                            1..=256,
                        ));
                        ui.end_row();

                        ui.label("Избыточность кода (%):");
                        ui.add(egui::Slider::new(
                            &mut self.settings_draft.redundancy_percent,
                            0..=50,
                        ));
                        ui.end_row();
                    });

                ui.separator();
                ui.label(egui::RichText::new("Аппаратные ресурсы").strong());
                ui.add_space(6.0);

                egui::Grid::new("resources_grid")
                    .num_columns(2)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("Макс. объём памяти (МиБ):");
                        ui.add(
                            egui::Slider::new(
                                &mut self.settings_draft.max_memory_mib,
                                64..=16384,
                            )
                            .logarithmic(true),
                        );
                        ui.end_row();

                        ui.label("Потоки процессора:");
                        ui.add(egui::Slider::new(
                            &mut self.settings_draft.cpu_threads,
                            1..=self.max_available_threads,
                        ));
                        ui.end_row();
                    });

                ui.add_space(10.0);
                ui.separator();

                ui.horizontal(|ui| {
                    if ui.button("Сохранить").clicked() {
                        self.settings = self.settings_draft.clone();
                        self.settings_window_open = false;
                    }
                    if ui.button("Отмена").clicked() {
                        // Откатываем черновик к последним сохранённым значениям.
                        self.settings_draft = self.settings.clone();
                        self.settings_window_open = false;
                    }
                    if ui.button("Сбросить по умолчанию").clicked() {
                        self.settings_draft = AdvancedSettings::default();
                    }
                });
            });

        // Синхронизируем состояние, если окно закрыли крестиком.
        if !open {
            self.settings_draft = self.settings.clone();
        }
        self.settings_window_open = open;
    }
}

impl eframe::App for ArchiverApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Минималистичная светлая тема, без лишних теней и скруглений —
        // в духе классических окон архиваторов.
        let mut style = (*ctx.style()).clone();
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        ctx.set_style(style);

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(4.0);
            ui.heading("Обработка файлов");
            ui.separator();

            // --- Блок выбора источника ---
            ui.label("Источник:");
            ui.horizontal(|ui| {
                let path_text = self
                    .source_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "— не выбрано —".to_owned());

                ui.add(
                    egui::TextEdit::singleline(&mut path_text.clone())
                        .desired_width(300.0)
                        .interactive(false),
                );

                if ui.button("Файл…").clicked() {
                    self.pick_file();
                }
                if ui.button("Папка…").clicked() {
                    self.pick_directory();
                }
            });
            if let Some(path) = &self.source_path {
                let kind = match self.source_kind {
                    SourceKind::File => "файл",
                    SourceKind::Directory => "директория",
                };
                ui.label(
                    egui::RichText::new(format!("Выбран {kind}: {}", path.display()))
                        .small()
                        .weak(),
                );
            }

            ui.add_space(10.0);
            ui.separator();
            ui.add_space(4.0);

            // --- Три группы радиокнопок в один ряд ---
            ui.columns(3, |cols| {
                Self::algorithm_group(
                    &mut cols[0],
                    "Сжатие",
                    &CompressionAlgorithm::ALL,
                    &mut self.compression,
                    CompressionAlgorithm::label,
                );
                Self::algorithm_group(
                    &mut cols[1],
                    "Шифрование",
                    &EncryptionAlgorithm::ALL,
                    &mut self.encryption,
                    EncryptionAlgorithm::label,
                );
                Self::algorithm_group(
                    &mut cols[2],
                    "Защита от помех",
                    &NoiseProtectionAlgorithm::ALL,
                    &mut self.noise_protection,
                    NoiseProtectionAlgorithm::label,
                );
            });

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(6.0);

            // --- Нижняя панель: настройки + запуск ---
            ui.horizontal(|ui| {
                if ui.button("⚙  Настройки…").clicked() {
                    self.settings_window_open = true;
                }

                ui.add_space(ui.available_width() - 180.0);

                let can_start = self.source_path.is_some();
                if ui
                    .add_enabled(can_start, egui::Button::new("Начать обработку"))
                    .clicked()
                {
                    // Точка интеграции с бизнес-логикой обработки файлов.
                    println!(
                        "Запуск: путь={:?}, сжатие={:?}, шифрование={:?}, помехозащита={:?}, настройки={:?}",
                        self.source_path,
                        self.compression,
                        self.encryption,
                        self.noise_protection,
                        self.settings
                    );
                }
            });
        });

        if self.settings_window_open {
            self.show_settings_window(ctx);
        }
    }
}

/// Запускает главное окно приложения.
///
/// Вызывается, например, из `main.rs`:
/// ```no_run
/// mod gui;
/// fn main() -> eframe::Result<()> {
///     gui::run()
/// }
/// ```
pub fn run() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([560.0, 340.0])
            .with_min_inner_size([480.0, 300.0])
            .with_resizable(true),
        ..Default::default()
    };

    eframe::run_native(
        "Архиватор",
        options,
        Box::new(|_cc| Ok(Box::new(ArchiverApp::new()))),
    )
}
