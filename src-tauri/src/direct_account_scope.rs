//! Non-secret, compile-time target of the new direct-Feishu acceptance profile.
pub const AUTH_IDENTIFIER:&str="app.snaptask.desktop.directfeishu20261020";
pub const SYNC_IDENTIFIER:&str="app.snaptask.desktop.directfeishusync20261020";
pub const APP_ID:&str="cli_0000000000000003";
pub const APP_TOKEN:&str="Example00000000000000000007";
pub const TABLE_ID:&str="tbl0000000000001";
pub const RECORD_IDS:[&str;2]=["rec0000000000002","rec0000000000003"];
pub fn config()->crate::feishu::FeishuConfig{crate::feishu::FeishuConfig{app_id:APP_ID.into(),app_token:APP_TOKEN.into(),table_id:TABLE_ID.into(),secret_configured:false}}
