//! Opt-in, compile-time-scoped task writes. Never enabled by a setting or environment variable.
use std::sync::OnceLock;

use crate::feishu::FeishuConfig;

pub const IDENTIFIER: &str = "app.snaptask.desktop.acceptance";
pub const APP_ID: &str = "cli_0000000000000001";
pub const APP_TOKEN: &str = "Example00000000000000000006";
pub const TABLE_ID: &str = "tbl0000000000004";
pub const TEMPLATE_V2_IDENTIFIER: &str = "app.snaptask.desktop.templatev2acceptance";
pub const TEMPLATE_V2_PROMOTION_IDENTIFIER: &str =
    "app.snaptask.desktop.templatev2promotionacceptance";
pub const TEMPLATE_V2_FINAL_IDENTIFIER: &str = "app.snaptask.desktop.templatev2finalacceptance";
pub const TEMPLATE_V2_APP_ID: &str = "cli_0000000000000002";
pub const TEMPLATE_V2_APP_TOKEN: &str = "Example00000000000000000008";
pub const TEMPLATE_V2_TABLE_ID: &str = "tbl0000000000005";
pub const TEMPLATE_V2_PROMOTION_APP_TOKEN: &str = "Example00000000000000000004";
pub const TEMPLATE_V2_PROMOTION_TABLE_ID: &str = "tbl0000000000002";
pub const TEMPLATE_V2_FINAL_APP_TOKEN: &str = "Example00000000000000000001";
pub const TEMPLATE_V2_FINAL_TABLE_ID: &str = "tbl0000000000003";
static RUNTIME_IDENTIFIER: OnceLock<String> = OnceLock::new();

pub fn configure(identifier: &str) -> Result<(), String> {
    if cfg!(feature = "feishu-write-acceptance")
        && identifier != IDENTIFIER
        && identifier != TEMPLATE_V2_IDENTIFIER
        && identifier != TEMPLATE_V2_PROMOTION_IDENTIFIER
        && identifier != TEMPLATE_V2_FINAL_IDENTIFIER
    {
        return Err("隔离验收构建不能使用正式应用身份启动".to_string());
    }
    RUNTIME_IDENTIFIER
        .set(identifier.to_string())
        .map_err(|_| "应用身份已初始化，不能更改".to_string())
}

pub fn enabled() -> bool {
    cfg!(feature = "feishu-write-acceptance")
        && RUNTIME_IDENTIFIER.get().is_some_and(|id| {
            id == IDENTIFIER
                || id == TEMPLATE_V2_IDENTIFIER
                || id == TEMPLATE_V2_PROMOTION_IDENTIFIER
                || id == TEMPLATE_V2_FINAL_IDENTIFIER
        })
}

fn policy_allows(feature: bool, identifier: &str, config: &FeishuConfig) -> bool {
    feature
        && ((identifier == IDENTIFIER
            && config.app_id == APP_ID
            && config.app_token == APP_TOKEN
            && config.table_id == TABLE_ID)
            || (identifier == TEMPLATE_V2_IDENTIFIER
                && config.app_id == TEMPLATE_V2_APP_ID
                && config.app_token == TEMPLATE_V2_APP_TOKEN
                && config.table_id == TEMPLATE_V2_TABLE_ID)
            || (identifier == TEMPLATE_V2_PROMOTION_IDENTIFIER
                && config.app_id == TEMPLATE_V2_APP_ID
                && config.app_token == TEMPLATE_V2_PROMOTION_APP_TOKEN
                && config.table_id == TEMPLATE_V2_PROMOTION_TABLE_ID)
            || (identifier == TEMPLATE_V2_FINAL_IDENTIFIER
                && config.app_id == TEMPLATE_V2_APP_ID
                && config.app_token == TEMPLATE_V2_FINAL_APP_TOKEN
                && config.table_id == TEMPLATE_V2_FINAL_TABLE_ID))
}

pub fn workspace_app_token() -> Option<&'static str> {
    if !enabled() {
        return None;
    }
    match RUNTIME_IDENTIFIER.get().map(String::as_str) {
        Some(TEMPLATE_V2_IDENTIFIER) => Some(TEMPLATE_V2_APP_TOKEN),
        Some(TEMPLATE_V2_PROMOTION_IDENTIFIER) => Some(TEMPLATE_V2_PROMOTION_APP_TOKEN),
        Some(TEMPLATE_V2_FINAL_IDENTIFIER) => Some(TEMPLATE_V2_FINAL_APP_TOKEN),
        Some(IDENTIFIER) => Some(APP_TOKEN),
        _ => None,
    }
}

pub fn allows(config: &FeishuConfig) -> bool {
    policy_allows(
        cfg!(feature = "feishu-write-acceptance"),
        RUNTIME_IDENTIFIER.get().map(String::as_str).unwrap_or(""),
        config,
    )
}

pub fn ensure_task_write(config: &FeishuConfig) -> Result<(), String> {
    if allows(config) {
        Ok(())
    } else {
        Err("写入尚未开放：正式版保持只读，隔离验收版仅允许指定账号应用、验收空间和任务表；本次未发送写入请求".to_string())
    }
}

pub fn ensure_source(source: &str) -> Result<(), String> {
    if cfg!(feature = "feishu-write-acceptance") && source != "feishu" {
        Err("写入尚未开放：隔离验收版不能向 WPS 或其他数据源写入".to_string())
    } else {
        Ok(())
    }
}

pub fn ensure_normal_build() -> Result<(), String> {
    if cfg!(feature = "feishu-write-acceptance") {
        Err("隔离验收版不能重新创建空间、修改表结构、断开共享授权或设置开机启动；请使用现有指定测试空间".to_string())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> FeishuConfig {
        FeishuConfig {
            app_id: APP_ID.to_string(),
            app_token: APP_TOKEN.to_string(),
            table_id: TABLE_ID.to_string(),
            secret_configured: true,
        }
    }

    #[test]
    fn scope_requires_feature_runtime_identity_and_exact_connection() {
        let selected = config();
        assert!(policy_allows(true, IDENTIFIER, &selected));
        assert!(!policy_allows(false, IDENTIFIER, &selected));
        assert!(!policy_allows(true, "app.snaptask.desktop", &selected));
        assert!(!policy_allows(true, "", &selected));
        for field in ["app_id", "app_token", "table_id"] {
            let mut wrong = selected.clone();
            match field {
                "app_id" => wrong.app_id.push_str("-other"),
                "app_token" => wrong.app_token = "Example00000000000000000003".to_string(),
                _ => wrong.table_id.push_str("-other"),
            }
            assert!(!policy_allows(true, IDENTIFIER, &wrong), "{field}");
        }
        let selected_final = FeishuConfig {
            app_id: TEMPLATE_V2_APP_ID.to_string(),
            app_token: TEMPLATE_V2_FINAL_APP_TOKEN.to_string(),
            table_id: TEMPLATE_V2_FINAL_TABLE_ID.to_string(),
            secret_configured: true,
        };
        assert!(policy_allows(
            true,
            TEMPLATE_V2_FINAL_IDENTIFIER,
            &selected_final
        ));
        assert!(!policy_allows(
            false,
            TEMPLATE_V2_FINAL_IDENTIFIER,
            &selected_final
        ));
        assert!(!policy_allows(
            true,
            "app.snaptask.desktop",
            &selected_final
        ));
        assert!(!policy_allows(
            true,
            TEMPLATE_V2_PROMOTION_IDENTIFIER,
            &selected_final
        ));
        for field in ["app_id", "app_token", "table_id"] {
            let mut wrong = selected_final.clone();
            match field {
                "app_id" => wrong.app_id.push_str("-other"),
                "app_token" => wrong.app_token.push_str("-other"),
                _ => wrong.table_id.push_str("-other"),
            }
            assert!(
                !policy_allows(true, TEMPLATE_V2_FINAL_IDENTIFIER, &wrong),
                "{field}"
            );
        }

        let selected_v2 = FeishuConfig {
            app_id: TEMPLATE_V2_APP_ID.to_string(),
            app_token: TEMPLATE_V2_APP_TOKEN.to_string(),
            table_id: TEMPLATE_V2_TABLE_ID.to_string(),
            secret_configured: true,
        };
        assert!(policy_allows(true, TEMPLATE_V2_IDENTIFIER, &selected_v2));
        assert!(!policy_allows(true, IDENTIFIER, &selected_v2));

        let selected_promotion = FeishuConfig {
            app_token: TEMPLATE_V2_PROMOTION_APP_TOKEN.to_string(),
            table_id: TEMPLATE_V2_PROMOTION_TABLE_ID.to_string(),
            ..selected_v2.clone()
        };
        assert!(!policy_allows(
            true,
            TEMPLATE_V2_FINAL_IDENTIFIER,
            &selected_promotion
        ));
        assert!(policy_allows(
            true,
            TEMPLATE_V2_PROMOTION_IDENTIFIER,
            &selected_promotion
        ));
        assert!(!policy_allows(
            false,
            TEMPLATE_V2_PROMOTION_IDENTIFIER,
            &selected_promotion
        ));
        assert!(!policy_allows(
            true,
            "app.snaptask.desktop",
            &selected_promotion
        ));
        assert!(!policy_allows(
            true,
            TEMPLATE_V2_IDENTIFIER,
            &selected_promotion
        ));
        assert!(!policy_allows(
            true,
            TEMPLATE_V2_PROMOTION_IDENTIFIER,
            &selected_v2
        ));
        for field in ["app_id", "app_token", "table_id"] {
            let mut wrong = selected_promotion.clone();
            match field {
                "app_id" => wrong.app_id.push_str("-other"),
                "app_token" => wrong.app_token.push_str("-other"),
                _ => wrong.table_id.push_str("-other"),
            }
            assert!(
                !policy_allows(true, TEMPLATE_V2_PROMOTION_IDENTIFIER, &wrong),
                "{field}"
            );
        }
    }

    #[test]
    fn uninitialized_process_fails_closed() {
        assert!(!enabled());
        assert!(ensure_task_write(&config()).is_err());
    }
}
