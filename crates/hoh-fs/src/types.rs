use hoh_error::HarnessError;
use serde::{Deserialize, Serialize};
hoh_brand::branded_string!(pub FsTargetKey);
hoh_brand::branded_string!(pub FsVersion);
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum FsObservation {
    Present { version: FsVersion },
    Absent,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsTarget {
    pub target_key: FsTargetKey,
    pub display_path: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FsType {
    File,
    Directory,
    Other,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FsPathType {
    File,
    Directory,
    Symlink,
    Other,
}
impl From<FsType> for FsPathType {
    fn from(v: FsType) -> Self {
        match v {
            FsType::File => Self::File,
            FsType::Directory => Self::Directory,
            FsType::Other => Self::Other,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsInfo {
    pub version: FsVersion,
    #[serde(rename = "type")]
    pub r#type: FsType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsPathInfo {
    pub version: FsVersion,
    #[serde(rename = "type")]
    pub r#type: FsPathType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsDirEntry {
    pub name: String,
    #[serde(rename = "type")]
    pub r#type: FsType,
    pub target: FsTarget,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<FsVersion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum FsWriteIntent {
    #[serde(rename = "createIfAbsent")]
    CreateIfAbsent,
    #[serde(rename = "replaceIfVersion")]
    ReplaceIfVersion { version: FsVersion },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FsWriteOperation {
    Create,
    Update,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsWriteOutcome {
    pub operation: FsWriteOperation,
    pub version: FsVersion,
    pub before: Option<String>,
    pub after: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsEditRequest {
    pub old_string: String,
    pub new_string: String,
    pub replace_all: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsEditOutcome {
    pub version: FsVersion,
    pub before: String,
    pub after: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsDeleteOutcome {
    pub before: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsMoveSourceFailure {
    pub code: FsErrorCode,
    pub message: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FsMoveOutcome {
    Deleted {
        source_before: Option<String>,
        destination: FsWriteOutcome,
    },
    Retained {
        source_before: Option<String>,
        destination: FsWriteOutcome,
        source_failure: FsMoveSourceFailure,
    },
}
impl Serialize for FsMoveOutcome {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = s.serialize_struct(
            "FsMoveOutcome",
            if matches!(self, Self::Deleted { .. }) {
                3
            } else {
                4
            },
        )?;
        match self {
            Self::Deleted {
                source_before,
                destination,
            } => {
                state.serialize_field("sourceBefore", source_before)?;
                state.serialize_field("destination", destination)?;
                state.serialize_field("sourceDeleted", &true)?;
            }
            Self::Retained {
                source_before,
                destination,
                source_failure,
            } => {
                state.serialize_field("sourceBefore", source_before)?;
                state.serialize_field("destination", destination)?;
                state.serialize_field("sourceDeleted", &false)?;
                state.serialize_field("sourceFailure", source_failure)?;
            }
        }
        state.end()
    }
}
impl<'de> Deserialize<'de> for FsMoveOutcome {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Raw {
            source_before: Option<String>,
            destination: FsWriteOutcome,
            source_deleted: bool,
            source_failure: Option<FsMoveSourceFailure>,
        }
        let r = Raw::deserialize(d)?;
        if r.source_deleted {
            Ok(Self::Deleted {
                source_before: r.source_before,
                destination: r.destination,
            })
        } else {
            Ok(Self::Retained {
                source_before: r.source_before,
                destination: r.destination,
                source_failure: r
                    .source_failure
                    .ok_or_else(|| serde::de::Error::missing_field("sourceFailure"))?,
            })
        }
    }
}
macro_rules! codes {
    ($($variant:ident => $code:literal),+ $(,)?) => {
        pub const FS_ERROR_CODES: &[&str] = &[$($code),+];
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
        pub enum FsErrorCode {
            $(#[serde(rename = $code)] $variant),+
        }
        impl FsErrorCode {
            pub const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $code),+ }
            }
            pub fn from_code(code: &str) -> Option<Self> {
                match code { $($code => Some(Self::$variant)),+, _ => None }
            }
        }
    };
}
codes! {
    NotFound => "FS_NOT_FOUND",
    NotDirectory => "FS_NOT_DIRECTORY",
    NotText => "FS_NOT_TEXT",
    NotRegularFile => "FS_NOT_REGULAR_FILE",
    TooLarge => "FS_TOO_LARGE",
    PermissionDenied => "FS_PERMISSION_DENIED",
    SandboxDenied => "FS_SANDBOX_DENIED",
    IoError => "FS_IO_ERROR",
    StaleVersion => "FS_STALE_VERSION",
    NotObserved => "FS_NOT_OBSERVED",
    AmbiguousEdit => "FS_AMBIGUOUS_EDIT",
    EditNotFound => "FS_EDIT_NOT_FOUND",
    Aborted => "FS_ABORTED",
    PatchMismatch => "FS_PATCH_MISMATCH",
    SameTarget => "FS_SAME_TARGET",
    AlreadyExists => "FS_ALREADY_EXISTS",
}
pub struct FsError;
impl FsError {
    pub const NAME: &str = "FsError";
    #[allow(clippy::new_ret_no_self)]
    pub fn new(message: impl Into<String>, code: FsErrorCode) -> HarnessError {
        HarnessError::named(Self::NAME, message, code.as_str())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsVersionGuard {
    pub version: FsVersion,
}
#[derive(Clone, Debug, Default)]
pub struct FsResolveOptions {
    pub cwd: Option<String>,
    pub signal: Option<hoh_timeout::AbortSignal>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FsByteRange {
    pub offset: u64,
    pub length: u64,
}
#[derive(Clone, Debug, Default)]
pub struct FsMoveOptions {
    pub hunks: Option<Vec<hoh_fs_patch::PatchHunk>>,
    pub expected_source: Option<FsVersionGuard>,
    pub expected_destination: Option<FsWriteIntent>,
}
