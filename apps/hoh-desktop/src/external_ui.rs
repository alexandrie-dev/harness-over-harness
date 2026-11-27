//! Desktop-owned catalog for isolated pages, workspace screens and chat panels.
use serde::{Deserialize, Serialize};
use tauri::{Manager, WebviewWindow, ipc::JavaScriptChannelId};

#[cfg(target_os = "linux")]
#[path = "external_ui_secretary.rs"]
mod secretary;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PagePlugin {
    id: String,
    name: String,
    version: String,
    digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "chatPanel")]
    chat_panel: Option<ChatPanel>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "workspaceLabel"
    )]
    workspace_label: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceBounds {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ChatPanel {
    title: String,
    sidebar_label: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueueItem {
    thread_id: String,
    title: String,
    project: String,
    state: QueueState,
    started_at: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
enum QueueState {
    Question,
    Approval,
    Review,
}

#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PanelEvent {
    Closed,
    OpenChat {
        #[serde(rename = "threadId")]
        thread_id: String,
    },
}

#[derive(Serialize)]
pub struct Catalog {
    directory: String,
    packages: Vec<PagePlugin>,
    rejected: Vec<Rejection>,
    prepared: Option<PagePlugin>,
}

#[derive(Serialize)]
pub struct InstalledPlugin {
    package: PagePlugin,
    catalog: Catalog,
}

#[derive(Serialize)]
struct Rejection {
    id: String,
    reason: String,
}

fn ensure_main(window: &WebviewWindow) -> Result<(), String> {
    if window.label() != "main" {
        return Err("External plugins can only be opened from the main HoH window.".into());
    }
    Ok(())
}

#[tauri::command]
pub async fn list_external_ui_plugins(window: WebviewWindow) -> Result<Catalog, String> {
    ensure_main(&window)?;
    #[cfg(target_os = "linux")]
    {
        let folder = window
            .app_handle()
            .path()
            .app_data_dir()
            .map_err(|e| e.to_string())?;
        tauri::async_runtime::spawn_blocking(move || linux::catalog_with_prepared(&folder))
            .await
            .map_err(|_| "The plugin catalog could not be read.")?
    }
    #[cfg(not(target_os = "linux"))]
    Err("External UI plugins require the isolated Linux WebKit view.".into())
}

#[tauri::command]
pub async fn install_external_ui_plugin(
    window: WebviewWindow,
    prepared: Option<PagePlugin>,
) -> Result<Option<InstalledPlugin>, String> {
    ensure_main(&window)?;
    #[cfg(target_os = "linux")]
    {
        let profile = window
            .app_handle()
            .path()
            .app_data_dir()
            .map_err(|error| error.to_string())?;
        let source = if prepared.is_some() {
            linux::prepared_path().ok_or("The prepared plugin folder is unavailable.")?
        } else {
            let Some(folder) = crate::native::pick_folder(window).await? else {
                return Ok(None);
            };
            std::path::PathBuf::from(folder)
        };
        tauri::async_runtime::spawn_blocking(move || {
            let package = linux::install(&profile, &source, prepared.as_ref())?;
            let catalog = linux::catalog_with_prepared(&profile)?;
            Ok(Some(InstalledPlugin { package, catalog }))
        })
        .await
        .map_err(|_| "The plugin installation was interrupted.")?
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = prepared;
        Err("External UI plugins require the isolated Linux WebKit view.".into())
    }
}

#[tauri::command]
pub async fn open_external_ui_plugin(
    window: WebviewWindow,
    package: PagePlugin,
    panel_id: Option<u32>,
    on_event: Option<JavaScriptChannelId>,
) -> Result<(), String> {
    ensure_main(&window)?;
    if !matches!(
        (&package.chat_panel, panel_id, &on_event),
        (Some(_), Some(_), Some(_)) | (None, None, None)
    ) {
        return Err("The requested placement does not match the plugin.".into());
    }
    #[cfg(target_os = "linux")]
    {
        let app = window.app_handle().clone();
        let on_event = on_event.map(|id| id.channel_on(window.as_ref().clone()));
        let folder = app.path().app_data_dir().map_err(|e| e.to_string())?;
        let snapshot = tauri::async_runtime::spawn_blocking(move || linux::open(&folder, &package))
            .await
            .map_err(|_| "The plugin package could not be read.")??;
        let engine = app
            .try_state::<crate::native::Host>()
            .map(|host| host.engine.clone());
        let (send, receive) = std::sync::mpsc::channel();
        app.run_on_main_thread(move || {
            let result = match panel_id.zip(on_event) {
                Some((id, events)) => window
                    .default_vbox()
                    .map_err(|_| "The main HoH window is unavailable.".into())
                    .and_then(|parent| linux::show_panel(&parent, snapshot, id, events)),
                None if snapshot.is_workspace_page() => window
                    .default_vbox()
                    .map_err(|_| "The main HoH window is unavailable.".into())
                    .and_then(|parent| linux::open_workspace(&parent, snapshot, engine)),
                None => window
                    .gtk_window()
                    .map_err(|_| "The main HoH window is unavailable.".into())
                    .and_then(|parent| linux::show(&parent, snapshot)),
            };
            let _ = send.send(result);
        })
        .map_err(|_| "The plugin window could not be opened.")?;
        tauri::async_runtime::spawn_blocking(move || {
            receive
                .recv()
                .map_err(|_| "The plugin window could not be opened.")?
        })
        .await
        .map_err(|_| "The plugin window could not be opened.")?
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (package, panel_id, on_event);
        Err("External UI plugins require the isolated Linux WebKit view.".into())
    }
}

#[tauri::command]
pub fn layout_external_ui_workspace(
    window: WebviewWindow,
    id: String,
    bounds: Option<WorkspaceBounds>,
) -> Result<(), String> {
    ensure_main(&window)?;
    if let Some(bounds) = &bounds
        && (![bounds.x, bounds.y, bounds.width, bounds.height]
            .into_iter()
            .all(|value| (0.0..=32_768.0).contains(&value))
            || bounds.width == 0.0
            || bounds.height == 0.0)
    {
        return Err("Invalid workspace view bounds.".into());
    }
    #[cfg(target_os = "linux")]
    window
        .run_on_main_thread(move || linux::layout_workspace(&id, bounds))
        .map_err(|_| "The workspace view could not be positioned.".to_owned())?;
    #[cfg(not(target_os = "linux"))]
    let _ = (id, bounds);
    Ok(())
}

#[tauri::command]
pub fn update_external_ui_panel(
    window: WebviewWindow,
    panel_id: u32,
    revision: u32,
    queue: Vec<QueueItem>,
) -> Result<(), String> {
    ensure_main(&window)?;
    if queue.len() > 512
        || queue.iter().any(|item| {
            item.thread_id.is_empty()
                || item.thread_id.len() > 128
                || !item
                    .thread_id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
                || item.title.chars().count() > 240
                || item.project.chars().count() > 120
        })
    {
        return Err("Invalid plugin queue snapshot.".into());
    }
    #[cfg(target_os = "linux")]
    window
        .run_on_main_thread(move || linux::update_panel(panel_id, revision, queue))
        .map_err(|_| "The plugin panel could not be updated.".to_owned())?;
    #[cfg(not(target_os = "linux"))]
    let _ = (panel_id, revision, queue);
    Ok(())
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde::Deserialize;
    use sha2::{Digest, Sha256};
    use std::{
        cell::{Cell, RefCell},
        ffi::CString,
        fs::{self, File, OpenOptions},
        io::Read,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::{
                ffi::OsStrExt,
                fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
            },
        },
        path::{Path, PathBuf},
        rc::Rc,
        time::{Duration, Instant},
    };
    use tauri::ipc::Channel;

    const SCHEME: &str = "hoh-local-ui";
    const MAX_PACKAGES: usize = 64;
    const MANIFEST_MAX: u64 = 32 * 1024;
    const ENTRY_MAX: u64 = 1024 * 1024;

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Manifest {
        format_version: u8,
        id: String,
        version: String,
        digest: Option<String>,
        ui: Option<Ui>,
        requested_permissions: Permissions,
        contributes: Contributions,
        requires: Requires,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Ui {
        entry: String,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Permissions {
        ui: Vec<String>,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Contributions {
        ui: UiContributions,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields, rename_all = "camelCase")]
    struct UiContributions {
        pages: Option<[Page; 1]>,
        chat_panel: Option<ChatPanel>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Page {
        id: String,
        name: String,
        handler: String,
        state_schema: Option<serde_json::Value>,
        sidebar_label: Option<String>,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Requires {
        services: Services,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Services {
        ui: UiService,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct UiService {
        protocol_versions: [u32; 1],
    }

    pub(super) struct Snapshot {
        package: PagePlugin,
        script: String,
        secretary: bool,
    }

    impl Snapshot {
        pub(super) fn is_workspace_page(&self) -> bool {
            self.package.workspace_label.is_some()
        }
    }

    fn valid_id(id: &str) -> bool {
        !id.is_empty()
            && id.len() <= 64
            && id.split('-').all(|part| {
                !part.is_empty()
                    && part
                        .bytes()
                        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
            })
    }

    fn open_beneath(dir: &File, path: &Path, flags: libc::c_int) -> std::io::Result<File> {
        let path = CString::new(path.as_os_str().as_bytes())?;
        // SAFETY: classically zero-initialized open_how v0 flags are valid.
        let mut how = unsafe { std::mem::zeroed::<libc::open_how>() };
        how.flags = (flags | libc::O_CLOEXEC | libc::O_NOFOLLOW) as u64;
        how.resolve = libc::RESOLVE_BENEATH
            | libc::RESOLVE_NO_SYMLINKS
            | libc::RESOLVE_NO_MAGICLINKS
            | libc::RESOLVE_NO_XDEV;
        // SAFETY: dir is a live descriptor and both syscall pointers refer to initialized data.
        let fd = unsafe {
            libc::syscall(
                libc::SYS_openat2,
                dir.as_raw_fd(),
                path.as_ptr(),
                &how,
                std::mem::size_of::<libc::open_how>(),
            ) as libc::c_int
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: a successful open returns a fresh descriptor owned by the caller.
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    fn folder(profile: &Path) -> Result<(PathBuf, Option<File>), String> {
        let path = profile.join("external_plugins");
        let parent = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(profile)
            .map_err(|_| "The active HoH profile could not be opened.")?;
        match open_beneath(
            &parent,
            Path::new("external_plugins"),
            libc::O_RDONLY | libc::O_DIRECTORY,
        ) {
            Ok(directory) => Ok((path, Some(directory))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((path, None)),
            Err(_) => Err("The external_plugins directory could not be opened safely.".into()),
        }
    }

    fn read_file(dir: &File, path: &Path, max: u64) -> Result<Vec<u8>, String> {
        let file = open_beneath(dir, path, libc::O_RDONLY | libc::O_NONBLOCK)
            .map_err(|_| "A required plugin file could not be opened safely.")?;
        let metadata = file
            .metadata()
            .map_err(|_| "A plugin file could not be inspected.")?;
        if !metadata.is_file() || metadata.nlink() != 1 || metadata.len() > max {
            return Err(
                "Plugin files must be single-link regular files within the package size limits."
                    .into(),
            );
        }
        let mut bytes = Vec::new();
        file.take(max + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "A plugin file could not be read.")?;
        if bytes.len() as u64 > max {
            return Err("A plugin file exceeds the package size limit.".into());
        }
        Ok(bytes)
    }

    fn read(root: &File, id: &str) -> Result<Snapshot, String> {
        if !valid_id(id) {
            return Err("Use a lowercase alphanumeric folder ID, with optional hyphens.".into());
        }
        let dir = open_beneath(root, Path::new(id), libc::O_RDONLY | libc::O_DIRECTORY)
            .map_err(|_| "The plugin folder could not be opened safely.")?;
        read_package(&dir, id)
    }

    fn parse_manifest(bytes: &[u8]) -> Result<Manifest, String> {
        serde_json::from_slice(bytes)
            .map_err(|error| format!("Unsupported plugin manifest: {error}"))
    }

    fn read_package(dir: &File, id: &str) -> Result<Snapshot, String> {
        let manifest_bytes = read_file(dir, Path::new("hoh-package.json"), MANIFEST_MAX)?;
        let manifest = parse_manifest(&manifest_bytes)?;
        if manifest.format_version != 2
            || manifest.id != id
            || manifest.version.len() > 64
            || semver::Version::parse(&manifest.version).is_err()
        {
            return Err("The plugin ID, format or version is invalid.".into());
        }
        let permissions = &manifest.requested_permissions.ui;
        let protocols = manifest.requires.services.ui.protocol_versions;
        match (manifest.contributes.ui, manifest.ui, manifest.digest) {
            (
                UiContributions {
                    pages: None,
                    chat_panel: Some(panel),
                },
                Some(ui),
                Some(digest),
            ) if protocols == [2]
                && permissions.len() == 4
                && ["sidebar", "chatPanel", "reviewQueue", "openChat"]
                    .iter()
                    .all(|permission| permissions.iter().any(|p| p == permission))
                && valid_label(&panel.title)
                && valid_label(&panel.sidebar_label) =>
            {
                let script = read_script(dir, ui, &digest)?;
                Ok(Snapshot {
                    package: PagePlugin {
                        id: id.to_owned(),
                        name: panel.title.clone(),
                        version: manifest.version,
                        digest: format!(
                            "sha256:{}",
                            hoh_protocol::approval::hex(&Sha256::digest(&manifest_bytes))
                        ),
                        chat_panel: Some(panel),
                        workspace_label: None,
                    },
                    script: format!("{script}\n{}", include_str!("external_ui_panel.js")),
                    secretary: false,
                })
            }
            (
                UiContributions {
                    pages: Some([page]),
                    chat_panel: None,
                },
                Some(ui),
                Some(digest),
            ) if (protocols == [1] && permissions == &["page"])
                || (protocols == [4]
                    && page.sidebar_label.is_some()
                    && permissions.len() == 2
                    && ["page", "secretary"]
                        .iter()
                        .all(|permission| permissions.iter().any(|p| p == permission))) =>
            {
                let secretary_manifest = (protocols == [4]).then_some(manifest_bytes.as_slice());
                read_page(
                    dir,
                    id,
                    manifest.version,
                    page,
                    ui,
                    digest,
                    secretary_manifest,
                )
            }
            _ => Err("This package does not match a supported external UI contract.".into()),
        }
    }

    fn valid_label(label: &str) -> bool {
        label == label.trim()
            && !label.is_empty()
            && label.chars().count() <= 80
            && !label.chars().any(|c| {
                c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
            })
    }

    fn read_page(
        dir: &File,
        id: &str,
        version: String,
        page: Page,
        ui: Ui,
        expected_digest: String,
        secretary_manifest: Option<&[u8]>,
    ) -> Result<Snapshot, String> {
        if page.id != "main"
            || page.handler != "mount"
            || !valid_label(&page.name)
            || page
                .sidebar_label
                .as_deref()
                .is_some_and(|label| !valid_label(label))
            || page.state_schema.is_some_and(|schema| {
                schema != serde_json::json!({"type": "object", "additionalProperties": false})
            })
        {
            return Err("This package does not match the supported UI-only page contract.".into());
        }
        let source = read_script(dir, ui, &expected_digest)?;
        let (digest, mount) = if let Some(manifest) = secretary_manifest {
            (
                format!(
                    "sha256:{}",
                    hoh_protocol::approval::hex(&Sha256::digest(manifest))
                ),
                include_str!("external_ui_secretary.js"),
            )
        } else {
            (
                expected_digest,
                ";mount(document.getElementById('hoh-plugin-root'));\n",
            )
        };
        Ok(Snapshot {
            package: PagePlugin {
                id: id.to_owned(),
                name: page.name,
                version,
                digest,
                chat_panel: None,
                workspace_label: page.sidebar_label,
            },
            script: format!("{source}\n{mount}"),
            secretary: secretary_manifest.is_some(),
        })
    }

    fn script_path(entry: &str) -> Result<&Path, String> {
        let path = Path::new(entry);
        if !entry.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
        }) || path.extension().and_then(|ext| ext.to_str()) != Some("js")
        {
            return Err("The UI entry must be a package-relative .js file.".into());
        }
        Ok(path)
    }

    fn read_script(dir: &File, ui: Ui, expected_digest: &str) -> Result<String, String> {
        let bytes = read_file(dir, script_path(&ui.entry)?, ENTRY_MAX)?;
        let digest = format!(
            "sha256:{}",
            hoh_protocol::approval::hex(&Sha256::digest(&bytes))
        );
        if digest != expected_digest {
            return Err("The UI entry does not match its declared SHA-256 digest.".into());
        }
        let source =
            String::from_utf8(bytes).map_err(|_| "The UI entry must be UTF-8 JavaScript.")?;
        if source.contains('\0')
            || source.contains("<!--")
            || source.to_ascii_lowercase().contains("</script")
        {
            return Err(
                "The UI entry contains markup that must be escaped in the JavaScript bundle."
                    .into(),
            );
        }
        Ok(source.replace("\r\n", "\n").replace('\r', "\n"))
    }

    pub(super) fn catalog(profile: &Path) -> Result<Catalog, String> {
        let (path, directory) = folder(profile)?;
        let mut catalog = Catalog {
            directory: path.to_string_lossy().into_owned(),
            packages: Vec::new(),
            rejected: Vec::new(),
            prepared: None,
        };
        let Some(directory) = directory else {
            return Ok(catalog);
        };
        for entry in catalog_entries(&directory)? {
            let Ok(id) = entry.file_name().into_string() else {
                continue;
            };
            match read(&directory, &id) {
                Ok(snapshot) => catalog.packages.push(snapshot.package),
                Err(reason) => catalog.rejected.push(Rejection { id, reason }),
            }
        }
        catalog.packages.sort_by(|a, b| a.id.cmp(&b.id));
        catalog.rejected.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(catalog)
    }

    fn catalog_entries(directory: &File) -> Result<Vec<fs::DirEntry>, String> {
        let entries = fs::read_dir(descriptor_path(directory))
            .map_err(|_| "The external_plugins directory could not be listed.")?;
        let mut visible = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|_| "A plugin folder could not be listed.")?;
            if entry.file_name().as_bytes().starts_with(b".") {
                continue;
            }
            if visible.len() == MAX_PACKAGES {
                return Err(format!(
                    "The external_plugins directory is limited to {MAX_PACKAGES} entries."
                ));
            }
            visible.push(entry);
        }
        Ok(visible)
    }

    fn descriptor_path(directory: &File) -> PathBuf {
        PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()))
    }

    pub(super) fn prepared_path() -> Option<PathBuf> {
        std::env::var_os("HOH_PLUGIN_PACKAGE")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    }

    fn source_package(path: &Path) -> Result<(&str, File), String> {
        let id = path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|id| valid_id(id))
            .ok_or("Use a lowercase alphanumeric folder ID, with optional hyphens.")?;
        let source = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(path)
            .map_err(|_| "The plugin folder could not be opened safely.")?;
        Ok((id, source))
    }

    pub(super) fn catalog_with_prepared(profile: &Path) -> Result<Catalog, String> {
        let mut catalog = catalog(profile)?;
        if let Some(path) = prepared_path()
            && let Ok((id, source)) = source_package(&path)
            && !catalog.packages.iter().any(|plugin| plugin.id == id)
        {
            catalog.prepared = read_package(&source, id)
                .ok()
                .map(|snapshot| snapshot.package);
        }
        Ok(catalog)
    }

    pub(super) fn install(
        profile: &Path,
        source: &Path,
        expected: Option<&PagePlugin>,
    ) -> Result<PagePlugin, String> {
        let (id, source) = source_package(source)?;
        let manifest_bytes = read_file(&source, Path::new("hoh-package.json"), MANIFEST_MAX)?;
        let manifest = parse_manifest(&manifest_bytes)?;
        let entry = manifest
            .ui
            .as_ref()
            .map(|ui| {
                let path = script_path(&ui.entry)?;
                Ok::<_, String>((path, read_file(&source, path, ENTRY_MAX)?))
            })
            .transpose()?;

        let (path, installed) = folder(profile)?;
        let installed = match installed {
            Some(installed) => installed,
            None => {
                match fs::DirBuilder::new().mode(0o700).create(path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => {
                        return Err(format!("Could not create the plugin directory: {error}"));
                    }
                }
                folder(profile)?
                    .1
                    .ok_or("The plugin directory is missing.")?
            }
        };
        installed
            .lock()
            .map_err(|error| format!("Could not lock the plugin directory: {error}"))?;
        let root = descriptor_path(&installed);
        let mut random = [0; 16];
        File::open("/dev/urandom")
            .and_then(|mut file| file.read_exact(&mut random))
            .map_err(|error| format!("Could not prepare a plugin snapshot: {error}"))?;
        let staging_name = format!(".install-{}", hoh_protocol::approval::hex(&random));
        let staging = root.join(&staging_name);
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&staging)
            .map_err(|error| format!("Could not stage the plugin: {error}"))?;
        let result = (|| {
            fs::write(staging.join("hoh-package.json"), manifest_bytes)
                .map_err(|error| format!("Could not write the plugin manifest: {error}"))?;
            if let Some((path, bytes)) = entry {
                let entry = staging.join(path);
                fs::create_dir_all(entry.parent().expect("entry inside staging"))
                    .and_then(|()| fs::write(entry, bytes))
                    .map_err(|error| format!("Could not write the plugin UI: {error}"))?;
            }
            let directory = open_beneath(
                &installed,
                Path::new(&staging_name),
                libc::O_RDONLY | libc::O_DIRECTORY,
            )
            .map_err(|_| "The staged plugin folder could not be opened safely.")?;
            let package = read_package(&directory, id)?.package;
            if expected.is_some_and(|expected| *expected != package) {
                return Err("The prepared plugin changed. Refresh Plugins and try again.".into());
            }
            let entries = catalog_entries(&installed)?;
            if entries.iter().any(|entry| entry.file_name() == id) {
                if read(&installed, id).is_ok_and(|existing| existing.package == package) {
                    return Ok(package);
                }
                return Err(format!(
                    "A different package is already installed with ID {id}."
                ));
            }
            if entries.len() == MAX_PACKAGES {
                return Err(format!(
                    "The plugin catalog is full ({MAX_PACKAGES} packages)."
                ));
            }
            let from = CString::new(staging_name).expect("generated staging name");
            let to = CString::new(id).expect("validated package ID");
            // SAFETY: both C strings and the shared directory descriptor outlive the call.
            let status = unsafe {
                libc::renameat2(
                    installed.as_raw_fd(),
                    from.as_ptr(),
                    installed.as_raw_fd(),
                    to.as_ptr(),
                    libc::RENAME_NOREPLACE,
                )
            };
            if status != 0 {
                let error = std::io::Error::last_os_error();
                return Err(format!("Could not install the plugin: {error}"));
            }
            Ok(package)
        })();
        if staging.exists() {
            let _ = fs::remove_dir_all(staging);
        }
        result
    }

    pub(super) fn open(profile: &Path, expected: &PagePlugin) -> Result<Snapshot, String> {
        let (_, directory) = folder(profile)?;
        let directory = directory.ok_or("The external_plugins directory is missing.")?;
        let snapshot = read(&directory, &expected.id)?;
        if snapshot.package != *expected {
            return Err(
                "The plugin changed since the catalog was read. Refresh and review it again."
                    .into(),
            );
        }
        Ok(snapshot)
    }

    struct PanelView {
        id: u32,
        widget: gtk::Box,
        view: webkit2gtk::WebView,
        bridge: Rc<PanelBridge>,
    }

    struct PanelBridge {
        events: Channel<PanelEvent>,
        queue: RefCell<Vec<QueueItem>>,
        revision: Cell<u32>,
        activation: Cell<Option<Instant>>,
    }

    thread_local! {
        static PANEL: RefCell<Option<PanelView>> = const { RefCell::new(None) };
        static WORKSPACE: RefCell<Option<WorkspaceView>> = const { RefCell::new(None) };
    }

    struct WorkspaceView {
        overlay: gtk::Overlay,
        host: webkit2gtk::WebView,
        page: Option<(PagePlugin, webkit2gtk::WebView)>,
    }

    fn detach_workspace(overlay: &gtk::Overlay, view: &webkit2gtk::WebView) {
        use gtk::prelude::*;
        view.hide();
        if view.parent().is_some() {
            overlay.remove(view);
        }
    }

    impl WorkspaceView {
        fn new(parent: &gtk::Box) -> Result<Self, String> {
            use gtk::prelude::*;
            let host = parent
                .children()
                .into_iter()
                .find_map(|child| child.downcast::<webkit2gtk::WebView>().ok())
                .ok_or("The main HoH view is unavailable.")?;
            let overlay = gtk::Overlay::new();
            parent.pack_start(&overlay, true, true, 0);
            parent.remove(&host);
            overlay.add(&host);
            overlay.show();
            Ok(Self {
                overlay,
                host,
                page: None,
            })
        }
    }

    pub(super) fn open_workspace(
        parent: &gtk::Box,
        snapshot: Snapshot,
        engine: Option<std::sync::Arc<crate::engine::Engine>>,
    ) -> Result<(), String> {
        use gtk::prelude::*;
        use webkit2gtk::WebViewExt;
        WORKSPACE.with_borrow_mut(|workspace| {
            if workspace.is_none() {
                *workspace = Some(WorkspaceView::new(parent)?);
            }
            let workspace = workspace.as_mut().expect("workspace overlay was installed");
            if workspace
                .page
                .as_ref()
                .is_some_and(|(active, _)| *active == snapshot.package)
            {
                return Ok(());
            }
            let secretary_engine = if snapshot.secretary {
                Some(engine.ok_or("The HoH model bridge is unavailable.")?)
            } else {
                None
            };
            let (view, url) = isolated_view(snapshot.script, None, true, secretary_engine)?;
            if let Some((_, previous)) = workspace.page.take() {
                detach_workspace(&workspace.overlay, &previous);
            }
            view.set_halign(gtk::Align::Start);
            view.set_valign(gtk::Align::Start);
            view.set_no_show_all(true);
            view.load_uri(&url);
            workspace.page = Some((snapshot.package, view));
            Ok(())
        })
    }

    pub(super) fn owns_workspace_view(view: &webkit2gtk::WebView) -> bool {
        WORKSPACE.with_borrow(|workspace| {
            workspace
                .as_ref()
                .and_then(|workspace| workspace.page.as_ref())
                .is_some_and(|(_, active)| active == view)
        })
    }

    pub(super) fn layout_workspace(id: &str, bounds: Option<WorkspaceBounds>) {
        use gtk::prelude::*;
        use webkit2gtk::WebViewExt;
        WORKSPACE.with_borrow(|workspace| {
            let Some(workspace) = workspace.as_ref() else {
                return;
            };
            let Some((_, view)) = workspace.page.as_ref().filter(|(page, _)| page.id == id) else {
                return;
            };
            let Some(bounds) = bounds else {
                detach_workspace(&workspace.overlay, view);
                return;
            };
            if let Some(parent) = workspace.overlay.parent().and_downcast::<gtk::Box>() {
                close_panel(&parent);
            }
            // DOM rectangles are in CSS pixels; GTK allocates in logical pixels.
            let zoom = workspace.host.zoom_level();
            let pixels = |value: f64| (value * zoom).round() as i32;
            let x = pixels(bounds.x);
            let y = pixels(bounds.y);
            view.set_zoom_level(zoom);
            view.set_margin_start(x);
            view.set_margin_top(y);
            view.set_size_request(
                pixels(bounds.x + bounds.width) - x,
                pixels(bounds.y + bounds.height) - y,
            );
            if view.parent().is_none() {
                workspace.overlay.add_overlay(view);
            }
            view.show();
        });
    }

    fn publish(view: &webkit2gtk::WebView, bridge: &PanelBridge) {
        use webkit2gtk::WebViewExt;
        let Ok(queue) = serde_json::to_string(&*bridge.queue.borrow()) else {
            return;
        };
        view.evaluate_javascript(
            &format!(
                "window.dispatchEvent(new CustomEvent('hoh-review-queue', {{detail: {queue}}}));"
            ),
            None,
            None,
            None::<&gtk::gio::Cancellable>,
            |_| {},
        );
    }

    pub(super) fn update_panel(id: u32, revision: u32, queue: Vec<QueueItem>) {
        PANEL.with_borrow(|active| {
            if let Some(panel) = active.as_ref().filter(|panel| panel.id == id)
                && revision > panel.bridge.revision.get()
            {
                panel.bridge.revision.set(revision);
                panel.bridge.queue.replace(queue);
                publish(&panel.view, &panel.bridge);
            }
        });
    }

    fn close_panel(parent: &gtk::Box) {
        use gtk::prelude::*;
        if let Some(panel) = PANEL.with_borrow_mut(Option::take) {
            panel.bridge.queue.borrow_mut().clear();
            parent.remove(&panel.widget);
            parent.set_orientation(gtk::Orientation::Vertical);
            let _ = panel.bridge.events.send(PanelEvent::Closed);
        }
    }

    pub(super) fn show_panel(
        parent: &gtk::Box,
        snapshot: Snapshot,
        id: u32,
        events: Channel<PanelEvent>,
    ) -> Result<(), String> {
        use gtk::prelude::*;
        use webkit2gtk::{LoadEvent, WebViewExt};
        let title = format!("{} · {}", snapshot.package.name, snapshot.package.id);
        let bridge = Rc::new(PanelBridge {
            events,
            queue: RefCell::new(Vec::new()),
            revision: Cell::new(0),
            activation: Cell::new(None),
        });
        let (view, url) = isolated_view(snapshot.script, Some(Rc::clone(&bridge)), false, None)?;
        close_panel(parent);
        let panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
        panel.set_width_request((parent.allocated_width() / 3).clamp(180, 380));
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.set_margin_start(10);
        header.set_margin_end(6);
        header.set_margin_top(6);
        header.set_margin_bottom(6);
        let label = gtk::Label::new(Some(&title));
        label.set_xalign(0.0);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        header.pack_start(&label, true, true, 0);
        let close = gtk::Button::with_label("Close plugin");
        let weak_parent = parent.downgrade();
        close.connect_clicked(move |_| {
            if let Some(parent) = weak_parent.upgrade() {
                close_panel(&parent);
            }
        });
        header.pack_end(&close, false, false, 0);
        panel.pack_start(&header, false, false, 0);
        panel.pack_start(
            &gtk::Separator::new(gtk::Orientation::Horizontal),
            false,
            false,
            0,
        );
        panel.pack_start(&view, true, true, 0);
        let load_bridge = Rc::clone(&bridge);
        view.connect_load_changed(move |view, event| {
            if event == LoadEvent::Finished {
                publish(view, &load_bridge);
            }
        });
        parent.set_orientation(gtk::Orientation::Horizontal);
        parent.pack_end(&panel, false, false, 0);
        PANEL.with_borrow_mut(|active| {
            *active = Some(PanelView {
                id,
                widget: panel.clone(),
                view: view.clone(),
                bridge,
            })
        });
        panel.show_all();
        view.load_uri(&url);
        Ok(())
    }

    fn isolated_view(
        script: String,
        panel: Option<Rc<PanelBridge>>,
        workspace: bool,
        secretary_engine: Option<std::sync::Arc<crate::engine::Engine>>,
    ) -> Result<(webkit2gtk::WebView, String), String> {
        use gtk::{
            gio::MemoryInputStream,
            glib::{Bytes, Propagation},
            prelude::*,
        };
        use soup::{MessageHeaders, MessageHeadersType};
        use webkit2gtk::{
            DownloadExt, FileChooserRequestExt, NavigationPolicyDecision,
            NavigationPolicyDecisionExt, NetworkProxyMode, NetworkProxySettings,
            PermissionRequestExt, PolicyDecisionExt, PolicyDecisionType, SecurityManagerExt,
            SettingsExt, URIRequestExt, URISchemeRequestExt, URISchemeResponse,
            URISchemeResponseExt, UserContentManager, UserContentManagerExt, WebContext,
            WebContextExt, WebView, WebViewExt, WebsiteDataManagerExt,
        };

        let mut random = [0; 24];
        File::open("/dev/urandom")
            .and_then(|mut file| file.read_exact(&mut random))
            .map_err(|_| "The plugin view could not get an isolated origin.")?;
        let url = format!(
            "{SCHEME}://{}.localhost/page",
            hoh_protocol::approval::hex(&random)
        );
        let hash = STANDARD.encode(Sha256::digest(script.as_bytes()));
        let style = if workspace || panel.is_some() {
            "'unsafe-inline'"
        } else {
            "'none'"
        };
        let placement = if workspace {
            r#" data-hoh-placement="workspace""#
        } else {
            ""
        };
        let csp = format!(
            "default-src 'none'; script-src 'sha256-{hash}'; style-src {style}; img-src data:; font-src 'none'; media-src 'none'; connect-src 'none'; frame-src 'none'; child-src 'none'; worker-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'; sandbox allow-scripts"
        );
        let body = format!(r#"<!doctype html><meta charset="utf-8"><meta name="referrer" content="no-referrer"><meta name="viewport" content="width=device-width, initial-scale=1"><main id="hoh-plugin-root"{placement}></main><script type="module">{script}</script>"#).into_bytes();

        let context = WebContext::new_ephemeral();
        context.set_sandbox_enabled(true);
        if !context.is_ephemeral() || !context.is_sandbox_enabled() {
            return Err("The platform could not provide an isolated plugin view.".into());
        }
        let security = context
            .security_manager()
            .ok_or("Plugin origin isolation is unavailable.")?;
        security.register_uri_scheme_as_no_access(SCHEME);
        security.register_uri_scheme_as_display_isolated(SCHEME);
        // As an additional transport boundary, route any network request to an exclusively
        // held loopback listener which never accepts or replies. There is no direct proxy fallback.
        let sink = std::net::TcpListener::bind(("127.0.0.1", 0))
            .map_err(|_| "The plugin network boundary could not be established.")?;
        let proxy = format!(
            "http://{}",
            sink.local_addr()
                .map_err(|_| "The plugin network boundary could not be established.")?
        );
        let data = context
            .website_data_manager()
            .ok_or("Plugin storage isolation is unavailable.")?;
        let mut network = NetworkProxySettings::new(Some(&proxy), &[]);
        data.set_network_proxy_settings(NetworkProxyMode::Custom, Some(&mut network));
        context.connect_download_started(|_, download| download.cancel());

        let manager = UserContentManager::new();
        let builder = WebView::builder()
            .web_context(&context)
            .user_content_manager(&manager);
        if let Some(bridge) = &panel {
            use javascriptcore::ValueExt;

            let bridge = Rc::clone(bridge);
            manager.connect_script_message_received(Some("hohOpenChat"), move |_, result| {
                let Some(value) = result.js_value().filter(|value| value.is_string()) else {
                    return;
                };
                let thread_id = value.to_str();
                if bridge
                    .queue
                    .borrow()
                    .iter()
                    .any(|item| item.thread_id == thread_id.as_str())
                    && bridge
                        .activation
                        .take()
                        .is_some_and(|at| at.elapsed() < Duration::from_millis(750))
                {
                    let _ = bridge.events.send(PanelEvent::OpenChat {
                        thread_id: thread_id.to_string(),
                    });
                }
            });
            if !manager.register_script_message_handler("hohOpenChat") {
                return Err("The plugin navigation bridge could not be registered.".into());
            }
        }
        let view = builder.build();
        if let Some(engine) = secretary_engine {
            secretary::attach(&view, &manager, engine)?;
        }
        let settings =
            WebViewExt::settings(&view).ok_or("Plugin view restrictions are unavailable.")?;
        settings.set_enable_developer_extras(false);
        settings.set_enable_dns_prefetching(false);
        settings.set_enable_html5_local_storage(false);
        settings.set_enable_html5_database(false);
        settings.set_enable_hyperlink_auditing(false);
        settings.set_enable_fullscreen(false);
        settings.set_enable_media(false);
        settings.set_enable_media_stream(false);
        settings.set_enable_webrtc(false);
        settings.set_allow_file_access_from_file_urls(false);
        settings.set_allow_universal_access_from_file_urls(false);
        settings.set_javascript_can_access_clipboard(false);
        settings.set_javascript_can_open_windows_automatically(false);
        view.drag_dest_unset();
        view.connect_create(|_, _| None);
        view.connect_context_menu(|_, _, _, _| true);
        view.connect_permission_request(|_, request| {
            request.deny();
            true
        });
        view.connect_run_file_chooser(|_, request| {
            request.cancel();
            true
        });
        let pointer = panel.clone();
        view.connect_button_press_event(move |_, event| {
            if event.button() == 1
                && let Some(bridge) = &pointer
            {
                bridge.activation.set(Some(Instant::now()));
            }
            if event.button() == 2 {
                Propagation::Stop
            } else {
                Propagation::Proceed
            }
        });
        view.connect_key_press_event(move |_, event| {
            use gtk::gdk::{ModifierType, keys::constants as key};
            let pressed = event.keyval();
            let modifiers = event.state();
            if matches!(pressed, key::Return | key::KP_Enter | key::space)
                && let Some(bridge) = &panel
            {
                bridge.activation.set(Some(Instant::now()));
            }
            if ((pressed == key::v || pressed == key::V)
                && modifiers.contains(ModifierType::CONTROL_MASK))
                || (pressed == key::Insert && modifiers.contains(ModifierType::SHIFT_MASK))
            {
                Propagation::Stop
            } else {
                Propagation::Proceed
            }
        });

        let address = url.clone();
        let owner = view.downgrade();
        context.register_uri_scheme(SCHEME, move |request| {
            let _keep_network_sink = &sink;
            let matches = request.uri().as_deref() == Some(address.as_str())
                && request.http_method().as_deref() == Some("GET")
                && owner
                    .upgrade()
                    .is_some_and(|view| request.web_view() == Some(view));
            let bytes = if matches { body.clone() } else { Vec::new() };
            let length = bytes.len() as i64;
            let stream = MemoryInputStream::from_bytes(&Bytes::from_owned(bytes));
            let response = URISchemeResponse::new(&stream, length);
            response.set_status(if matches { 200 } else { 404 }, None);
            response.set_content_type("text/html; charset=utf-8");
            let headers = MessageHeaders::new(MessageHeadersType::Response);
            headers.append("Content-Security-Policy", &csp);
            headers.append("X-Content-Type-Options", "nosniff");
            headers.append("Cache-Control", "no-store");
            headers.append("Referrer-Policy", "no-referrer");
            headers.append(
                "Permissions-Policy",
                "camera=(), microphone=(), geolocation=(), clipboard-read=(), clipboard-write=()",
            );
            response.set_http_headers(headers);
            request.finish_with_response(&response);
        });

        let address = url.clone();
        view.connect_decide_policy(move |_, decision, kind| {
            if kind == PolicyDecisionType::Response {
                return false; // Only the response to the one allowed navigation can be reached.
            }
            let allow = kind == PolicyDecisionType::NavigationAction
                && decision
                    .downcast_ref::<NavigationPolicyDecision>()
                    .and_then(|decision| decision.navigation_action())
                    .and_then(|action| action.request())
                    .and_then(|request| request.uri())
                    .is_some_and(|uri| uri.as_str() == address);
            if !allow {
                decision.ignore();
            }
            !allow
        });

        Ok((view, url))
    }

    pub(super) fn show(parent: &gtk::ApplicationWindow, snapshot: Snapshot) -> Result<(), String> {
        use gtk::prelude::*;
        use webkit2gtk::WebViewExt;
        let (view, url) = isolated_view(snapshot.script, None, false, None)?;
        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        let title = format!(
            "HoH — Plugin {} ({})",
            snapshot.package.name, snapshot.package.id
        );
        window.set_title(&title);
        let header = gtk::HeaderBar::new();
        header.set_title(Some(&title));
        header.set_show_close_button(true);
        window.set_titlebar(Some(&header));
        window.set_transient_for(Some(parent));
        window.set_destroy_with_parent(true);
        window.set_default_size(900, 650);
        window.add(&view);
        view.load_uri(&url);
        window.show_all();
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::os::unix::fs::symlink;

        struct Profile(PathBuf);

        impl Profile {
            fn new() -> Self {
                let unique = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos();
                let path = std::env::temp_dir()
                    .join(format!("hoh-ui-plugins-{}-{unique}", std::process::id()));
                fs::create_dir(&path).unwrap();
                Self(path)
            }
        }

        impl Drop for Profile {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }

        #[test]
        fn installs_a_validated_snapshot_without_replacing_an_existing_package() {
            let profile = Profile::new();
            let author = Profile::new();
            let source = author.0.join("hello-workspace");
            fs::create_dir(&source).unwrap();
            let manifest = include_str!("../../../docs/examples/hello-workspace/hoh-package.json");
            let script = include_bytes!("../../../docs/examples/hello-workspace/ui.js");
            fs::write(source.join("hoh-package.json"), manifest).unwrap();
            fs::write(source.join("ui.js"), b"bad digest").unwrap();
            assert!(install(&profile.0, &source, None).is_err());
            assert!(catalog(&profile.0).unwrap().rejected.is_empty());

            fs::write(source.join("ui.js"), script).unwrap();
            let (_, source_dir) = source_package(&source).unwrap();
            let expected = read_package(&source_dir, "hello-workspace")
                .unwrap()
                .package;
            let installed = install(&profile.0, &source, Some(&expected)).unwrap();
            assert_eq!(installed, expected);
            assert_eq!(install(&profile.0, &source, None).unwrap(), installed);

            let updated = manifest.replace("Hello Workspace", "Changed Workspace");
            fs::write(source.join("hoh-package.json"), updated).unwrap();
            assert!(install(&profile.0, &source, Some(&expected)).is_err());
            assert!(install(&profile.0, &source, None).is_err());
            fs::remove_dir_all(&source).unwrap();
            assert_eq!(catalog(&profile.0).unwrap().packages, [installed.clone()]);
            assert!(open(&profile.0, &installed).unwrap().is_workspace_page());
            assert_eq!(
                fs::read_dir(profile.0.join("external_plugins"))
                    .unwrap()
                    .count(),
                1
            );
        }

        #[test]
        fn full_catalog_remains_readable_after_a_failed_install() {
            let profile = Profile::new();
            let author = Profile::new();
            let source = author.0.join("hello-workspace");
            fs::create_dir(&source).unwrap();
            fs::write(
                source.join("hoh-package.json"),
                include_bytes!("../../../docs/examples/hello-workspace/hoh-package.json"),
            )
            .unwrap();
            fs::write(
                source.join("ui.js"),
                include_bytes!("../../../docs/examples/hello-workspace/ui.js"),
            )
            .unwrap();
            let directory = profile.0.join("external_plugins");
            fs::create_dir(&directory).unwrap();
            fs::create_dir(directory.join(".leftover-stage")).unwrap();
            for index in 0..MAX_PACKAGES {
                fs::create_dir(directory.join(format!("package-{index}"))).unwrap();
            }
            assert!(
                install(&profile.0, &source, None)
                    .unwrap_err()
                    .contains("full")
            );
            assert_eq!(catalog(&profile.0).unwrap().rejected.len(), MAX_PACKAGES);
            assert!(!directory.join("hello-workspace").exists());
        }

        #[test]
        fn page_sidebar_label_opts_into_workspace_and_is_verified_on_open() {
            let profile = Profile::new();
            let dir = profile.0.join("external_plugins/hello-workspace");
            fs::create_dir_all(&dir).unwrap();
            let source = include_bytes!("../../../docs/examples/hello-workspace/ui.js");
            let example = include_str!("../../../docs/examples/hello-workspace/hoh-package.json");
            fs::write(dir.join("ui.js"), source).unwrap();
            let path = dir.join("hoh-package.json");
            fs::write(&path, example).unwrap();
            let workspace = catalog(&profile.0).unwrap().packages.pop().unwrap();
            assert_eq!(
                workspace.workspace_label.as_deref(),
                Some("Hello Workspace")
            );
            assert!(open(&profile.0, &workspace).unwrap().is_workspace_page());

            let mut manifest: serde_json::Value = serde_json::from_str(example).unwrap();
            manifest["contributes"]["ui"]["pages"][0]
                .as_object_mut()
                .unwrap()
                .remove("sidebarLabel");
            fs::write(&path, manifest.to_string()).unwrap();
            assert!(open(&profile.0, &workspace).is_err());
            let standalone = catalog(&profile.0).unwrap().packages.pop().unwrap();
            assert!(!open(&profile.0, &standalone).unwrap().is_workspace_page());

            manifest["contributes"]["ui"]["pages"][0]["sidebarLabel"] = " ".into();
            fs::write(&path, manifest.to_string()).unwrap();
            assert_eq!(
                catalog(&profile.0).unwrap().rejected[0].id,
                "hello-workspace"
            );
        }

        #[test]
        fn secretary_requires_workspace_protocol_and_binds_permissions_to_the_catalog() {
            let profile = Profile::new();
            let dir = profile.0.join("external_plugins/hello-workspace");
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join("ui.js"),
                include_bytes!("../../../docs/examples/hello-workspace/ui.js"),
            )
            .unwrap();
            let path = dir.join("hoh-package.json");
            let mut manifest: serde_json::Value = serde_json::from_str(include_str!(
                "../../../docs/examples/hello-workspace/hoh-package.json"
            ))
            .unwrap();
            fs::write(&path, manifest.to_string()).unwrap();
            let v1 = catalog(&profile.0).unwrap().packages.pop().unwrap();
            assert!(!open(&profile.0, &v1).unwrap().secretary);

            manifest["requestedPermissions"]["ui"] = serde_json::json!(["page", "secretary"]);
            fs::write(&path, manifest.to_string()).unwrap();
            assert_eq!(catalog(&profile.0).unwrap().rejected.len(), 1);

            manifest["requires"]["services"]["ui"]["protocolVersions"] = serde_json::json!([4]);
            fs::write(&path, manifest.to_string()).unwrap();
            let v4 = catalog(&profile.0).unwrap().packages.pop().unwrap();
            let snapshot = open(&profile.0, &v4).unwrap();
            assert!(snapshot.secretary);
            assert!(snapshot.is_workspace_page());
            assert!(snapshot.script.contains("hohSecretaryPropose"));
            assert_ne!(v1.digest, v4.digest);
            assert!(open(&profile.0, &v1).is_err());

            manifest["requestedPermissions"]["ui"] = serde_json::json!(["secretary", "page"]);
            fs::write(&path, manifest.to_string()).unwrap();
            assert!(open(&profile.0, &v4).is_err());
            assert_eq!(catalog(&profile.0).unwrap().packages.len(), 1);

            manifest["contributes"]["ui"]["pages"][0]
                .as_object_mut()
                .unwrap()
                .remove("sidebarLabel");
            fs::write(&path, manifest.to_string()).unwrap();
            assert_eq!(catalog(&profile.0).unwrap().rejected.len(), 1);
        }

        #[test]
        fn panel_requires_the_v2_contract_and_is_bound_to_its_manifest() {
            let profile = Profile::new();
            let dir = profile.0.join("external_plugins").join("panel");
            fs::create_dir_all(&dir).unwrap();
            let source = "export function mount(root, sdk) {}";
            fs::write(dir.join("ui.js"), source).unwrap();
            let digest = format!(
                "sha256:{}",
                hoh_protocol::approval::hex(&Sha256::digest(source))
            );
            let mut manifest = serde_json::json!({
                "formatVersion": 2, "id": "panel", "version": "1.0.0", "digest": digest,
                "ui": {"entry": "ui.js"},
                "requestedPermissions": {"ui": ["sidebar", "chatPanel", "reviewQueue", "openChat"]},
                "contributes": {"ui": {"chatPanel": {"title": "Panel", "sidebarLabel": "Panel menu"}}},
                "requires": {"services": {"ui": {"protocolVersions": [2]}}}
            });
            let path = dir.join("hoh-package.json");
            fs::write(&path, manifest.to_string()).unwrap();
            let package = catalog(&profile.0).unwrap().packages.pop().unwrap();
            assert_eq!(
                package.chat_panel.as_ref().unwrap().sidebar_label,
                "Panel menu"
            );
            assert!(
                open(&profile.0, &package)
                    .unwrap()
                    .script
                    .contains("reviewQueue")
            );
            manifest["contributes"]["ui"]["chatPanel"]["sidebarLabel"] = "Changed".into();
            fs::write(&path, manifest.to_string()).unwrap();
            assert!(open(&profile.0, &package).is_err());
            manifest["requestedPermissions"]["ui"] =
                serde_json::json!(["sidebar", "chatPanel", "reviewQueue"]);
            fs::write(&path, manifest.to_string()).unwrap();
            assert_eq!(catalog(&profile.0).unwrap().rejected[0].id, "panel");
        }

        #[test]
        fn catalog_is_bound_to_installed_packages_and_reviewed_content() {
            let profile = Profile::new();
            assert!(catalog(&profile.0).unwrap().packages.is_empty());
            let root = profile.0.join("external_plugins");
            fs::create_dir(&root).unwrap();
            for (id, script) in [
                (
                    "first",
                    "export function mount(root) { root.textContent = 'first'; }",
                ),
                (
                    "second",
                    "export function mount(root) { root.textContent = 'second'; }",
                ),
            ] {
                let dir = root.join(id);
                fs::create_dir(&dir).unwrap();
                let digest = format!(
                    "sha256:{}",
                    hoh_protocol::approval::hex(&Sha256::digest(script))
                );
                let manifest = serde_json::json!({
                    "formatVersion": 2, "id": id, "version": "1.0.0", "digest": digest,
                    "ui": {"entry":"ui.js"}, "requestedPermissions":{"ui":["page"]},
                    "contributes":{"ui":{"pages":[{"id":"main","name":id,"handler":"mount"}]}},
                    "requires":{"services":{"ui":{"protocolVersions":[1]}}}
                });
                fs::write(dir.join("hoh-package.json"), manifest.to_string()).unwrap();
                fs::write(dir.join("ui.js"), script).unwrap();
            }
            symlink(root.join("first"), root.join("shortcut")).unwrap();
            let found = catalog(&profile.0).unwrap();
            assert_eq!(
                found
                    .packages
                    .iter()
                    .map(|p| p.id.as_str())
                    .collect::<Vec<_>>(),
                ["first", "second"]
            );
            assert_eq!(found.rejected[0].id, "shortcut");
            let first = open(&profile.0, &found.packages[0]).unwrap();
            let second = open(&profile.0, &found.packages[1]).unwrap();
            let first_script = first.script;
            assert!(first_script.contains("'first'"));
            assert!(!first_script.contains("'second'"));
            assert!(second.script.contains("'second'"));

            fs::write(root.join("first/ui.js"), "changed").unwrap();
            assert!(open(&profile.0, &found.packages[0]).is_err());
            assert!(open(&profile.0, &found.packages[1]).is_ok());
        }
    }
}
