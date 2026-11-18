//! Shared file-delivery guidance for clients that render desktop-style file references.

use hoh_kernel::{Context, Error, Inject, Service, plugin};
use hoh_system_prompt::{PromptSection, PromptSectionOrderName, SystemPrompt, SystemPromptKey};

/// Stable Loader identity (`name` of the source).
pub const NAME: &str = "deliverable-file-references";

/// The section this plugin registers.
const SECTION_NAME: &str = "ui:deliverable-file-references";

/// Static Web guidance for primary outputs and existing-file references.
const FILE_REFERENCE_PROMPT: &str = concat!(
    "Prefer showing the primary results within your final response alongside a brief explanation. ",
    "Use ![Description](<path/to/image.png>) when an image supports an explanation or comparison. Use [Description](<path/to/image.png>) when referring to an image or listing files. Enclose Markdown file destinations in angle brackets, especially paths containing spaces. ",
    "Do not call present just to list edited source files, or run commands to check whether a diff view will appear. ",
    "Use present to publish complete deliverables, including images, Office documents, spreadsheets, and slide decks, when separate access helps the user. Hosts that support file cards can show the presented file with preview and open actions. ",
    "Avoid repeating results already shown inline unless the separate card adds useful access. ",
    "Outside commands, configuration expressions, and code blocks, link every mention of an existing file, including repeats and tables, to its full path relative to the working directory or absolute; append #L24 or #L24-L30 to the target for known lines. ",
    "Use the filename or a clear alias as the label, adding only enough parent directories to distinguish files; keep full paths out of labels. Default to the name alone; when precise locations matter, append :24 or :24–30, with no # or L in the line suffix."
);

/// Prompt registry shared by the Web and SDK desktop compositions (`inject = ['systemPrompt']`).
#[derive(Inject)]
pub struct Deps {
    system_prompt: Service<SystemPromptKey>,
}

/// Register the desktop file-reference guidance without mounting a GUI.
#[plugin(DeliverableFileReferences, name = "deliverable-file-references")]
pub async fn apply(ctx: Context, deps: Deps) -> Result<(), Error> {
    deps.system_prompt.section(
        &ctx,
        PromptSection {
            name: SECTION_NAME.into(),
            order: SystemPrompt::get_section_order(
                PromptSectionOrderName::DeliverableFileReferences,
            ),
            text: FILE_REFERENCE_PROMPT.into(),
            ..PromptSection::default()
        },
    )?;
    Ok(())
}
