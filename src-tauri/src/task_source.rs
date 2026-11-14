use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSourceDescriptor {
    pub id: &'static str,
    pub name: &'static str,
    pub location: &'static str,
    pub status: &'static str,
    pub file_extensions: &'static [&'static str],
    pub supports_read: bool,
    pub supports_write: bool,
    pub requires_confirmation: bool,
}

pub trait TaskSource {
    fn descriptor(&self) -> TaskSourceDescriptor;
}

struct WpsMultidimensionalTable;
struct FeishuBitable;
struct LocalTaskWorkspace;

impl TaskSource for WpsMultidimensionalTable {
    fn descriptor(&self) -> TaskSourceDescriptor {
        TaskSourceDescriptor {
            id: "wps-multidimensional-table",
            name: "WPS 多维表格",
            location: "cloud",
            status: "available",
            file_extensions: &[],
            supports_read: true,
            supports_write: true,
            requires_confirmation: true,
        }
    }
}

impl TaskSource for FeishuBitable {
    fn descriptor(&self) -> TaskSourceDescriptor {
        if crate::write_acceptance::enabled() {
            return TaskSourceDescriptor {
                id: "feishu-bitable",
                name: "飞书多维表格（隔离写入验收）",
                location: "cloud",
                status: "acceptance-only",
                file_extensions: &[],
                supports_read: true,
                supports_write: true,
                requires_confirmation: true,
            };
        }
        TaskSourceDescriptor {
            id: "feishu-bitable",
            name: "飞书多维表格",
            location: "cloud",
            status: "read-preview",
            file_extensions: &[],
            supports_read: true,
            supports_write: false,
            requires_confirmation: true,
        }
    }
}

impl TaskSource for LocalTaskWorkspace {
    fn descriptor(&self) -> TaskSourceDescriptor {
        TaskSourceDescriptor {
            id: "local-task-workspace",
            name: "本地任务工作台",
            location: "local",
            status: "available",
            file_extensions: &[],
            supports_read: true,
            supports_write: true,
            requires_confirmation: true,
        }
    }
}

pub fn available_sources() -> Vec<TaskSourceDescriptor> {
    let sources: [&dyn TaskSource; 3] = [
        &WpsMultidimensionalTable,
        &FeishuBitable,
        &LocalTaskWorkspace,
    ];
    sources.iter().map(|source| source.descriptor()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_workspace_has_safe_write_contract() {
        let source = available_sources()
            .into_iter()
            .find(|source| source.id == "local-task-workspace")
            .expect("local workspace source");
        assert!(source.file_extensions.is_empty());
        assert_eq!(source.status, "available");
        assert!(source.supports_read);
        assert!(source.supports_write);
        assert!(source.requires_confirmation);
    }

    #[test]
    fn feishu_source_starts_read_only_until_live_write_verification() {
        let source = available_sources()
            .into_iter()
            .find(|source| source.id == "feishu-bitable")
            .expect("feishu source");
        assert!(source.supports_read);
        assert!(!source.supports_write);
        assert!(source.requires_confirmation);
        assert_eq!(source.status, "read-preview");
    }
}
