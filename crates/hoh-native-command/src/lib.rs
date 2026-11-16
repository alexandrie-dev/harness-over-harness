//! Port of @deepseek-ai/dsh-native-command (packages/util/native-command).
pub mod desktop_entry;
pub mod file_applications;
pub mod file_applications_linux;
pub mod file_applications_windows;
pub mod host_environment;
pub mod path_opener;
pub mod runner;
pub mod types;

pub use desktop_entry::{desktop_application_icon, desktop_data_directories, desktop_entry_fields};
pub use file_applications::{native_file_applications, open_native_file_application};
pub use host_environment::{HostEnvironmentFacts, is_wsl};
pub use path_opener::*;
pub use runner::{NativeCommandRunner, run_native_command};
pub use types::NativeFileApplication;
