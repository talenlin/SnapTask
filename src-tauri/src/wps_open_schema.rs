//! WPS365 schema discovery only. No credentials, network calls, or connection writes.
//! The AirScript adapter remains unchanged until the official route is verified.

use std::collections::{BTreeMap, HashSet};

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WpsDiscoveredTaskTable {
    pub file_id: String,
    pub sheet_id: String,
    pub sheet_name: String,
    pub field_ids: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WpsSchemaDiscoveryError {
    InvalidFileId,
    InvalidResponse,
    ApiRejected(i64),
    InvalidManifest,
    NoCompatibleTable,
    AmbiguousTables,
    SelectedTableNotCompatible,
}

struct FieldRule {
    key: String,
    names: Vec<String>,
    kind: String,
    required: bool,
}

fn field_rules() -> Result<Vec<FieldRule>, WpsSchemaDiscoveryError> {
    let manifest: Value =
        serde_json::from_str(include_str!("../resources/task-workspace-template.v1.json"))
            .map_err(|_| WpsSchemaDiscoveryError::InvalidManifest)?;
    manifest["taskTable"]["fields"]
        .as_array()
        .ok_or(WpsSchemaDiscoveryError::InvalidManifest)?
        .iter()
        .map(|field| {
            let key = field["key"]
                .as_str()
                .ok_or(WpsSchemaDiscoveryError::InvalidManifest)?;
            let name = field["name"]
                .as_str()
                .ok_or(WpsSchemaDiscoveryError::InvalidManifest)?;
            let kind = field["platformFieldTypes"]["wps"]
                .as_str()
                .or_else(|| field["type"].as_str())
                .ok_or(WpsSchemaDiscoveryError::InvalidManifest)?;
            let required = field["required"]
                .as_bool()
                .ok_or(WpsSchemaDiscoveryError::InvalidManifest)?;
            let mut names = vec![name.to_string()];
            for alias in field["aliases"]
                .as_array()
                .ok_or(WpsSchemaDiscoveryError::InvalidManifest)?
            {
                names.push(
                    alias
                        .as_str()
                        .ok_or(WpsSchemaDiscoveryError::InvalidManifest)?
                        .to_string(),
                );
            }
            Ok(FieldRule {
                key: key.to_string(),
                names,
                kind: kind.to_string(),
                required,
            })
        })
        .collect()
}

fn identifier(value: &Value) -> Option<String> {
    if let Some(id) = value.as_str() {
        if !id.is_empty() && id.trim() == id && id.chars().count() <= 128 {
            return Some(id.to_string());
        }
    }
    value.as_u64().map(|id| id.to_string())
}

fn matching_type(kind: &str, actual: &str) -> bool {
    match kind {
        "text" | "multilineText" => matches!(actual, "SingleLineText" | "MultiLineText"),
        "singleSelect" => actual == "SingleSelect",
        "number" => actual == "Number",
        "date" => actual == "Date",
        // Optional unknown field types are omitted, never guessed.
        _ => false,
    }
}

fn map_fields(sheet: &Value, rules: &[FieldRule]) -> Option<BTreeMap<String, String>> {
    let fields = sheet["fields"].as_array()?;
    let mut ids = HashSet::new();
    for field in fields {
        if !ids.insert(identifier(&field["id"])?) {
            return None;
        }
        field["name"].as_str()?;
        field["type"].as_str()?;
    }
    let mut result = BTreeMap::new();
    for rule in rules {
        let matches: Vec<_> = fields
            .iter()
            .filter(|field| {
                field["name"]
                    .as_str()
                    .is_some_and(|name| rule.names.iter().any(|candidate| candidate == name))
            })
            .collect();
        // Even two aliases for the same logical field are ambiguous.
        if matches.len() > 1 {
            return None;
        }
        match matches.first() {
            Some(field) if matching_type(&rule.kind, field["type"].as_str()?) => {
                result.insert(rule.key.clone(), identifier(&field["id"])?);
            }
            _ if rule.required => return None,
            _ => {}
        }
    }
    Some(result)
}

/// Resolve a task table from the documented WPS365 `/schema` response.
/// This proves field discovery only, NOT empty records, views, dashboard contents,
/// status linkage, file permissions, or complete template-copy acceptance.
pub fn discover_task_table(
    file_id: &str,
    response: &Value,
    selected_sheet_id: Option<&str>,
) -> Result<WpsDiscoveredTaskTable, WpsSchemaDiscoveryError> {
    if identifier(&Value::String(file_id.to_string())).is_none() {
        return Err(WpsSchemaDiscoveryError::InvalidFileId);
    }
    let code = response["code"]
        .as_i64()
        .ok_or(WpsSchemaDiscoveryError::InvalidResponse)?;
    if code != 0 {
        // Do not propagate server messages or response contents into UI/logs.
        return Err(WpsSchemaDiscoveryError::ApiRejected(code));
    }
    let sheets = response["data"]["sheets"]
        .as_array()
        .ok_or(WpsSchemaDiscoveryError::InvalidResponse)?;
    let rules = field_rules()?;
    let mut sheet_ids = HashSet::new();
    let mut candidates = Vec::new();
    for sheet in sheets {
        let id = identifier(&sheet["id"]).ok_or(WpsSchemaDiscoveryError::InvalidResponse)?;
        if !sheet_ids.insert(id.clone()) {
            return Err(WpsSchemaDiscoveryError::InvalidResponse);
        }
        let sheet_type = sheet["sheet_type"]
            .as_str()
            .ok_or(WpsSchemaDiscoveryError::InvalidResponse)?;
        if sheet_type != "xlEtDataBaseSheet" {
            continue;
        }
        let name = sheet["name"]
            .as_str()
            .filter(|name| !name.trim().is_empty())
            .ok_or(WpsSchemaDiscoveryError::InvalidResponse)?;
        if let Some(field_ids) = map_fields(sheet, &rules) {
            candidates.push(WpsDiscoveredTaskTable {
                file_id: file_id.to_string(),
                sheet_id: id,
                sheet_name: name.to_string(),
                field_ids,
            });
        }
    }
    if let Some(selected) = selected_sheet_id {
        return candidates
            .into_iter()
            .find(|table| table.sheet_id == selected)
            .ok_or(WpsSchemaDiscoveryError::SelectedTableNotCompatible);
    }
    match candidates.len() {
        0 => Err(WpsSchemaDiscoveryError::NoCompatibleTable),
        1 => Ok(candidates.remove(0)),
        _ => Err(WpsSchemaDiscoveryError::AmbiguousTables),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn task_sheet(id: Value) -> Value {
        json!({"id":id,"name":"任务清单","sheet_type":"xlEtDataBaseSheet",
        "fields":[
            {"id":"B","name":"任务名称","type":"MultiLineText"},
            {"id":"C","name":"任务类别","type":"SingleSelect"},
            {"id":"D","name":"状态","type":"SingleSelect"},
            {"id":"E","name":"进度","type":"Number"},
            {"id":"F","name":"截止日期","type":"Date"}
        ]})
    }

    fn response(sheets: Vec<Value>) -> Value {
        json!({"code":0,"data":{"sheets":sheets}})
    }

    #[test]
    fn resolves_numeric_ids_and_manifest_aliases() {
        let result =
            discover_task_table("file_example", &response(vec![task_sheet(json!(3))]), None)
                .unwrap();
        assert_eq!(result.sheet_id, "3");
        assert_eq!(result.field_ids["quadrant"], "C");
        assert_eq!(result.field_ids["plannedDate"], "F");
    }

    #[test]
    fn ignores_dashboard_even_if_it_has_similar_fields() {
        let mut dashboard = task_sheet(json!(4));
        dashboard["sheet_type"] = json!("xlDbDashBoardSheet");
        let result = discover_task_table(
            "file_example",
            &response(vec![dashboard, task_sheet(json!(3))]),
            None,
        )
        .unwrap();
        assert_eq!(result.sheet_id, "3");
    }

    #[test]
    fn multiple_candidates_require_explicit_selection() {
        let data = response(vec![task_sheet(json!(3)), task_sheet(json!(4))]);
        assert_eq!(
            discover_task_table("file_example", &data, None),
            Err(WpsSchemaDiscoveryError::AmbiguousTables)
        );
        assert_eq!(
            discover_task_table("file_example", &data, Some("4"))
                .unwrap()
                .sheet_id,
            "4"
        );
        assert_eq!(
            discover_task_table("file_example", &data, Some("5")),
            Err(WpsSchemaDiscoveryError::SelectedTableNotCompatible)
        );
    }

    #[test]
    fn rejects_missing_or_wrong_required_field_type() {
        let mut sheet = task_sheet(json!(3));
        sheet["fields"][3]["type"] = json!("Formula");
        assert_eq!(
            discover_task_table("file_example", &response(vec![sheet]), None),
            Err(WpsSchemaDiscoveryError::NoCompatibleTable)
        );
        let mut sheet = task_sheet(json!(3));
        sheet["fields"][1]["name"] = json!("其他字段");
        assert_eq!(
            discover_task_table("file_example", &response(vec![sheet]), None),
            Err(WpsSchemaDiscoveryError::NoCompatibleTable)
        );
    }

    #[test]
    fn rejects_competing_aliases_and_duplicate_ids() {
        let mut sheet = task_sheet(json!(3));
        sheet["fields"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":"G","name":"任务类型","type":"SingleSelect"}));
        assert_eq!(
            discover_task_table("file_example", &response(vec![sheet]), None),
            Err(WpsSchemaDiscoveryError::NoCompatibleTable)
        );
        let mut sheet = task_sheet(json!(3));
        sheet["fields"][1]["id"] = json!("B");
        assert_eq!(
            discover_task_table("file_example", &response(vec![sheet]), None),
            Err(WpsSchemaDiscoveryError::NoCompatibleTable)
        );
        assert_eq!(
            discover_task_table(
                "file_example",
                &response(vec![task_sheet(json!(3)), task_sheet(json!("3"))]),
                None
            ),
            Err(WpsSchemaDiscoveryError::InvalidResponse)
        );
    }

    #[test]
    fn rejects_untyped_sheets_and_untrusted_error_text() {
        let mut sheet = task_sheet(json!(3));
        sheet.as_object_mut().unwrap().remove("sheet_type");
        assert_eq!(
            discover_task_table("file_example", &response(vec![sheet]), None),
            Err(WpsSchemaDiscoveryError::InvalidResponse)
        );
        let rejected = json!({"code":403000001,"msg":"sensitive server message"});
        assert_eq!(
            discover_task_table("file_example", &rejected, None),
            Err(WpsSchemaDiscoveryError::ApiRejected(403000001))
        );
        assert_eq!(
            discover_task_table("", &response(vec![]), None),
            Err(WpsSchemaDiscoveryError::InvalidFileId)
        );
    }
}
