pub mod installer;
pub mod server;

pub use installer::{
    all_client_keys, client_from_key, list_clients, run_install, run_install_for, run_uninstall,
    run_uninstall_for, Ide, Scope,
};
pub use server::run_mcp_server;
