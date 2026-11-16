use serde::{Deserialize, Serialize, Serializer, ser::SerializeStruct};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchHunk {
    pub line_number: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    pub old_lines: Vec<String>,
    pub new_lines: Vec<String>,
    pub is_end_of_file: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PatchOperation {
    Add {
        path: String,
        line_number: u64,
        content: String,
    },
    Delete {
        path: String,
        line_number: u64,
    },
    Update {
        path: String,
        line_number: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        move_to: Option<String>,
        hunks: Vec<PatchHunk>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FailureStage {
    Context,
    OldLines,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HunkApplyFailure {
    pub hunk_line_number: u64,
    pub stage: FailureStage,
    pub search_start: u64,
    pub pattern: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyHunksResult {
    Success { after: String },
    Failure { failure: HunkApplyFailure },
}

impl Serialize for ApplyHunksResult {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("ApplyHunksResult", 2)?;
        match self {
            Self::Success { after } => {
                state.serialize_field("ok", &true)?;
                state.serialize_field("after", after)?;
            }
            Self::Failure { failure } => {
                state.serialize_field("ok", &false)?;
                state.serialize_field("failure", failure)?;
            }
        }
        state.end()
    }
}

impl<'de> Deserialize<'de> for ApplyHunksResult {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Record {
            ok: bool,
            after: Option<String>,
            failure: Option<HunkApplyFailure>,
        }
        let record = Record::deserialize(deserializer)?;
        if record.ok {
            record
                .after
                .map(|after| Self::Success { after })
                .ok_or_else(|| serde::de::Error::missing_field("after"))
        } else {
            record
                .failure
                .map(|failure| Self::Failure { failure })
                .ok_or_else(|| serde::de::Error::missing_field("failure"))
        }
    }
}
