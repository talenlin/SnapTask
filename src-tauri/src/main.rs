#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

fn main() {
    #[cfg(feature = "feishu-direct-sync-ui-acceptance")]
    snaptask_lib::startup_diagnostics::install();
    snaptask_lib::run();
}
