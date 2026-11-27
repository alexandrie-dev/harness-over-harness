use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    ffi::{CString, OsStr},
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Write},
    os::unix::{ffi::OsStrExt, fs::PermissionsExt},
    path::Path,
    process::{Command, Stdio},
};
use tauri::ipc::Channel;

const DEFAULT_RELEASE_REPOSITORY: &str = "alexandrie-dev/harness-over-harness";
const MAX_ASSET_SIZE: u64 = 512 * 1024 * 1024;

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

    fn installed_at(self, executable: &Path) -> Option<&Path> {
        let binary = Path::new(self.binary_dir).join("hoh-desktop");
        let root = executable.ancestors().nth(binary.components().count())?;
        let valid_root = if self.extension == "zip" {
            root.extension() == Some(OsStr::new("app"))
        } else {
            root.file_name() == Some(OsStr::new(self.install_dir))
        };
        (valid_root && executable.ends_with(&binary) && root.join(self.version_file).is_file())
            .then_some(root)
    }
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
    NoCompatiblePackage(Version),
    Available(Version),
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
    Archive,
}

fn fetch(
    url: &str,
    destination: &Path,
    token: Option<&str>,
    response: Response,
) -> Result<u16, String> {
    let (accept, timeout, limit) = match response {
        Response::Metadata => ("application/vnd.github+json", "20", 2 * 1024 * 1024),
        Response::Archive => ("application/octet-stream", "300", MAX_ASSET_SIZE),
    };
    let mut command = Command::new("/usr/bin/curl");
    command.arg("--disable");
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
            timeout,
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
    let mut child = command
        .spawn()
        .map_err(|error| format!("Could not start curl for the update ({error})."))?;
    let header = token.map(|token| {
        writeln!(
            child.stdin.take().expect("curl stdin is piped"),
            "Authorization: Bearer {token}"
        )
    });
    let output = child
        .wait_with_output()
        .map_err(|error| format!("Could not read the GitHub response ({error})."))?;
    if !output.status.success() || header.is_some_and(|result| result.is_err()) {
        return Err("Could not download from GitHub. Check your connection and try again.".into());
    }
    if fs::metadata(destination).map_or(true, |file| file.len() > limit) {
        return Err("The GitHub response was missing or exceeded the download limit.".into());
    }
    std::str::from_utf8(&output.stdout)
        .ok()
        .and_then(|status| status.parse().ok())
        .ok_or_else(|| "Could not read the GitHub response status.".into())
}

fn github_token() -> Option<String> {
    // Finder launches with a restricted PATH, so also look in the Homebrew bin directories.
    for binary in ["gh", "/opt/homebrew/bin/gh", "/usr/local/bin/gh"] {
        match Command::new(binary)
            .args(["auth", "token", "--hostname", "github.com"])
            .env("GH_PROMPT_DISABLED", "1")
            .current_dir("/")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
        {
            Ok(output) => {
                let token = String::from_utf8(output.stdout).ok()?.trim().to_owned();
                return (output.status.success()
                    && !token.is_empty()
                    && token.bytes().all(|byte| byte.is_ascii_graphic()))
                .then_some(token);
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
) -> Result<Option<(Release, Option<String>)>, String> {
    let url = format!("{api}/releases/latest");
    let mut token = None;
    let mut status = fetch(&url, destination, None, Response::Metadata)?;
    if status == 404 && fetch(api, destination, None, Response::Metadata)? == 200 {
        return Ok(None);
    }
    if matches!(status, 401 | 403 | 404) {
        token = github_token();
        if let Some(token) = token.as_deref() {
            status = fetch(&url, destination, Some(token), Response::Metadata)?;
        }
    }
    if status == 404
        && token.is_some()
        && fetch(api, destination, token.as_deref(), Response::Metadata)? == 200
    {
        return Ok(None);
    }
    if matches!(status, 401 | 404) {
        return Err("GitHub Releases is unavailable. For a private repository, run gh auth login and use an account with access.".into());
    }
    if status != 200 {
        return Err(format!(
            "GitHub could not check for updates (HTTP {status})."
        ));
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

    fn verify(&self, path: &Path) -> Result<(), String> {
        let read_error = |error| format!("Could not read the downloaded package ({error}).");
        let file = File::open(path).map_err(read_error)?;
        if file.metadata().map_err(read_error)?.len() != self.size {
            return Err("The downloaded package size does not match the release.".into());
        }
        let mut reader = BufReader::new(file);
        let mut hash = Sha256::new();
        loop {
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

fn extract(archive: &Path, staging: &Path) -> Result<(), String> {
    let status = Command::new("/usr/bin/tar")
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(staging)
        .args(["--no-same-owner", "--no-same-permissions"])
        .env_remove("TAR_OPTIONS")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("Could not start the package extractor ({error})."))?;
    if !status.success() {
        return Err(
            "Could not unpack the HoH release. On Linux, make sure zstd is installed.".into(),
        );
    }
    Ok(())
}

fn validate_bundle(bundle: &Path, package: Package, version: &Version) -> Result<(), String> {
    let invalid = || "The package does not contain a complete HoH installation for this release.";
    let version_marker =
        fs::read_to_string(bundle.join(package.version_file)).map_err(|_| invalid())?;
    if !bundle.symlink_metadata().is_ok_and(|file| file.is_dir())
        || version_marker.trim() != version.to_string()
    {
        return Err(invalid().into());
    }
    for program in ["hoh-desktop", "hoh"] {
        let binary = bundle.join(package.binary_dir).join(program);
        let metadata = binary.symlink_metadata().map_err(|_| invalid())?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o100 == 0 {
            return Err(invalid().into());
        }
    }
    #[cfg(target_os = "macos")]
    if !Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(bundle)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
    {
        return Err("macOS could not verify the signature of the new HoH.app.".into());
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
    progress: Option<impl Fn(UpdateProgress)>,
) -> Result<UpdateOutcome, String> {
    let Some(package) = package_for(std::env::consts::OS, std::env::consts::ARCH) else {
        return Ok(UpdateOutcome::UnsupportedInstall);
    };
    let Some(installed) = package.installed_at(executable) else {
        return Ok(UpdateOutcome::UnsupportedInstall);
    };
    let metadata = tempfile::NamedTempFile::new()
        .map_err(|error| format!("Could not prepare the update check ({error})."))?;
    let repo = release_repository()?;
    let api = format!("https://api.github.com/repos/{repo}");
    let Some((release, token)) = latest_release(&api, metadata.path())? else {
        return Ok(UpdateOutcome::NoRelease);
    };
    let Some(version) = available_version(&release, installed_version)? else {
        return Ok(UpdateOutcome::UpToDate);
    };
    let Some(asset) = release.asset(package) else {
        return Ok(UpdateOutcome::NoCompatiblePackage(version));
    };
    if asset.size == 0 || asset.size > MAX_ASSET_SIZE {
        return Err("The update package has an unexpected size.".into());
    }
    asset.checksum()?;
    let Some(progress) = progress else {
        return Ok(UpdateOutcome::Available(version));
    };

    let parent = installed
        .parent()
        .ok_or("Could not locate the HoH installation.")?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(parent.join(".hoh-update.lock"))
        .map_err(|error| format!("Could not write to the HoH installation folder ({error})."))?;
    lock.try_lock()
        .map_err(|error| format!("Could not acquire the HoH update lock ({error})."))?;

    let marker = fs::read_to_string(installed.join(package.version_file))
        .map_err(|error| format!("Could not read the installed version ({error})."))?;
    if marker.trim() != installed_version.to_string() {
        return Err(
            "The installation has changed on disk. Restart HoH before checking again.".into(),
        );
    }

    let staging = tempfile::Builder::new()
        .prefix(".hoh-update-")
        .tempdir_in(parent)
        .map_err(|error| format!("Could not prepare the HoH installation folder ({error})."))?;
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
    let status = fetch(&url, &archive, token.as_deref(), Response::Archive)?;
    if status != 200 {
        return Err(format!(
            "GitHub could not serve the update package (HTTP {status})."
        ));
    }
    asset.verify(&archive)?;

    progress(UpdateProgress::Installing(version.clone()));
    let unpacked = staging.path().join("unpacked");
    fs::create_dir(&unpacked)
        .map_err(|error| format!("Could not prepare the package extractor ({error})."))?;
    extract(&archive, &unpacked)?;
    let staged = if package.extension == "zip" {
        unpacked.join(package.install_dir)
    } else {
        unpacked.join(package.name).join(package.install_dir)
    };
    validate_bundle(&staged, package, &version)?;
    swap_directories(installed, &staged)
        .map_err(|error| format!("Could not replace the HoH installation ({error})."))?;
    Ok(UpdateOutcome::Restarting(version))
}

async fn run_update(
    window: tauri::WebviewWindow,
    on_progress: Option<Channel<UpdateProgress>>,
) -> Result<UpdateOutcome, String> {
    if window.label() != "main" {
        return Err("Updates are available in the main window.".into());
    }
    use tauri::Manager;
    let app = window.app_handle();
    let installed_version = app.package_info().version.clone();
    let executable = tauri::process::current_binary(&app.env())
        .map_err(|error| format!("Could not locate HoH ({error})."))?;
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let notify = on_progress.map(|channel| {
            move |progress| {
                let _ = channel.send(progress);
            }
        });
        update(&executable, &installed_version, notify)
    })
    .await
    .map_err(|_| "HoH could not complete the update request.")??;
    if matches!(outcome, UpdateOutcome::Restarting(_)) {
        app.request_restart();
    }
    Ok(outcome)
}

#[tauri::command]
pub async fn check_for_update(window: tauri::WebviewWindow) -> Result<UpdateOutcome, String> {
    run_update(window, None).await
}

#[tauri::command]
pub async fn auto_update(
    window: tauri::WebviewWindow,
    on_progress: Channel<UpdateProgress>,
) -> Result<UpdateOutcome, String> {
    run_update(window, Some(on_progress)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_must_be_stable_and_newer() {
        let installed = Version::new(0, 2, 0);
        let mut release = Release {
            tag_name: "v0.10.0".into(),
            draft: false,
            prerelease: false,
            assets: Vec::new(),
        };
        assert_eq!(
            available_version(&release, &installed).unwrap(),
            Some(Version::new(0, 10, 0))
        );
        release.prerelease = true;
        assert_eq!(available_version(&release, &installed).unwrap(), None);
        release.prerelease = false;
        for tag in ["v0.1.9", "v0.2.0", "v0.2.0+rebuilt", "v0.3.0-beta.1"] {
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
        assert!(asset.verify(&file).is_ok());
        fs::write(&file, b"modified").unwrap();
        assert!(asset.verify(&file).is_err());
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
