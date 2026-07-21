//! UI layer: panels, the reusable kv table, and the workspace shell.

pub mod app;
pub mod autotest_panel;
pub mod console_panel;
pub mod docker_host_dialog;
pub mod docker_panel;
pub mod env_panel;
pub mod environments_view;
pub mod kv_manager_view;
pub mod k8s_cluster_dialog;
pub mod k8s_panel;
pub mod kv_table;
pub mod method_colors;
pub mod project_manage_panel;
pub mod project_tree_panel;
pub mod proxy_panel;
pub mod request_panel;
pub mod response_panel;
pub mod settings_window;
pub mod share_dialog;
pub mod share_panel;
pub mod sftp_browser;
pub mod ssh_host_dialog;
pub mod ssh_panel;
pub mod stress_panel;
pub mod theme;
pub mod themes;

pub use app::VerveApp;
