//! Read-only exchange export, not a backup or a CSV import contract.
use super::*;
use std::{
    fs::OpenOptions,
    io::Write,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CsvExport {
    path: String,
    count: usize,
}

fn cell(value: &str) -> String {
    // Spreadsheet readers may execute a quoted cell as a formula. Force text.
    let risky = value.trim_start().starts_with(['=', '+', '-', '@'])
        || value.starts_with(['\t', '\r', '\n']);
    format!(
        "\"{}{}\"",
        if risky { "'" } else { "" },
        value.replace('"', "\"\"")
    )
}
fn document(tasks: &[LocalTask]) -> Result<(String, usize), String> {
    let active: Vec<_> = tasks.iter().filter(|t| !t.trashed).collect();
    if active.len() > 10000 {
        return Err("CSV 导出最多支持 10,000 项任务，请使用完整备份".into());
    }
    let mut output =
        "\u{feff}任务名称,任务详情,状态,进度(%),四象限,今日待办,开始日期,计划完成日期\r\n"
            .to_string();
    for t in &active {
        let status = match t.progress {
            0 => "未开始",
            100 => "已完成",
            _ => "进行中",
        };
        let quadrant = match t.quadrant.as_str() {
            "iu" => "重要且紧急",
            "in" => "重要不紧急",
            "un" => "紧急不重要",
            _ => "不重要不紧急",
        };
        output.push_str(
            &[
                t.title.clone(),
                t.detail.clone(),
                status.into(),
                t.progress.to_string(),
                quadrant.into(),
                if t.today { "是" } else { "否" }.into(),
                t.start_date.clone().unwrap_or_default(),
                t.planned_date.clone().unwrap_or_default(),
            ]
            .iter()
            .map(|v| cell(v))
            .collect::<Vec<_>>()
            .join(","),
        );
        output.push_str("\r\n");
        if output.len() > 16 * 1024 * 1024 {
            return Err("CSV 超过 16 MiB，请使用完整备份".into());
        }
    }
    Ok((output, active.len()))
}
fn write_csv(path: &std::path::Path, csv: &str) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("未导出：请选择不存在的新文件名（{e}）"))?;
    file.write_all(csv.as_bytes()).map_err(error)?;
    file.sync_all().map_err(error)?;
    Ok(())
}
pub(super) fn export(state: &LocalWorkspaceState) -> Result<Option<CsvExport>, String> {
    let tasks = state.with(|db| Ok(db.snapshot()?.tasks))?;
    let (csv, count) = document(&tasks)?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(error)?
        .as_secs();
    let Some(mut path) = rfd::FileDialog::new()
        .set_title("导出全部本地任务 CSV（不覆盖旧文件）")
        .set_file_name(format!("本地任务-{stamp}.csv"))
        .add_filter("CSV 任务清单", &["csv"])
        .save_file()
    else {
        return Ok(None);
    };
    if !path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("csv"))
    {
        path.set_extension("csv");
    }
    write_csv(&path, &csv)?;
    Ok(Some(CsvExport {
        path: path.display().to_string(),
        count,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn task() -> LocalTask {
        LocalTask {
            content_modified_at: None,
            id: "sample".into(),
            title: "任务,\"一\"".into(),
            detail: "两行\n详情".into(),
            progress: 71,
            quadrant: "in".into(),
            today: true,
            start_date: None,
            planned_date: Some("2026-10-20".into()),
            trashed: false,
            revision: 1,
        }
    }
    #[test]
    fn csv_quotes_multiline_unicode_and_derived_status() {
        let (csv, count) = document(&[task()]).unwrap();
        assert_eq!(count, 1);
        assert!(csv.starts_with('\u{feff}'));
        assert!(csv.contains("\"任务,\"\"一\"\"\""));
        assert!(csv.contains("\"两行\n详情\""));
        assert!(csv.contains("\"进行中\",\"71\""));
    }
    #[test]
    fn formula_like_text_is_neutralized_even_with_leading_whitespace() {
        for v in [
            "=HYPERLINK(\"x\")",
            "  =1+1",
            "+1",
            "-2",
            "@SUM(A1)",
            "\tstuff",
            "\ntext",
        ] {
            assert!(cell(v).starts_with("\"'"));
        }
        assert_eq!(cell("正常文本"), "\"正常文本\"");
    }
    #[test]
    fn export_omits_trash_and_does_not_mutate_tasks() {
        let mut trash = task();
        trash.trashed = true;
        let t = task();
        let before = t.clone();
        assert_eq!(document(&[t.clone(), trash]).unwrap().1, 1);
        assert_eq!(t, before);
        assert_eq!(document(&[]).unwrap().1, 0);
    }
    #[test]
    fn oversized_export_is_refused_not_truncated() {
        assert!(document(&vec![task(); 10001]).is_err());
    }
    #[test]
    fn actual_file_round_trip_and_no_overwrite() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("project-track-csv-test-{nonce}"));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("sample.csv");
        let (csv, _) = document(&[task()]).unwrap();
        write_csv(&path, &csv).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), csv);
        assert!(write_csv(&path, "replacement").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), csv);
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }
}
