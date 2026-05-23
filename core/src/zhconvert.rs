// 繁化姬 API 整合
// 官方網站：https://zhconvert.org
// 使用條款：免費使用者須在軟體中明確聲明使用了繁化姬服務並附上超連結。
//   - 超過 3000 條目自動分批（每批 3000 筆）
//   - 分批間使用 userProtectReplace 保護分隔符不被轉換
//   - 透過 progress_cb 回呼回報進度

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::models::LocresEntry;

// ── 轉換器型別 ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Copy)]
pub enum ZhConverter {
    Traditional, // 繁體化
    Hongkong,    // 香港化
    Taiwan,      // 台灣化
}

impl ZhConverter {
    /// 對應繁化姬 API 的 `converter` 參數值
    pub fn as_api_str(self) -> &'static str {
        match self {
            ZhConverter::Traditional => "Traditional",
            ZhConverter::Hongkong => "Hongkong",
            ZhConverter::Taiwan => "Taiwan",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            ZhConverter::Traditional => "繁體化",
            ZhConverter::Hongkong => "香港化",
            ZhConverter::Taiwan => "台灣化",
        }
    }

    pub fn all() -> &'static [ZhConverter] {
        &[ZhConverter::Traditional, ZhConverter::Hongkong, ZhConverter::Taiwan]
    }
}

// ── API 回應結構 ──────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ApiResponse {
    code: i32,
    msg: String,
    data: Option<ApiData>,
}

#[derive(Deserialize)]
struct ApiData {
    text: String,
}

// ── 分批常數與分隔符 ──────────────────────────────────────────────────────────

/// 每批最大條目數（官方無明確限制，3000 為保守值）
pub const BATCH_SIZE: usize = 3000;

/// 批次內各條目之間的分隔符。
/// 使用 ASCII 不常見組合，並透過 userProtectReplace 保護，
/// 確保轉換器不會修改此字串。
const SEP: &str = "<<<ZHSEP>>>";
const SEP_FULL: &str = "\n<<<ZHSEP>>>\n";

// ── 單批次轉換 ────────────────────────────────────────────────────────────────

fn convert_batch(texts: &[&str], converter: ZhConverter) -> Result<Vec<String>> {
    if texts.is_empty() {
        return Ok(vec![]);
    }

    let combined = texts.join(SEP_FULL);

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("無法建立 HTTP client")?;

    let response = client
        .post("https://api.zhconvert.org/convert")
        .form(&[
            ("text", combined.as_str()),
            ("converter", converter.as_api_str()),
            // 保護分隔符不被繁化姬修改
            ("userProtectReplace", SEP),
        ])
        .send()
        .context("繁化姬 API 請求失敗，請確認網路連線")?;

    let api_resp: ApiResponse = response.json().context("解析繁化姬 API 回應失敗")?;

    if api_resp.code != 0 {
        anyhow::bail!("繁化姬 API 錯誤 (code={}): {}", api_resp.code, api_resp.msg);
    }

    let result_text = api_resp
        .data
        .ok_or_else(|| anyhow::anyhow!("繁化姬 API 回應缺少 data 欄位"))?
        .text;

    // 分割回各條目結果
    let results: Vec<String> = result_text
        .split(SEP_FULL)
        .map(|s| s.trim_matches('\n').to_string())
        .collect();

    if results.len() != texts.len() {
        // 寬鬆處理：盡量對應，多退少補
        let mut aligned = results;
        aligned.resize_with(texts.len(), || String::new());
        return Ok(aligned);
    }

    Ok(results)
}

// ── 公開入口：批次轉換 LocresEntry ────────────────────────────────────────────
//
// 只轉換「尚未被手動修改」的條目（is_modified() == false），
// 避免覆蓋使用者已經手動輸入的翻譯。
//
// `progress_cb(completed, total)`：每完成一批呼叫一次。

pub fn convert_entries(
    entries: &mut Vec<LocresEntry>,
    converter: ZhConverter,
    mut progress_cb: impl FnMut(usize, usize),
) -> Result<usize> {
    // 收集需要轉換的索引（未修改的條目）
    let targets: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| !e.is_modified())
        .map(|(i, _)| i)
        .collect();

    let total = targets.len();
    if total == 0 {
        progress_cb(0, 0);
        return Ok(0);
    }

    let mut converted_count = 0;

    for (batch_idx, chunk) in targets.chunks(BATCH_SIZE).enumerate() {
        let texts: Vec<&str> = chunk.iter().map(|&i| entries[i].value.as_str()).collect();

        let results = convert_batch(&texts, converter)
            .with_context(|| format!("第 {} 批次（共 {} 批）轉換失敗", batch_idx + 1, targets.chunks(BATCH_SIZE).count()))?;

        for (&entry_idx, new_val) in chunk.iter().zip(results.iter()) {
            let entry = &mut entries[entry_idx];
            // 只有真正改變的才標記 modified，避免污染 diff
            if !new_val.is_empty() && new_val != &entry.value {
                entry.modified = Some(new_val.clone());
                converted_count += 1;
            }
        }

        let completed = std::cmp::min((batch_idx + 1) * BATCH_SIZE, total);
        progress_cb(completed, total);
    }

    Ok(converted_count)
}
