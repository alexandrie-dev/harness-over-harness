//! Node-half coverage for the model guidance paired with Web file references.

use std::path::PathBuf;

use hoh_deliverable_file_references::DeliverableFileReferences;
use hoh_kernel::Kernel;
use hoh_system_prompt::{AssembleContext, SystemPrompt, SystemPromptKey};
use serde_json::json;

const SNAPSHOT: &str = "Prefer showing the primary results within your final response alongside a brief explanation. Use ![Description](<path/to/image.png>) when an image supports an explanation or comparison. Use [Description](<path/to/image.png>) when referring to an image or listing files. Enclose Markdown file destinations in angle brackets, especially paths containing spaces. Do not call present just to list edited source files, or run commands to check whether a diff view will appear. Use present to publish complete deliverables, including images, Office documents, spreadsheets, and slide decks, when separate access helps the user. Hosts that support file cards can show the presented file with preview and open actions. Avoid repeating results already shown inline unless the separate card adds useful access. Outside commands, configuration expressions, and code blocks, link every mention of an existing file, including repeats and tables, to its full path relative to the working directory or absolute; append #L24 or #L24-L30 to the target for known lines. Use the filename or a clear alias as the label, adding only enough parent directories to distinguish files; keep full paths out of labels. Default to the name alone; when precise locations matter, append :24 or :24–30, with no # or L in the line suffix.";

fn sidecar(family: &str, scenario: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(family)
        .join(scenario)
        .join("system-prompt.expected.md");
    std::fs::read_to_string(path).unwrap()
}

#[tokio::test(start_paused = true)]
async fn registers_final_response_file_reference_guidance_only_while_mounted() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    ctx.plugin::<SystemPrompt>(json!({"personaPrefix": ""}))
        .unwrap()
        .settled()
        .await
        .unwrap();
    let prompt = ctx.get::<SystemPromptKey>().unwrap();
    let mounted = ctx.plugin::<DeliverableFileReferences>(json!({})).unwrap();
    mounted.settled().await.unwrap();

    let assembly = prompt.assemble(AssembleContext::default()).await.unwrap();
    let section = assembly
        .sections
        .iter()
        .find(|entry| entry.name == "ui:deliverable-file-references");
    assert_eq!(section.map(|s| s.text.as_str()), Some(SNAPSHOT));
    let text = &section.unwrap().text;

    // Only live request pins track current guidance; seeded sessions retain their recorded prompt.
    for scenario in ["fresh-round-trip", "ptc-round"] {
        assert!(
            sidecar("web", scenario).contains(text.as_str()),
            "{scenario}"
        );
    }
    for scenario in ["desktop-context", "desktop-ptc-context"] {
        assert!(
            sidecar("sdk", scenario).contains(text.as_str()),
            "{scenario}"
        );
    }

    mounted.dispose().await;
    let assembly = prompt.assemble(AssembleContext::default()).await.unwrap();
    assert!(
        !assembly
            .sections
            .iter()
            .any(|entry| entry.name == "ui:deliverable-file-references")
    );
    kernel.shutdown().await;
}
