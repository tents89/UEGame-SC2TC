// build.rs ── 專案工作區圖示處理

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo::rustc-check-cfg=cfg(has_app_icon)");

    // 1. 取得當前子專案 (gui) 的絕對路徑
    let gui_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let gui_path = std::path::Path::new(&gui_dir);
    
    // 2. 依據你的目錄結構，往上跳一層就是總根目錄，再進入 icon 資料夾
    let icon_dir = gui_path.parent().unwrap().join("icon");

    let png_path = icon_dir.join("app_icon.png");
    let ico_path = icon_dir.join("app_icon.ico");
    let icns_path = icon_dir.join("app_icon.icns");

    // ── 1. 處理 PNG（轉換為絕對路徑傳給 main.rs） ──────────────────────────────
    if png_path.is_file() {
        let png_str = png_path.to_str().unwrap();
        println!("cargo:rerun-if-changed={}", png_str);
        
        // 傳遞絕對路徑，徹底解決 include_bytes! 找不到檔案的問題
        println!("cargo:rustc-env=ICON_PATH={}", png_str);
        println!("cargo:rustc-cfg=has_app_icon");
        println!("cargo:warning=[icon] PNG 圖示已就緒: {}", png_str);
    } else {
        println!("cargo:warning=[icon] 找不到 PNG 圖示（預期位置: {:?}）", png_path);
    }

    // ── 2. 跨平台特定圖示處理 ──────────────────────────────────────────────────
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    match target_os.as_str() {
        // Windows：用 winres 嵌入 .ico 到 .exe 檔案資源中
        // winres crate 僅在 host = Windows 時可用（由 Cargo.toml 的 target-specific 依賴控制）
        #[cfg(windows)]
        "windows" => {
            if ico_path.is_file() {
                let ico_str = ico_path.to_str().unwrap();
                println!("cargo:rerun-if-changed={}", ico_str);

                let mut res = winres::WindowsResource::new();
                res.set_icon(ico_str);
                if let Err(e) = res.compile() {
                    println!("cargo:warning=[icon] winres 嵌入失敗: {}", e);
                } else {
                    println!("cargo:warning=[icon] .ico 已嵌入 .exe: {}", ico_str);
                }
            } else {
                println!("cargo:warning=[icon] 找不到 .ico 檔案（預期位置: {:?}）", ico_path);
            }
        }

        // macOS：設定環境變數供打包工具使用
        "macos" => {
            if icns_path.is_file() {
                let icns_str = icns_path.to_str().unwrap();
                println!("cargo:rerun-if-changed={}", icns_str);
                println!("cargo:rustc-env=ICNS_PATH={}", icns_str);
                println!("cargo:warning=[icon] macOS .icns 已就緒: {}", icns_str);
            }
        }

        _ => {}
    }
}