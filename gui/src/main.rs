// release build 隱藏 Windows console 視窗（避免出現黑底命令列）；
// debug build 保留 console 以便檢視 eprintln 偵錯輸出。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use std::io::Write;
use std::path::PathBuf;

mod app;
mod tree_view;
mod locres_editor;
mod build_panel;

// ── 跨平台啟動 log ───────────────────────────────────────────────────────────
// release Windows 沒有 console，eprintln! 會被吃掉；故同步寫一份 log 檔到
// 使用者資料目錄，方便回報問題時提供。

fn user_data_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA").map(PathBuf::from)
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join("Library").join("Application Support"))
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        if let Some(d) = std::env::var_os("XDG_DATA_HOME") {
            Some(PathBuf::from(d))
        } else {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share"))
        }
    }
}

fn log_file_path() -> Option<PathBuf> {
    let dir = user_data_dir()?.join("UE_L10nTool");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("startup.log"))
}

fn startup_log(msg: &str) {
    // terminal 啟動時仍可看到（macOS / Linux / Windows debug）
    eprintln!("{}", msg);

    if let Some(path) = log_file_path() {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let _ = writeln!(f, "[{}] {}", ts, msg);
        }
    }
}

fn main() -> eframe::Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Unreal Engine L10n Mod Tool")
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size([800.0, 600.0]);

    if let Some(icon) = load_icon() {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "Unreal Engine L10n Mod Tool",
        options,
        Box::new(|cc| {
            setup_fonts(&cc.egui_ctx);
            Box::new(app::App::new(cc)) as Box<dyn eframe::App>
        }),
    )
}


//   1. 編譯期嵌入（最可靠）：build.rs 找到圖示時設定 has_app_icon cfg
//      → 使用 include_bytes!(env!("ICON_PATH")) 在編譯時直接嵌入位元組
//
// 平台說明：
//   Windows：工作列圖示來自 .exe 內嵌資源（build.rs + winres），
//            視窗標題列圖示來自此函式回傳的 IconData。
//   macOS  ：視窗標題列圖示由此函式設定；Dock 圖示需 cargo-bundle + .icns。
//   Linux  ：_NET_WM_ICON（由 egui 設定），影響大多數視窗管理員的工作列圖示。

fn load_icon() -> Option<egui::IconData> {
    // ── 方案 A：編譯期嵌入 ───────────────────────────────────────────────────
    #[cfg(has_app_icon)]
    {
        // ICON_PATH 由 build.rs 的 cargo:rustc-env=ICON_PATH=... 設定
        const ICON_BYTES: &[u8] = include_bytes!(env!("ICON_PATH"));
        if let Ok(img) = image::load_from_memory(ICON_BYTES) {
            let img = img.into_rgba8();
            let (width, height) = img.dimensions();
            return Some(egui::IconData {
                rgba: img.into_raw(),
                width,
                height,
            });
        }
    }

    // ── 方案 B：執行期搜尋（build.rs 未設定時的 fallback）──────────────────
    let candidates = ["app_icon.png", "app_icon.ico", "app_icon.icn"];

    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));

    let mut search_dirs: Vec<std::path::PathBuf> = vec![];
    if let Some(ref dir) = exe_dir {
        search_dirs.push(dir.clone());
        search_dirs.push(dir.join("assets"));
        search_dirs.push(dir.join("icon"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        search_dirs.push(cwd.clone());
        search_dirs.push(cwd.join("assets"));
        search_dirs.push(cwd.join("icon"));
    }

    for dir in &search_dirs {
        for name in &candidates {
            let path = dir.join(name);
            if let Ok(bytes) = std::fs::read(&path) {
                if let Ok(img) = image::load_from_memory(&bytes) {
                    let img = img.into_rgba8();
                    let (width, height) = img.dimensions();
                    startup_log(&format!("[icon] 執行期載入: {}", path.display()));
                    return Some(egui::IconData {
                        rgba: img.into_raw(),
                        width,
                        height,
                    });
                }
            }
        }
    }

    startup_log("[icon] 找不到圖示，視窗將使用預設圖示");
    None
}

// ── 系統字體自動偵測 ─────────────────────────────────────────────────────────
fn system_font_candidates() -> Vec<std::path::PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let win = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
        let f = |name: &str| std::path::PathBuf::from(format!(r"{}\Fonts\{}", win, name));
        vec![
            f("msjh.ttc"),    // 微軟正黑體 繁體 ★
            f("msjhbd.ttc"),
            f("kaiu.ttf"),    // 標楷體
            f("mingliu.ttc"), // 細明體
            f("msyh.ttc"),    // 微軟雅黑 簡體 fallback
            f("simsun.ttc"),
            f("meiryo.ttc"),
            f("YuGothM.ttc"),
        ]
    }
    #[cfg(target_os = "macos")]
    {
        let p = |s: &str| std::path::PathBuf::from(s);
        vec![
            p("/System/Library/Fonts/PingFang.ttc"),           // 蘋方 繁體 ★
            p("/System/Library/Fonts/STHeiti Medium.ttc"),
            p("/System/Library/Fonts/STHeiti Light.ttc"),
            p("/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc"),
            p("/Library/Fonts/Hiragino Sans GB.ttc"),
            p("/Library/Fonts/Arial Unicode.ttf"),
        ]
    }
    #[cfg(target_os = "linux")]
    {
        let p = |s: &str| std::path::PathBuf::from(s);
        vec![
            p("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc"), // Noto ★
            p("/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc"),
            p("/usr/share/fonts/noto-cjk/NotoSansCJKtc-Regular.otf"),
            p("/usr/share/fonts/noto/NotoSansCJK-Regular.ttc"),
            p("/usr/share/fonts/truetype/wqy/wqy-microhei.ttc"),         // 文泉驛
            p("/usr/share/fonts/wenquanyi/wqy-microhei/wqy-microhei.ttc"),
            p("/usr/share/fonts/truetype/arphic/uming.ttc"),              // AR PL
            p("/usr/share/fonts/truetype/arphic/ukai.ttc"),
            p("/usr/share/fonts/opentype/source-han-sans/SourceHanSansTC-Regular.otf"),
            p("/usr/share/fonts/adobe-source-han-sans/SourceHanSansTC-Regular.otf"),
        ]
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        vec![]
    }
}

fn load_system_cjk_font() -> Option<Vec<u8>> {
    for path in system_font_candidates() {
        if let Ok(data) = std::fs::read(&path) {
            startup_log(&format!("[font] 載入: {}", path.display()));
            return Some(data);
        }
    }
    startup_log("[font] 找不到系統 CJK 字體，CJK 字元可能顯示為方塊");
    None
}

fn setup_fonts(ctx: &egui::Context) {
    use egui::{FontData, FontDefinitions, FontFamily};

    let Some(font_data) = load_system_cjk_font() else {
        return;
    };

    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert("SystemCJK".to_owned(), FontData::from_owned(font_data));

    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts.families.entry(family).or_default().insert(0, "SystemCJK".to_owned());
    }

    ctx.set_fonts(fonts);
}
