use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use egui::*;
use egui_extras::{TableBuilder, Column};
use ue_mod_core::{LocresEntry, StagingArea};
use ue_mod_core::locres::{export_to_csv, import_from_csv, convert_to_traditional};
use ue_mod_core::zhconvert::{self, ZhConverter};

// ── 轉換方式 ─────────────────────────────────────────────────────────────────

#[derive(PartialEq, Clone, Copy, Default)]
pub enum ConvertMethod {
    #[default]
    OpenCC,
    ZhConvert,
}

// ── 背景轉換任務（繁化姬用）─────────────────────────────────────────────────

#[derive(Default)]
struct ConversionTask {
    completed: usize,
    total: usize,
    finished: bool,
    result: Option<Result<(usize, Vec<LocresEntry>), String>>,
}

// ── 主要狀態結構 ─────────────────────────────────────────────────────────────

pub struct LocresEditorState {
    pub current_path: Option<String>,
    /// 載入的 locres 所屬的容器（pak / utoc）絕對路徑。和 current_path 一起
    /// 組成 StagingArea.locres_edits 的鍵，避免同名跨容器互覆蓋。
    pub current_pak: Option<PathBuf>,
    pub entries: Vec<LocresEntry>,
    pub search: String,
    pub show_modified_only: bool,
    pub batch_find: String,
    pub batch_replace: String,
    pub status: String,
    pub is_loaded: bool,

    pub show_convert_dialog: bool,
    pub convert_method: ConvertMethod,
    pub opencc_mode: String,
    pub zh_converter: ZhConverter,
    conversion_task: Option<Arc<Mutex<ConversionTask>>>,
}

impl Default for LocresEditorState {
    fn default() -> Self {
        Self {
            current_path: None,
            current_pak: None,
            entries: vec![],
            search: String::new(),
            show_modified_only: false,
            batch_find: String::new(),
            batch_replace: String::new(),
            status: String::new(),
            is_loaded: false,
            show_convert_dialog: false,
            convert_method: ConvertMethod::default(),
            opencc_mode: "tw2".to_string(),
            zh_converter: ZhConverter::Taiwan,
            conversion_task: None,
        }
    }
}

impl LocresEditorState {
    pub fn open_locres(&mut self, path: &str, pak: &std::path::Path, aes_key: &str) {
        self.current_path = Some(path.to_string());
        self.current_pak = Some(pak.to_path_buf());
        self.entries.clear();
        self.is_loaded = false;
        self.status = "正在讀取 locres...".to_string();

        let key_opt = if aes_key.is_empty() { None } else { Some(aes_key) };
        match ue_mod_core::locres::read_locres_from_pak(pak, path, key_opt) {
            Ok(mut entries) => {
                for e in &mut entries {
                    e.value = e.value.replace("\r\n", "\n");
                }
                let count = entries.len();
                self.entries = entries;
                self.is_loaded = true;
                self.status = format!("已載入 {} 個條目", count);
            }
            Err(e) => {
                self.status = format!("讀取失敗: {}", e);
            }
        }
    }

    pub fn export_csv(&self, output: &std::path::Path) -> anyhow::Result<()> {
        export_to_csv(&self.entries, output)
    }

    pub fn import_csv(&mut self, path: &std::path::Path) -> anyhow::Result<()> {
        let count = import_from_csv(path, &mut self.entries)?;
        self.status = format!("匯入完成，成功匹配並修改 {} 個條目", count);
        Ok(())
    }

    pub fn run_opencc(&mut self) {
        match convert_to_traditional(&mut self.entries, &self.opencc_mode) {
            Ok(()) => {
                let count = self.entries.iter().filter(|e| e.is_modified()).count();
                self.status = format!("OpenCC 完成，{} 個條目已轉換", count);
            }
            Err(e) => {
                self.status = format!("OpenCC 失敗: {}", e);
            }
        }
        self.show_convert_dialog = false;
    }

    pub fn start_zhconvert(&mut self) {
        let task = Arc::new(Mutex::new(ConversionTask {
            total: self.entries.iter().filter(|e| !e.is_modified()).count(),
            ..Default::default()
        }));
        self.conversion_task = Some(Arc::clone(&task));

        let mut entries_clone = self.entries.clone();
        let converter = self.zh_converter;

        std::thread::spawn(move || {
            let result = zhconvert::convert_entries(
                &mut entries_clone,
                converter,
                |completed, total| {
                    if let Ok(mut t) = task.lock() {
                        t.completed = completed;
                        t.total = total;
                    }
                },
            );
            if let Ok(mut t) = task.lock() {
                t.finished = true;
                t.result = Some(match result {
                    Ok(count) => Ok((count, entries_clone)),
                    Err(e)    => Err(e.to_string()),
                });
            }
        });
    }

    fn poll_conversion_task(&mut self) -> Option<String> {
        let task_arc = self.conversion_task.as_ref()?.clone();
        let mut task = task_arc.lock().ok()?;
        if !task.finished { return None; }
        let result = task.result.take();
        drop(task);
        self.conversion_task = None;

        match result? {
            Ok((count, updated)) => {
                self.entries = updated;
                self.show_convert_dialog = false;
                Some(format!("繁化姬轉換完成，{} 個條目已轉換", count))
            }
            Err(e) => {
                self.show_convert_dialog = false;
                Some(format!("繁化姬失敗: {}", e))
            }
        }
    }

    pub fn batch_find_replace(&mut self) {
        if self.batch_find.is_empty() { return; }
        let mut count = 0;
        for entry in &mut self.entries {
            let source = entry.modified.as_ref().unwrap_or(&entry.value);
            if source.contains(&self.batch_find) {
                entry.modified = Some(source.replace(&self.batch_find, &self.batch_replace));
                count += 1;
            }
        }
        self.status = format!("已替換 {} 個條目", count);
    }

    pub fn reset_all(&mut self) {
        for entry in &mut self.entries { entry.modified = None; }
        self.status = "已重設所有修改".to_string();
    }

    pub fn commit_to_staging(&self, staging: &mut StagingArea) {
        if let (Some(path), Some(pak)) = (&self.current_path, &self.current_pak) {
            if self.entries.iter().any(|e| e.is_modified()) {
                staging.locres_edits.insert(
                    (pak.clone(), path.clone()),
                    self.entries.clone(),
                );
            }
        }
    }

    pub fn filtered_indices(&self) -> Vec<usize> {
        let search_lower = self.search.to_lowercase();
        self.entries.iter().enumerate()
            .filter(|(_, e)| {
                if self.show_modified_only && !e.is_modified() { return false; }
                if search_lower.is_empty() { return true; }
                e.key.to_lowercase().contains(&search_lower)
                    || e.value.to_lowercase().contains(&search_lower)
                    || e.effective_value().to_lowercase().contains(&search_lower)
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn modified_stats(&self) -> (bool, usize) {
        let count = self.entries.iter().filter(|e| e.is_modified()).count();
        (count > 0, count)
    }

    fn conversion_progress(&self) -> Option<(usize, usize)> {
        let task = self.conversion_task.as_ref()?.lock().ok()?;
        if task.finished { return None; }
        Some((task.completed, task.total))
    }
}

// ── OpenCC 模式清單 ───────────────────────────────────────────────────────────

const OPENCC_MODES: &[(&str, &str)] = &[
    ("tw2", "tw2 — 台灣繁體（常用詞）"),
    ("twp", "twp — 台灣繁體（含 IT 詞彙）"),
    ("tw",  "tw  — 台灣繁體（異體字）"),
    ("hk",  "hk  — 香港繁體（異體字）"),
];

fn opencc_mode_label(mode: &str) -> &'static str {
    OPENCC_MODES.iter().find(|(v, _)| *v == mode).map(|(_, l)| *l).unwrap_or("tw2")
}

// ── 主要 UI ──────────────────────────────────────────────────────────────────

pub fn show_locres_editor(ui: &mut Ui, state: &mut LocresEditorState, staging: &mut StagingArea) {
    if let Some(msg) = state.poll_conversion_task() {
        state.status = msg;
    }

    ui.horizontal(|ui| {
        ui.heading("在地化編輯器");
        ui.separator();
        if let Some(path) = &state.current_path {
            let name = path.rsplit('/').next().unwrap_or(path);
            ui.label(RichText::new(name).monospace().color(Color32::from_rgb(150, 200, 255)));
        } else {
            ui.colored_label(Color32::GRAY, "（未開啟檔案）");
        }
    });

    if !state.is_loaded {
        ui.separator();
        ui.colored_label(Color32::GRAY, &state.status);
        return;
    }

    let (has_changes, modified_count) = state.modified_stats();
    let is_converting = state.conversion_task.is_some();

    // ── 工具列 ───────────────────────────────────────────────────────────────
    ui.horizontal_wrapped(|ui| {
        if ui.button("匯出 CSV").clicked() {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("CSV", &["csv"])
                .set_file_name("locres_export.csv")
                .save_file()
            {
                match state.export_csv(&path) {
                    Ok(()) => state.status = format!("已匯出至: {}", path.display()),
                    Err(e) => state.status = format!("匯出失敗: {}", e),
                }
            }
        }

        if ui.button("匯入 CSV").clicked() {
            if let Some(path) = rfd::FileDialog::new().add_filter("CSV", &["csv"]).pick_file() {
                if let Err(e) = state.import_csv(&path) {
                    state.status = format!("匯入失敗: {}", e);
                }
            }
        }

        ui.separator();

        // ── 簡轉繁按鈕（點擊開啟選擇對話框）────────────────────────────────
        if ui.add_enabled(!is_converting, Button::new("簡轉繁…")).clicked() {
            state.show_convert_dialog = true;
        }
        if let Some((done, total)) = state.conversion_progress() {
            ui.colored_label(
                Color32::from_rgb(100, 200, 255),
                format!("轉換中… {}/{}", done, total),
            );
        }

        ui.separator();

        if ui.add_enabled(
            has_changes,
            Button::new(RichText::new("提交到待建構區").color(Color32::BLACK))
                .fill(Color32::from_rgb(255, 200, 50)),
        ).clicked() {
            state.commit_to_staging(staging);
            state.status = "已提交修改到待建構區".to_string();
        }

        if ui.add_enabled(has_changes, Button::new("重設修改")).clicked() {
            state.reset_all();
        }
    });

    CollapsingHeader::new("批次尋找替換").show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label("尋找:");
            ui.text_edit_singleline(&mut state.batch_find);
            ui.label("替換為:");
            ui.text_edit_singleline(&mut state.batch_replace);
            if ui.button("套用").clicked() { state.batch_find_replace(); }
        });
    });

    ui.horizontal(|ui| {
        ui.label("搜尋:");
        ui.text_edit_singleline(&mut state.search);
        ui.checkbox(&mut state.show_modified_only, "只顯示修改");
    });

    if !state.status.is_empty() {
        ui.horizontal(|ui| {
            ui.colored_label(Color32::from_rgb(150, 200, 255), &state.status);
            let total = state.entries.len();
            let color = if modified_count > 0 { Color32::from_rgb(100, 220, 100) } else { Color32::GRAY };
            ui.colored_label(color, format!("({} 個已修改 / {} 個條目)", modified_count, total));
        });
    }

    ui.separator();

    let indices = state.filtered_indices();
    TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .cell_layout(Layout::left_to_right(Align::Min))
        .column(Column::initial(180.0).clip(true))
        .column(Column::initial(350.0).clip(true))
        .column(Column::remainder())
        .header(24.0, |mut header| {
            header.col(|ui| { ui.strong("Key"); });
            header.col(|ui| { ui.strong("原始"); });
            header.col(|ui| { ui.strong("修改"); });
        })
        .body(|body| {
            body.rows(60.0, indices.len(), |mut row| {
                let idx = indices[row.index()];
                let entry = &mut state.entries[idx];
                let is_modified = entry.is_modified();

                row.col(|ui| {
                    ui.label(RichText::new(&entry.key).monospace().color(Color32::LIGHT_GRAY));
                });
                row.col(|ui| {
                    ScrollArea::vertical().id_source(format!("orig_{}", idx)).show(ui, |ui| {
                        ui.label(RichText::new(&entry.value).color(Color32::GRAY));
                    });
                });
                row.col(|ui| {
                    ui.horizontal(|ui| {
                        let mut edited = entry.modified.as_deref().unwrap_or(&entry.value).to_string();
                        ScrollArea::vertical().id_source(format!("mod_{}", idx)).show(ui, |ui| {
                            if ui.add_sized(
                                [ui.available_width() - 40.0, 50.0],
                                TextEdit::multiline(&mut edited),
                            ).changed() {
                                entry.modified = if edited == entry.value { None } else { Some(edited) };
                            }
                        });
                        if is_modified && ui.small_button("重設").clicked() {
                            entry.modified = None;
                        }
                    });
                });
            });
        });

    // ── 簡轉繁對話框 ─────────────────────────────────────────────────────────
    show_convert_dialog(ui.ctx(), state);
}

fn show_convert_dialog(ctx: &Context, state: &mut LocresEditorState) {
    if !state.show_convert_dialog { return; }

    let is_converting = state.conversion_task.is_some();
    let mut open = true;

    Window::new("簡轉繁")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_pos(ctx.screen_rect().center()) 
        .min_width(360.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut state.convert_method, ConvertMethod::OpenCC,    "OpenCC（本機）");
                ui.selectable_value(&mut state.convert_method, ConvertMethod::ZhConvert, "繁化姬（線上）");
            });
            ui.separator();

            match state.convert_method {
                // ── OpenCC ───────────────────────────────────────────────────
                ConvertMethod::OpenCC => {
                    ui.label("使用本機 OpenCC 引擎，無需網路，立即轉換。");
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label("轉換模式:");
                        ComboBox::from_id_source("opencc_mode")
                            .selected_text(opencc_mode_label(&state.opencc_mode))
                            .show_ui(ui, |ui| {
                                for &(val, label) in OPENCC_MODES {
                                    ui.selectable_value(&mut state.opencc_mode, val.to_string(), label);
                                }
                            });
                    });
                    ui.add_space(10.0);
                    
                    ui.horizontal(|ui| {
                        if ui.add_enabled(
                            !is_converting,
                            Button::new(RichText::new("  開始轉換  ").color(Color32::BLACK))
                                .fill(Color32::from_rgb(80, 180, 80)),
                        ).clicked() {
                            state.run_opencc();
                        }
                        if ui.button("取消").clicked() {
                            state.show_convert_dialog = false;
                        }
                    });
                }

                // ── 繁化姬 ───────────────────────────────────────────────────
                ConvertMethod::ZhConvert => {
                    ui.horizontal(|ui| {
                        ui.label("線上服務，由");
                        ui.hyperlink_to("繁化姬 zhconvert.org", "https://zhconvert.org");
                        ui.label("提供。");
                    });
                    ui.add_space(6.0);

                    ui.horizontal(|ui| {
                        ui.label("轉換器:");
                        ComboBox::from_id_source("zh_converter")
                            .selected_text(state.zh_converter.display_name())
                            .show_ui(ui, |ui| {
                                for &conv in ZhConverter::all() {
                                    ui.selectable_value(&mut state.zh_converter, conv, conv.display_name());
                                }
                            });
                    });

                    let pending = state.entries.iter().filter(|e| !e.is_modified()).count();
                    let batch_count = pending.div_ceil(zhconvert::BATCH_SIZE);
                    ui.add_space(4.0);

                    if pending > zhconvert::BATCH_SIZE {
                        ui.colored_label(
                            Color32::from_rgb(255, 200, 50),
                            format!(
                                "{} 個條目，將分 {} 批（每批最多 {} 筆）",
                                pending, batch_count, zhconvert::BATCH_SIZE
                            ),
                        );
                    } else {
                        ui.colored_label(Color32::GRAY, format!("{} 個條目（單批）", pending));
                    }

                    // 進度條（轉換中顯示）
                    if let Some((done, total)) = state.conversion_progress() {
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.colored_label(
                                Color32::from_rgb(100, 200, 255),
                                format!("轉換中… {}/{}", done, total),
                            );
                        });
                        let progress = if total > 0 { done as f32 / total as f32 } else { 0.0 };
                        ui.add(ProgressBar::new(progress).desired_width(300.0).animate(true));
                    }

                    ui.add_space(10.0);
                    
                    ui.horizontal(|ui| {
                        if ui.add_enabled(
                            !is_converting,
                            Button::new(RichText::new("  開始轉換  ").color(Color32::BLACK))
                                .fill(Color32::from_rgb(80, 180, 80)),
                        ).clicked() {
                            state.start_zhconvert();
                        }
                        if ui.add_enabled(!is_converting, Button::new("取消")).clicked() {
                            state.show_convert_dialog = false;
                        }
                        if is_converting { ui.spinner(); }
                    });
                }
            }

            if state.convert_method == ConvertMethod::ZhConvert {
                ui.add_space(6.0);
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(RichText::new("本功能使用了繁化姬 API 服務").small().color(Color32::GRAY));
                    ui.hyperlink_to(RichText::new("https://zhconvert.org").small(), "https://zhconvert.org");
                });
            }
        });

    if !open && !is_converting {
        state.show_convert_dialog = false;
    }
}
