mod candidate;
mod extract;
mod install;
mod instance;
mod recovery;
pub use install::Installation;

use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    ffi::{CString, OsStr},
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Read, Write},
    os::{
        fd::AsRawFd,
        unix::{
            ffi::OsStrExt,
            fs::{OpenOptionsExt, PermissionsExt},
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use tauri::{Manager, ipc::Channel};

const DEFAULT_RELEASE_REPOSITORY: &str = "alexandrie-dev/harness-over-harness";
const MAX_ASSET_SIZE: u64 = 512 * 1024 * 1024;
const MAX_METADATA_SIZE: u64 = 4096;
const INVALID_PACKAGE: &str =
    "The package does not contain a complete HoH installation for this release.";

#[derive(Clone, Copy)]
struct Package {
    name: &'static str,
    extension: &'static str,
    install_dir: &'static str,
    binary_dir: &'static str,
    version_file: &'static str,
}

fn package_for(os: &str, arch: &str) -> Option<Package> {
    let name = match (os, arch) {
        ("linux", "x86_64") => "HoH-linux-x86_64",
        ("linux", "aarch64") => "HoH-linux-aarch64",
        ("macos", "aarch64") => {
            return Some(Package {
                name: "HoH-macos-arm64",
                extension: "zip",
                install_dir: "HoH.app",
                binary_dir: "Contents/MacOS",
                version_file: "Contents/Resources/.hoh-release",
            });
        }
        _ => return None,
    };
    Some(Package {
        name,
        extension: "tar.zst",
        install_dir: "app",
        binary_dir: "",
        version_file: ".hoh-release",
    })
}

impl Package {
    fn asset_name(self) -> String {
        format!("{}.{}", self.name, self.extension)
    }

    fn staged_in(self, unpacked: &Path) -> io::Result<PathBuf> {
        let relative = if self.extension == "zip" {
            PathBuf::from(self.install_dir)
        } else {
            Path::new(self.name).join(self.install_dir)
        };
        package_directory(unpacked, &relative)
    }

    fn installed_at(self, executable: &Path) -> Option<&Path> {
        let binary = Path::new(self.binary_dir).join("hoh-desktop");
        let root = executable.ancestors().nth(binary.components().count())?;
        let valid_root = if self.extension == "zip" {
            root.extension() == Some(OsStr::new("app"))
        } else {
            root.file_name() == Some(OsStr::new(self.install_dir))
        };
        ((valid_root || recovery::is_backup(root))
            && executable.ends_with(&binary)
            && root.join(self.version_file).symlink_metadata().is_ok())
        .then_some(root)
    }

    fn lease_path(self, bundle: &Path) -> PathBuf {
        if cfg!(target_os = "linux") {
            bundle.join(self.version_file)
        } else {
            bundle.to_owned()
        }
    }

    fn open_lease(self, bundle: &Path) -> io::Result<File> {
        let path = self.lease_path(bundle);
        if cfg!(target_os = "linux") {
            open_regular_file(&path)
        } else {
            File::open(path)
        }
    }
}

fn package_directory(root: &Path, relative: &Path) -> io::Result<PathBuf> {
    let mut directory = root.to_owned();
    let require_directory = |path: &Path| -> io::Result<()> {
        if path.symlink_metadata()?.is_dir() {
            Ok(())
        } else {
            Err(io::ErrorKind::InvalidData.into())
        }
    };
    require_directory(&directory)?;
    for component in relative.components() {
        directory.push(component.as_os_str());
        require_directory(&directory)?;
    }
    Ok(directory)
}

fn open_regular_file(path: &Path) -> io::Result<File> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "expected a regular file");
    if !path.symlink_metadata()?.is_file() {
        return Err(invalid());
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(invalid());
    }
    Ok(file)
}

fn read_install_metadata(path: &Path) -> io::Result<String> {
    let file = open_regular_file(path)?;
    let mut value = String::new();
    file.take(MAX_METADATA_SIZE + 1).read_to_string(&mut value)?;
    if value.len() as u64 > MAX_METADATA_SIZE {
        return Err(io::ErrorKind::FileTooLarge.into());
    }
    Ok(value)
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}

impl Release {
    fn asset(&self, package: Package) -> Option<&Asset> {
        let name = package.asset_name();
        self.assets
            .iter()
            .find(|asset| asset.name == name && asset.state == "uploaded")
    }
}

#[derive(Deserialize)]
struct Asset {
    id: u64,
    name: String,
    state: String,
    size: u64,
    digest: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "version")]
pub enum UpdateOutcome {
    UnsupportedInstall,
    NoRelease,
    UpToDate,
    RolledBack,
    NoCompatiblePackage(Version),
    Available(Version),
    ManualUpdateAvailable(Version),
    Restarting(Version),
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "version")]
pub enum UpdateProgress {
    Downloading(Version),
    Installing(Version),
}

fn available_version(release: &Release, installed: &Version) -> Result<Option<Version>, String> {
    if release.draft || release.prerelease {
        return Ok(None);
    }
    let tag = &release.tag_name;
    let version = Version::parse(tag.strip_prefix('v').unwrap_or(tag))
        .map_err(|_| "The latest GitHub release tag must be a semantic version, such as v0.2.0.")?;
    Ok((version.pre.is_empty() && version.cmp_precedence(installed).is_gt()).then_some(version))
}

enum Response {
    Metadata,
    Archive(u64),
}

enum StagingError {
    Failed(String),
    ShutdownUnconfirmed(String),
}

impl From<String> for StagingError {
    fn from(error: String) -> Self {
        Self::Failed(error)
    }
}

impl From<&str> for StagingError {
    fn from(error: &str) -> Self {
        Self::Failed(error.into())
    }
}

fn retain_if_shutdown_unconfirmed<T>(
    result: Result<T, StagingError>,
    staging: tempfile::TempDir,
) -> Result<(T, tempfile::TempDir), String> {
    match result {
        Ok(value) => Ok((value, staging)),
        Err(StagingError::Failed(error)) => Err(error),
        Err(StagingError::ShutdownUnconfirmed(error)) => {
            let retained = staging.keep();
            Err(format!(
                "{error} Staging was retained at {}.",
                retained.display()
            ))
        }
    }
}

fn fetch(
    url: &str,
    destination: &Path,
    token: Option<&str>,
    response: Response,
    cancelled: impl Fn() -> bool,
) -> Result<u16, StagingError> {
    if cancelled() {
        return Err("The update check was cancelled.".into());
    }
    let mut command = Command::new("/usr/bin/curl");
    let (accept, timeout, limit) = match response {
        Response::Metadata => ("application/vnd.github+json", 20, 2 * 1024 * 1024),
        Response::Archive(size) => {
            unsafe {
                command.pre_exec(move || {
                    let mut limit = std::mem::MaybeUninit::<libc::rlimit>::uninit();
                    if libc::getrlimit(libc::RLIMIT_FSIZE, limit.as_mut_ptr()) != 0 {
                        return Err(io::Error::last_os_error());
                    }
                    let mut limit = limit.assume_init();
                    limit.rlim_cur = limit.rlim_cur.min(size as libc::rlim_t);
                    if libc::setrlimit(libc::RLIMIT_FSIZE, &limit) != 0 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            ("application/octet-stream", 300, size)
        }
    };
    command.process_group(0).arg("--disable");
    if token.is_some() {
        // Send the credential over stdin. curl drops Authorization on cross-host redirects.
        command.args(["--header", "@-"]).stdin(Stdio::piped());
    } else {
        command.stdin(Stdio::null());
    }
    command
        .args([
            "--silent",
            "--location",
            "--max-redirs",
            "5",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "10",
            "--max-time",
            &timeout.to_string(),
            "--max-filesize",
            &limit.to_string(),
            "--user-agent",
            "HoH-Updater",
            "--header",
            &format!("Accept: {accept}"),
            "--output",
        ])
        .arg(destination)
        .args(["--write-out", "%{http_code}", "--", url])
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let deadline = Instant::now() + Duration::from_secs(timeout);
    let mut child = command
        .spawn()
        .map_err(|error| format!("Could not start curl for the update ({error})."))?;
    let header = token.map(|token| {
        writeln!(
            child.stdin.take().expect("curl stdin is piped"),
            "Authorization: Bearer {token}"
        )
    });
    let status =
        install::wait_for_child_group(&mut child, || cancelled() || Instant::now() >= deadline)
            .map_err(|error| match error {
                install::ChildWaitError::Interrupted if cancelled() => {
                    "The update check was cancelled.".into()
                }
                install::ChildWaitError::Interrupted => "The GitHub download took too long.".into(),
                install::ChildWaitError::ShutdownUnconfirmed(error) => {
                    StagingError::ShutdownUnconfirmed(format!(
                        "Could not confirm that curl stopped ({error})."
                    ))
                }
            })?;
    if !status.success() || header.is_some_and(|result| result.is_err()) {
        return Err("Could not download from GitHub. Check your connection and try again.".into());
    }
    if fs::metadata(destination).map_or(true, |file| file.len() > limit) {
        return Err("The GitHub response was missing or exceeded the download limit.".into());
    }
    let mut bytes = String::new();
    child
        .stdout
        .take()
        .expect("curl stdout is piped")
        .read_to_string(&mut bytes)
        .map_err(|error| format!("Could not read the GitHub response status ({error})."))?;
    bytes
        .parse()
        .map_err(|_| "Could not read the GitHub response status.".into())
}

fn github_token(cancelled: impl Fn() -> bool) -> Option<String> {
    // Finder launches with a restricted PATH, so also look in the Homebrew bin directories.
    for binary in ["gh", "/opt/homebrew/bin/gh", "/usr/local/bin/gh"] {
        match Command::new(binary)
            .process_group(0)
            .args(["auth", "token", "--hostname", "github.com"])
            .env("GH_PROMPT_DISABLED", "1")
            .current_dir("/")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                let deadline = Instant::now() + Duration::from_secs(10);
                let status = install::wait_for_child_group(&mut child, || {
                    cancelled() || Instant::now() >= deadline
                })
                .ok()?;
                let stdout = child.stdout.take()?;
                if !status.success()
                    || unsafe { libc::fcntl(stdout.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) }
                        < 0
                {
                    return None;
                }
                let mut token = String::new();
                stdout.take(1024).read_to_string(&mut token).ok()?;
                let token = token.trim();
                return (!token.is_empty()
                    && token.len() < 1024
                    && token.bytes().all(|byte| byte.is_ascii_graphic()))
                .then(|| token.to_owned());
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => return None,
        }
    }
    None
}

fn release_repository() -> Result<&'static str, String> {
    let repo = option_env!("HOH_RELEASE_REPOSITORY").unwrap_or(DEFAULT_RELEASE_REPOSITORY);
    let valid = |component: &str| {
        !component.is_empty()
            && component != "."
            && component != ".."
            && component
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    };
    if repo
        .split_once('/')
        .is_some_and(|(owner, name)| valid(owner) && valid(name))
    {
        Ok(repo)
    } else {
        Err("HOH_RELEASE_REPOSITORY must be a GitHub owner/repository pair.".into())
    }
}

fn latest_release(
    api: &str,
    destination: &Path,
    cancelled: impl Fn() -> bool + Copy,
) -> Result<Option<(Release, Option<String>)>, StagingError> {
    let url = format!("{api}/releases/latest");
    let mut token = None;
    let mut status = fetch(&url, destination, None, Response::Metadata, cancelled)?;
    if status == 404 && fetch(api, destination, None, Response::Metadata, cancelled)? == 200 {
        return Ok(None);
    }
    if matches!(status, 401 | 403 | 404) {
        token = github_token(cancelled);
        if cancelled() {
            return Err("The update check was cancelled.".into());
        }
        if let Some(token) = token.as_deref() {
            status = fetch(
                &url,
                destination,
                Some(token),
                Response::Metadata,
                cancelled,
            )?;
        }
    }
    if status == 404
        && token.is_some()
        && fetch(
            api,
            destination,
            token.as_deref(),
            Response::Metadata,
            cancelled,
        )? == 200
    {
        return Ok(None);
    }
    if matches!(status, 401 | 404) {
        return Err("GitHub Releases is unavailable. For a private repository, run gh auth login and use an account with access.".into());
    }
    if status != 200 {
        return Err(format!("GitHub could not check for updates (HTTP {status}).").into());
    }
    let json =
        fs::read(destination).map_err(|error| format!("Could not read the release ({error})."))?;
    let release = serde_json::from_slice(&json)
        .map_err(|_| "GitHub returned unreadable release metadata.")?;
    Ok(Some((release, token)))
}

impl Asset {
    fn checksum(&self) -> Result<&str, String> {
        self.digest
            .as_deref()
            .and_then(|digest| digest.strip_prefix("sha256:"))
            .filter(|hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .ok_or_else(|| "GitHub did not provide a SHA-256 checksum for this package.".into())
    }

    fn verify(&self, path: &Path, cancelled: impl Fn() -> bool) -> Result<(), String> {
        let read_error = |error| format!("Could not read the downloaded package ({error}).");
        let file = File::open(path).map_err(read_error)?;
        if file.metadata().map_err(read_error)?.len() != self.size {
            return Err("The downloaded package size does not match the release.".into());
        }
        let mut reader = BufReader::new(file);
        let mut hash = Sha256::new();
        loop {
            if cancelled() {
                return Err("The update was cancelled.".into());
            }
            let bytes = reader.fill_buf().map_err(read_error)?;
            if bytes.is_empty() {
                break;
            }
            hash.update(bytes);
            let len = bytes.len();
            reader.consume(len);
        }
        if !hoh_protocol::approval::hex(&hash.finalize()).eq_ignore_ascii_case(self.checksum()?) {
            return Err("The downloaded package does not match the GitHub checksum.".into());
        }
        Ok(())
    }
}

fn validate_bundle_layout(bundle: &Path, package: Package, version: &Version) -> io::Result<()> {
    let marker = Path::new(package.version_file);
    package_directory(bundle, marker.parent().ok_or(io::ErrorKind::InvalidInput)?)?;
    let bin = package_directory(bundle, Path::new(package.binary_dir))?;
    let version_marker = read_install_metadata(&bundle.join(marker))?;
    if version_marker.trim() != version.to_string() {
        return Err(io::ErrorKind::InvalidData.into());
    }
    for program in ["hoh-desktop", "hoh"] {
        let metadata = bin.join(program).symlink_metadata()?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o100 == 0 {
            return Err(io::ErrorKind::InvalidData.into());
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn require_managed_linux_bundle(bundle: &Path) -> io::Result<()> {
    if !bundle.symlink_metadata()?.is_dir() {
        return Err(io::ErrorKind::Unsupported.into());
    }
    for entry in bundle.read_dir()? {
        let entry = entry?;
        if !entry.file_type()?.is_file()
            || !matches!(
                entry.file_name().to_str(),
                Some("hoh-desktop" | "hoh" | ".hoh-release" | "LICENSE" | "NOTICE")
            )
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "the HoH installation contains user-managed files",
            ));
        }
    }
    Ok(())
}

fn validate_bundle(
    bundle: &Path,
    package: Package,
    version: &Version,
    cancelled: impl Fn() -> bool,
) -> Result<(), StagingError> {
    if cancelled() {
        return Err("The update was cancelled.".into());
    }
    validate_bundle_layout(bundle, package, version).map_err(|_| INVALID_PACKAGE)?;
    #[cfg(target_os = "linux")]
    require_managed_linux_bundle(bundle).map_err(|_| INVALID_PACKAGE)?;
    #[cfg(target_os = "macos")]
    {
        let error =
            |error| format!("macOS could not verify the signature of the new HoH.app ({error}).");
        let mut child = Command::new("/usr/bin/codesign")
            .process_group(0)
            .args(["--verify", "--deep", "--strict"])
            .arg(bundle)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(error)?;
        let deadline = Instant::now() + Duration::from_secs(45);
        let status =
            install::wait_for_child_group(&mut child, || cancelled() || Instant::now() >= deadline)
                .map_err(|failure| match failure {
                    install::ChildWaitError::Interrupted if cancelled() => {
                        "The update was cancelled.".into()
                    }
                    install::ChildWaitError::Interrupted => {
                        "macOS signature verification took too long.".into()
                    }
                    install::ChildWaitError::ShutdownUnconfirmed(error) => {
                        StagingError::ShutdownUnconfirmed(format!(
                            "Could not confirm that macOS signature verification stopped ({error})."
                        ))
                    }
                })?;
        if !status.success() {
            return Err("macOS could not verify the signature of the new HoH.app.".into());
        }
    }
    Ok(())
}

// Swap the app and engine as a single unit. A failed swap leaves the current installation intact.
fn swap_directories(installed: &Path, staged: &Path) -> io::Result<()> {
    let installed = CString::new(installed.as_os_str().as_bytes())?;
    let staged = CString::new(staged.as_os_str().as_bytes())?;
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            installed.as_ptr(),
            libc::AT_FDCWD,
            staged.as_ptr(),
            libc::RENAME_EXCHANGE,
        )
    };
    #[cfg(target_os = "macos")]
    let result = unsafe {
        libc::renameatx_np(
            libc::AT_FDCWD,
            installed.as_ptr(),
            libc::AT_FDCWD,
            staged.as_ptr(),
            libc::RENAME_SWAP,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn update(
    executable: &Path,
    installed_version: &Version,
    installation: &Installation,
    progress: Option<(Version, impl Fn(UpdateProgress))>,
) -> Result<UpdateOutcome, String> {
    let Some(package) = package_for(std::env::consts::OS, std::env::consts::ARCH) else {
        return Ok(UpdateOutcome::UnsupportedInstall);
    };
    let Some(installed) = package.installed_at(executable) else {
        return Ok(UpdateOutcome::UnsupportedInstall);
    };
    if !installation.can_update() {
        return Ok(UpdateOutcome::UnsupportedInstall);
    }
    let cancelled = || installation.cancelled();
    let metadata = tempfile::tempdir()
        .map_err(|error| format!("Could not prepare the update check ({error})."))?;
    let repo = release_repository()?;
    let api = format!("https://api.github.com/repos/{repo}");
    let release = latest_release(&api, &metadata.path().join("release.json"), cancelled);
    let (release, metadata) = retain_if_shutdown_unconfirmed(release, metadata)?;
    drop(metadata);
    let Some((release, token)) = release else {
        return Ok(UpdateOutcome::NoRelease);
    };
    let Some(version) = available_version(&release, installed_version)? else {
        return Ok(UpdateOutcome::UpToDate);
    };
    let Some(asset) = release.asset(package) else {
        return Ok(UpdateOutcome::NoCompatiblePackage(version));
    };
    #[cfg(target_os = "linux")]
    if require_managed_linux_bundle(installed).is_err() {
        return Ok(UpdateOutcome::ManualUpdateAvailable(version));
    }
    if asset.size == 0 || asset.size > MAX_ASSET_SIZE {
        return Err("The update package has an unexpected size.".into());
    }
    asset.checksum()?;
    let Some((expected, progress)) = progress else {
        return Ok(UpdateOutcome::Available(version));
    };
    if expected != version {
        return Ok(UpdateOutcome::Available(version));
    }

    let parent = installed
        .parent()
        .ok_or("Could not locate the HoH installation.")?;
    let staging = tempfile::Builder::new()
        .prefix(".hoh-update-")
        .tempdir_in(parent)
        .map_err(|error| format!("Could not prepare the HoH installation folder ({error})."))?;
    extract::check_download_space(staging.path(), asset.size)?;
    let name = package.asset_name();
    progress(UpdateProgress::Downloading(version.clone()));
    let url = if token.is_some() {
        format!("{api}/releases/assets/{}", asset.id)
    } else {
        format!(
            "https://github.com/{repo}/releases/download/{}/{name}",
            release.tag_name
        )
    };
    let archive = staging.path().join(name);
    let download = fetch(
        &url,
        &archive,
        token.as_deref(),
        Response::Archive(asset.size),
        cancelled,
    );
    let (status, staging) = retain_if_shutdown_unconfirmed(download, staging)?;
    if status != 200 {
        return Err(format!(
            "GitHub could not serve the update package (HTTP {status})."
        ));
    }
    asset.verify(&archive, cancelled)?;

    progress(UpdateProgress::Installing(version.clone()));
    let unpacked = staging.path().join("unpacked");
    fs::create_dir(&unpacked)
        .map_err(|error| format!("Could not prepare the package extractor ({error})."))?;
    let extracted = extract::extract(&archive, &unpacked, cancelled);
    let ((), staging) = retain_if_shutdown_unconfirmed(extracted, staging)?;
    let bundle = package.staged_in(&unpacked).map_err(|_| INVALID_PACKAGE)?;
    let validated = validate_bundle(&bundle, package, &version, cancelled);
    let ((), staging) = retain_if_shutdown_unconfirmed(validated, staging)?;
    #[cfg(target_os = "linux")]
    extract::preserve_access_modes(installed, &bundle).map_err(|error| {
        format!("Could not preserve permissions for the HoH installation ({error}).")
    })?;
    candidate::qualify(bundle.join(package.binary_dir).join("hoh"), cancelled)?;
    #[cfg(target_os = "linux")]
    if require_managed_linux_bundle(installed).is_err() {
        return Ok(UpdateOutcome::ManualUpdateAvailable(version));
    }
    installation.handoff(executable, installed, staging, package)?;
    Ok(UpdateOutcome::Restarting(version))
}

async fn run_update(
    window: tauri::WebviewWindow,
    request: Option<(Version, Channel<UpdateProgress>)>,
) -> Result<UpdateOutcome, String> {
    if window.label() != "main" {
        return Err("Updates are available in the main window.".into());
    }
    let app = window.app_handle();
    let installation = app.state::<Installation>();
    let installing = request.is_some();
    if !installing && installation.take_recovered() {
        return Ok(UpdateOutcome::RolledBack);
    }
    let installed_version = app.package_info().version.clone();
    let executable =
        std::env::current_exe().map_err(|error| format!("Could not locate HoH ({error})."))?;
    installation.begin()?;
    let worker_app = app.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        update(
            &executable,
            &installed_version,
            &worker_app.state::<Installation>(),
            request.map(|(version, channel)| {
                (version, move |progress| {
                    let _ = channel.send(progress);
                })
            }),
        )
    })
    .await
    .map_err(|_| "HoH could not complete the update request.");
    if installation.finish() {
        app.exit(0);
    }
    outcome?
}

#[tauri::command]
pub fn desktop_ui_ready(window: tauri::WebviewWindow, legacy: bool, view: u32, ready: bool) {
    let legacy_shell =
        crate::native::legacy_shell_requested(std::env::var_os("HOH_WINDOW_SHELL").as_deref());
    if window.label() == "main" && legacy == legacy_shell {
        window.state::<Installation>().ui_status(view, ready);
    }
}

#[tauri::command]
pub fn open_update_releases(window: tauri::WebviewWindow) -> Result<(), String> {
    if window.label() != "main" {
        return Err("Releases are available in the main window.".into());
    }
    let repo = release_repository()?;
    let url = format!("https://github.com/{repo}/releases");
    crate::opener::open(OsStr::new(&url))
}

#[tauri::command]
pub async fn check_for_update(window: tauri::WebviewWindow) -> Result<UpdateOutcome, String> {
    run_update(window, None).await
}

#[tauri::command]
pub async fn auto_update(
    window: tauri::WebviewWindow,
    expected_version: Version,
    on_progress: Channel<UpdateProgress>,
) -> Result<UpdateOutcome, String> {
    run_update(window, Some((expected_version, on_progress))).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staging_survives_an_unconfirmed_helper_exit() {
        let root = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir_in(root.path()).unwrap();
        let archive = staging.path().join("release.tar.zst");
        fs::write(&archive, b"partial download").unwrap();
        let error = StagingError::ShutdownUnconfirmed("curl did not stop".into());

        assert!(retain_if_shutdown_unconfirmed::<()>(Err(error), staging).is_err());
        assert!(archive.is_file());
    }

    #[test]
    fn release_must_be_stable_and_newer() {
        let installed = Version::new(0, 0, 1);
        let mut release = Release {
            tag_name: "v0.0.2".into(),
            draft: false,
            prerelease: false,
            assets: Vec::new(),
        };
        assert_eq!(
            available_version(&release, &installed).unwrap(),
            Some(Version::new(0, 0, 2))
        );
        release.prerelease = true;
        assert_eq!(available_version(&release, &installed).unwrap(), None);
        release.prerelease = false;
        for tag in ["v0.0.0", "v0.0.1", "v0.0.1+rebuilt", "v0.0.3-beta.1"] {
            release.tag_name = tag.into();
            assert_eq!(available_version(&release, &installed).unwrap(), None);
        }
    }

    #[test]
    fn release_must_have_the_completed_asset_for_the_current_target() {
        let release: Release = serde_json::from_value(serde_json::json!({
            "tag_name": "v0.2.0",
            "draft": false,
            "prerelease": false,
            "assets": [
                { "id": 1, "name": "HoH-linux-x86_64.tar.zst", "state": "uploaded", "size": 12 },
                { "id": 2, "name": "HoH-linux-aarch64.tar.zst", "state": "starter", "size": 0 },
                { "id": 3, "name": "HoH-macos-arm64.zip", "state": "uploaded", "size": 12 },
            ],
        }))
        .unwrap();
        assert_eq!(
            release
                .asset(package_for("linux", "x86_64").unwrap())
                .unwrap()
                .id,
            1
        );
        assert!(
            release
                .asset(package_for("linux", "aarch64").unwrap())
                .is_none()
        );
        assert_eq!(
            release
                .asset(package_for("macos", "aarch64").unwrap())
                .unwrap()
                .id,
            3
        );
        assert!(package_for("macos", "x86_64").is_none());
    }

    #[test]
    fn checksum_detects_a_same_size_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("package");
        let bytes = b"original";
        let asset = Asset {
            id: 1,
            name: "package".into(),
            state: "uploaded".into(),
            size: bytes.len() as u64,
            digest: Some(format!(
                "sha256:{}",
                hoh_protocol::approval::hex(&Sha256::digest(bytes))
            )),
        };
        fs::write(&file, bytes).unwrap();
        assert!(asset.verify(&file, || false).is_ok());
        fs::write(&file, b"modified").unwrap();
        assert!(asset.verify(&file, || false).is_err());
    }

    fn write_bundle_layout(bundle: &Path, package: Package) {
        let marker = bundle.join(package.version_file);
        fs::create_dir_all(marker.parent().unwrap()).unwrap();
        fs::write(&marker, b"0.0.1\n").unwrap();
        let bin = bundle.join(package.binary_dir);
        fs::create_dir_all(&bin).unwrap();
        for program in ["hoh-desktop", "hoh"] {
            let path = bin.join(program);
            fs::write(&path, b"executable").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn staged_linux_bundle_rejects_a_symlinked_package_directory() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let unpacked = directory.path().join("unpacked");
        let outside = directory.path().join("outside");
        let package = package_for("linux", "x86_64").unwrap();
        let bundle = outside.join(package.install_dir);
        write_bundle_layout(&bundle, package);
        assert!(validate_bundle_layout(&bundle, package, &Version::new(0, 0, 1)).is_ok());
        fs::create_dir(&unpacked).unwrap();
        symlink(&outside, unpacked.join(package.name)).unwrap();

        assert_eq!(
            package.staged_in(&unpacked).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn staged_macos_bundle_rejects_symlinked_contents_and_subdirectories() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let unpacked = directory.path().join("unpacked");
        let package = package_for("macos", "aarch64").unwrap();
        let bundle = unpacked.join(package.install_dir);
        let version = Version::new(0, 0, 1);
        write_bundle_layout(&bundle, package);
        assert_eq!(package.staged_in(&unpacked).unwrap(), bundle);
        assert!(validate_bundle_layout(&bundle, package, &version).is_ok());

        let outside = directory.path().join("outside");
        for name in ["Contents", "Contents/MacOS", "Contents/Resources"] {
            let path = bundle.join(name);
            fs::rename(&path, &outside).unwrap();
            symlink(&outside, &path).unwrap();
            assert_eq!(
                validate_bundle_layout(&bundle, package, &version)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidData
            );
            fs::remove_file(&path).unwrap();
            fs::rename(&outside, &path).unwrap();
        }
    }

    #[test]
    fn swap_replaces_app_and_engine_together() {
        let directory = tempfile::tempdir().unwrap();
        let installed = directory.path().join("installed");
        let staged = directory.path().join("staged");
        fs::create_dir(&installed).unwrap();
        fs::create_dir(&staged).unwrap();
        for program in ["hoh-desktop", "hoh"] {
            fs::write(installed.join(program), b"old").unwrap();
            fs::write(staged.join(program), b"new").unwrap();
        }
        swap_directories(&installed, &staged).unwrap();
        for program in ["hoh-desktop", "hoh"] {
            assert_eq!(fs::read(installed.join(program)).unwrap(), b"new");
            assert_eq!(fs::read(staged.join(program)).unwrap(), b"old");
        }
    }
}
