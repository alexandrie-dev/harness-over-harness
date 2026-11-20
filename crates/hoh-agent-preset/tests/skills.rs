use hoh_loader::parse_patches;
use hoh_skill::render_skill_content;
use quick_xml::{NsReader, XmlVersion, events::Event, name::ResolveResult};
use regex::Regex;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

const PRUNER_THRESHOLD_CHARS: usize = 8192;

fn skill() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("skills")
        .join("dsh-doc")
}

fn read(path: impl AsRef<Path>) -> String {
    fs::read_to_string(path).unwrap()
}

fn body() -> String {
    let text = read(skill().join("SKILL.md"));
    let end = text[4..].find("\n---\n").unwrap() + 4;
    text[end + "\n---\n".len()..].to_owned()
}

fn files_below(root: &Path) -> Vec<PathBuf> {
    fn visit(root: &Path, out: &mut Vec<PathBuf>) {
        let mut entries: Vec<_> = fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                visit(&path, out);
            } else {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    visit(root, &mut out);
    out
}

fn template_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<_> = fs::read_dir(skill().join("templates"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    dirs
}

fn manifest(dir: &Path) -> Value {
    serde_json::from_str(&read(dir.join("package.json"))).unwrap()
}

fn backtick_resources(text: &str) -> BTreeSet<String> {
    Regex::new(r"`((?:references|templates)/[^`]+)`")
        .expect("resource regex is valid")
        .captures_iter(text)
        .map(|capture| capture[1].to_owned())
        .collect()
}

fn valid_detail_path(path: &str) -> bool {
    Regex::new(r"^[^/]+/.+\.md$").unwrap().is_match(path)
}

fn valid_detail_markdown(markdown: &str) -> bool {
    Regex::new(r"^!\[[^\]\r\n]+\]\(preview\.svg\)\n$")
        .unwrap()
        .is_match(markdown)
}

#[derive(Debug, PartialEq, Eq)]
struct PackageDocument {
    origin: &'static str,
    path: String,
    markdown: String,
}

fn read_locale_document(template: &Path, locale: &Path) -> PackageDocument {
    let value: Value = serde_json::from_str(&read(locale)).unwrap();
    let path = value["meta"]["detail"].as_str().unwrap().to_owned();
    PackageDocument {
        origin: "locale",
        markdown: read(template.join(&path)),
        path,
    }
}

#[derive(Debug, PartialEq, Eq)]
struct DetailImage {
    path: PathBuf,
    media_type: &'static str,
}

fn detail_image(document: &PackageDocument) -> Option<DetailImage> {
    valid_detail_markdown(&document.markdown).then(|| DetailImage {
        path: Path::new(&document.path)
            .parent()
            .unwrap()
            .join("preview.svg"),
        media_type: "image/svg+xml",
    })
}

fn body_names_template_resource(body: &str, template: &str, file: &str) -> bool {
    let reference = if file.starts_with("locale/") {
        "locale"
    } else {
        file
    };
    body.contains(&format!("`templates/{template}/{reference}`"))
}

fn forbidden_document_content(text: &str) -> Option<&'static str> {
    let lower = text.to_ascii_lowercase();
    [
        "do not read sources",
        "do not read dsh sources",
        "do not read package sources",
        "do not read dsh package sources",
    ]
    .into_iter()
    .find(|forbidden| lower.contains(forbidden))
    .or_else(|| {
        ["`cat ", "`cp ", "`ls "]
            .into_iter()
            .find(|forbidden| text.contains(forbidden))
    })
}

fn inspect_svg(
    source: &str,
    forbidden_elements: &[&str],
    forbid_active_attributes: bool,
) -> Result<BTreeMap<String, String>, String> {
    let forbidden: BTreeSet<_> = forbidden_elements.iter().copied().collect();
    let mut reader = NsReader::from_str(source);
    reader.config_mut().expand_empty_elements = true;
    let mut stack: Vec<Vec<u8>> = Vec::new();
    let mut root_seen = false;
    let mut root_closed = false;
    let mut root_attributes = BTreeMap::new();
    loop {
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|error| error.to_string())?;
        let unknown_element_namespace = matches!(namespace, ResolveResult::Unknown(_));
        match event {
            Event::Start(tag) => {
                if unknown_element_namespace {
                    return Err("SVG contains an unbound element namespace prefix".into());
                }
                if stack.is_empty() {
                    if root_seen || root_closed {
                        return Err("SVG must contain exactly one root element".into());
                    }
                    root_seen = true;
                    if tag.local_name().as_ref() != b"svg" {
                        return Err("SVG root element must be svg".into());
                    }
                }
                let local_name =
                    String::from_utf8_lossy(tag.local_name().as_ref()).to_ascii_lowercase();
                if forbidden.contains(local_name.as_str()) {
                    return Err(format!("SVG contains forbidden element {local_name}"));
                }
                for attribute in tag.attributes() {
                    let attribute = attribute.map_err(|error| error.to_string())?;
                    let name = String::from_utf8_lossy(attribute.key.as_ref()).to_ascii_lowercase();
                    let unknown_attribute_namespace = matches!(
                        reader.resolver().resolve_attribute(attribute.key).0,
                        ResolveResult::Unknown(_)
                    );
                    if unknown_attribute_namespace && name != "xmlns" && !name.starts_with("xmlns:")
                    {
                        return Err("SVG contains an unbound attribute namespace prefix".into());
                    }
                    if forbid_active_attributes
                        && (name.starts_with("on") || name.ends_with("href"))
                    {
                        return Err(format!("SVG contains forbidden attribute {name}"));
                    }
                    if stack.is_empty() {
                        let value = attribute
                            .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                            .map_err(|error| error.to_string())?
                            .into_owned();
                        root_attributes.insert(name, value);
                    }
                }
                stack.push(tag.name().as_ref().to_vec());
            }
            Event::End(tag) => {
                if stack.pop().as_deref() != Some(tag.name().as_ref()) {
                    return Err("SVG contains mismatched element tags".into());
                }
                if stack.is_empty() {
                    root_closed = true;
                }
            }
            Event::Text(text) => {
                let text = text.xml10_content().map_err(|error| error.to_string())?;
                if stack.is_empty() && !text.trim().is_empty() {
                    return Err("SVG contains text outside its root element".into());
                }
            }
            Event::CData(text) => {
                text.decode().map_err(|error| error.to_string())?;
                if stack.is_empty() {
                    return Err("SVG contains CDATA outside its root element".into());
                }
            }
            Event::GeneralRef(reference) => {
                let name = reference.decode().map_err(|error| error.to_string())?;
                let is_predefined = matches!(name.as_ref(), "amp" | "lt" | "gt" | "apos" | "quot");
                let is_valid_character = reference
                    .resolve_char_ref()
                    .map_err(|error| error.to_string())?
                    .is_some();
                if !is_predefined && !is_valid_character {
                    return Err(format!("SVG contains an undeclared entity &{name};"));
                }
            }
            Event::Eof => break,
            Event::DocType(_) => return Err("SVG document types are not allowed".into()),
            Event::Decl(_) | Event::PI(_) | Event::Comment(_) => {}
            Event::Empty(_) => return Err("XML reader did not expand an empty element".into()),
        }
    }
    if !root_seen || !root_closed || !stack.is_empty() {
        return Err("SVG is not a complete XML document".into());
    }
    Ok(root_attributes)
}

#[test]
fn is_the_only_skill_the_package_skill_root_exposes() {
    let skill = skill();
    let root = skill.parent().unwrap();
    let mut names: Vec<_> = fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["dsh-doc"]);
    let frontmatter = read(skill.join("SKILL.md"));
    assert!(frontmatter.starts_with("---\nname: dsh-doc\n"));
    assert!(!body().trim().is_empty());
}

#[test]
fn leaves_repository_skills_without_a_duplicate_and_wraps_tool_design() {
    let text = body();
    assert_eq!(text.matches("`references/tool-design.md`").count(), 1);
    assert!(skill().join("references/tool-design.md").is_file());
}

#[test]
fn renders_below_the_pruner_threshold_and_lists_exactly_the_shipped_files() {
    let rendered = render_skill_content(
        &json!({
            "name":"dsh-doc",
            "provider":"filesystem",
            "resourceBase":{"kind":"directory","path":skill()},
            "content":body()
        })
        .into(),
    )
    .unwrap();
    assert!(rendered.chars().count() < PRUNER_THRESHOLD_CHARS);

    let referenced = backtick_resources(&body());
    let shipped: BTreeSet<_> = files_below(&skill())
        .into_iter()
        .filter_map(|path| {
            let relative = path.strip_prefix(skill()).unwrap();
            (relative != Path::new("SKILL.md")).then(|| {
                let mut value = relative.to_string_lossy().replace('\\', "/");
                if let Some(index) = value.find("/locale/") {
                    value.truncate(index + "/locale".len());
                }
                value
            })
        })
        .collect();
    assert_eq!(referenced, shipped);
}

#[test]
fn ships_templates_whose_manifest_patch_and_javascript_are_well_formed_assets() {
    for dir in template_dirs() {
        let package = manifest(&dir);
        let patch = package["dsh"]["bundle"]["patch"]
            .as_str()
            .unwrap()
            .trim_start_matches("./");
        let parsed = parse_patches(&read(dir.join(patch)), &dir.join(patch)).unwrap();
        assert!(!parsed.is_empty());
        for file in fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
        {
            if file.extension().is_some_and(|extension| extension == "js") {
                let source = read(file);
                assert!(!source.trim().is_empty());
                assert!(!source.contains('\0'));
            }
        }
    }
}

#[test]
fn ships_discoverable_exported_packaged_display_resources() {
    let skill_body = body();
    for dir in template_dirs() {
        let template = dir.file_name().unwrap().to_string_lossy();
        let package = manifest(&dir);
        assert!(package.get("icon").is_none());
        let files = package["files"].as_array().unwrap();
        assert!(files.contains(&json!("docs")));
        assert_eq!(package["exports"]["./locale/*.json"], "./locale/*.json");
        assert_eq!(package["exports"]["./icon"], "./icon.svg");

        let locale_files: Vec<_> = fs::read_dir(dir.join("locale"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert!(locale_files.iter().any(|path| path.ends_with("en.json")));
        let english: Value = serde_json::from_str(&read(dir.join("locale/en.json"))).unwrap();
        let english_keys: BTreeSet<_> = english["meta"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        let mut resources = BTreeSet::new();
        for export in package["exports"].as_object().unwrap().values() {
            let export = export.as_str().unwrap();
            if !export.contains('*') {
                resources.insert(export.to_owned());
            }
        }
        for locale in locale_files {
            let value: Value = serde_json::from_str(&read(&locale)).unwrap();
            let keys: BTreeSet<_> = value["meta"].as_object().unwrap().keys().cloned().collect();
            assert_eq!(keys, english_keys, "{}", locale.display());
            for key in ["title", "description", "detail"] {
                assert!(!value["meta"][key].as_str().unwrap().trim().is_empty());
            }
            let detail = value["meta"]["detail"].as_str().unwrap();
            assert!(valid_detail_path(detail), "{}: {detail}", locale.display());
            resources.insert(detail.to_owned());
            resources.insert(format!(
                "locale/{}",
                locale.file_name().unwrap().to_string_lossy()
            ));
        }
        resources.insert(
            package["dsh"]["bundle"]["patch"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
        for resource in resources {
            let file = resource.trim_start_matches("./");
            assert!(
                body_names_template_resource(&skill_body, &template, file),
                "SKILL.md does not name templates/{template}/{file}"
            );
            assert!(dir.join(file).is_file(), "missing {file}");
        }
    }
}

#[test]
fn publishes_template_metadata_through_the_same_static_contract_as_ci() {
    for dir in template_dirs() {
        let package = manifest(&dir);
        let exports = package["exports"].as_object().unwrap();
        let files: Vec<_> = package["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect();
        for export in [".", "./client"] {
            if let Some(path) = exports.get(export).and_then(Value::as_str) {
                let path = path.trim_start_matches("./");
                assert!(files.contains(&path));
                assert!(dir.join(path).is_file());
            }
        }
        let patch = package["dsh"]["bundle"]["patch"]
            .as_str()
            .unwrap()
            .trim_start_matches("./");
        assert!(files.contains(&patch));
        assert!(dir.join("icon.svg").is_file());
        assert!(dir.join("docs/DETAIL.md").is_file());
    }
}

#[test]
fn uses_a_36_unit_canvas_for_each_template_icon() {
    for dir in template_dirs() {
        let icon = read(dir.join("icon.svg"));
        let attributes = inspect_svg(&icon, &[], false)
            .unwrap_or_else(|error| panic!("{}: {error}", dir.display()));
        assert_eq!(attributes.get("width").map(String::as_str), Some("36"));
        assert_eq!(attributes.get("height").map(String::as_str), Some("36"));
        assert_eq!(
            attributes.get("viewbox").map(String::as_str),
            Some("0 0 36 36")
        );
    }
}

#[test]
fn shows_one_packaged_svg_in_each_template_detail_document() {
    for dir in template_dirs() {
        let english: Value = serde_json::from_str(&read(dir.join("locale/en.json"))).unwrap();
        let expected_detail = english["meta"]["detail"].as_str().unwrap();
        for locale in fs::read_dir(dir.join("locale")).unwrap() {
            let locale = locale.unwrap().path();
            let document = read_locale_document(&dir, &locale);
            assert_eq!(document.origin, "locale", "{}", locale.display());
            assert_eq!(document.path, expected_detail, "{}", locale.display());
            assert!(
                valid_detail_markdown(&document.markdown),
                "{} has an invalid detail document",
                locale.display()
            );
            assert_eq!(
                detail_image(&document),
                Some(DetailImage {
                    path: PathBuf::from("docs/preview.svg"),
                    media_type: "image/svg+xml",
                }),
                "{}",
                locale.display()
            );
        }
        let preview = read(dir.join("docs/preview.svg"));
        let attributes = inspect_svg(
            &preview,
            &[
                "script",
                "style",
                "text",
                "foreignobject",
                "image",
                "animate",
                "animatetransform",
            ],
            true,
        )
        .unwrap_or_else(|error| panic!("{}: {error}", dir.display()));
        assert_eq!(attributes.get("width").map(String::as_str), Some("640"));
        assert_eq!(attributes.get("height").map(String::as_str), Some("300"));
        assert_eq!(
            attributes.get("viewbox").map(String::as_str),
            Some("0 0 640 300")
        );
    }
}

#[test]
fn names_only_shipped_files_and_no_retired_skill() {
    for file in files_below(&skill()) {
        if file.extension().is_none_or(|extension| extension != "md") {
            continue;
        }
        let text = read(&file);
        for relative in backtick_resources(&text) {
            assert!(
                skill().join(&relative).exists(),
                "{}: {relative}",
                file.display()
            );
        }
        for retired in [
            "cordis-plugin-development",
            "editing-cordis-compositions",
            "cordis-composition-reference",
            "agent-experience",
        ] {
            assert!(!text.contains(retired), "{}: {retired}", file.display());
        }
    }
}

#[test]
fn locates_checkout_documentation_from_verified_markers() {
    let guide = read(skill().join("references/locate-docs.md"));
    for marker in [
        "<root>/pnpm-workspace.yaml",
        "@deepseek-ai/dsh-root",
        "<root>/apps/cli/package.json",
        "<root>/docs/user/index.md",
        "<root>/packages/README.md",
    ] {
        assert!(guide.contains(marker));
    }
    assert!(guide.contains("five parents below the root"));
}

#[test]
fn documents_posix_commands_that_use_the_verified_installation() {
    let locate = read(skill().join("references/locate-docs.md"));
    assert!(locate.contains("```sh\n"));
    assert!(locate.contains("$DSH_NODE_EXECUTABLE"));
    assert!(locate.contains("process.env.DSH_INSTALL_DIR"));
    let upgrading = read(skill().join("references/upgrading.md"));
    assert!(upgrading.contains("line.startsWith('## ')"));
    assert!(upgrading.contains("$DSH_INSTALL_DIR/docs/upgrade-guide/README.md"));
}

#[test]
fn documents_a_powershell_process_form_that_waits_for_the_runtime() {
    let guide = read(skill().join("references/locate-docs.md"));
    assert!(guide.contains("```powershell\n"));
    assert!(guide.contains("[System.Diagnostics.Process]::Start($start)"));
    assert!(guide.contains("$process.WaitForExit()"));
    assert!(guide.contains("exit $process.ExitCode"));
    assert!(guide.contains("RedirectStandardError = $true"));
}

#[test]
fn fidelity_validators_reject_inputs_the_original_checks_reject() {
    assert_eq!(
        forbidden_document_content("Do not read sources"),
        Some("do not read sources")
    );

    assert!(valid_detail_path("docs/DETAIL.md"));
    assert!(!valid_detail_path("DETAIL.md"));
    assert!(!valid_detail_path("/docs/DETAIL.md"));
    assert!(valid_detail_markdown("![Preview](preview.svg)\n"));
    assert!(!valid_detail_markdown("![Preview\ntext](preview.svg)\n"));
    assert!(!valid_detail_markdown(
        "![Preview](preview.svg)\nUnexpected text\n"
    ));

    assert!(body_names_template_resource(
        "Use `templates/example/icon.svg`.",
        "example",
        "icon.svg"
    ));
    assert!(!body_names_template_resource(
        "No packaged resource is named here.",
        "example",
        "icon.svg"
    ));

    for unsafe_svg in [
        r#"<svg xmlns="http://www.w3.org/2000/svg"><rect onclick="run()"/></svg>"#,
        r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><use xlink:href="#shape"/></svg>"##,
        "<svg>&undeclared;</svg>",
        "<!DOCTYPE svg><svg/>",
        "<svg><unknown:rect/></svg>",
    ] {
        assert!(inspect_svg(unsafe_svg, &[], true).is_err());
    }
    assert!(inspect_svg("<svg><g></svg>", &[], true).is_err());

    let document = PackageDocument {
        origin: "locale",
        path: "docs/DETAIL.md".into(),
        markdown: "![Preview](preview.svg)\n".into(),
    };
    assert_eq!(
        detail_image(&document),
        Some(DetailImage {
            path: PathBuf::from("docs/preview.svg"),
            media_type: "image/svg+xml",
        })
    );
}

#[test]
fn never_forbids_reading_sources_or_routes_skill_files_through_the_shell() {
    for file in files_below(&skill()) {
        if file.extension().is_none_or(|extension| extension != "md") {
            continue;
        }
        let text = read(&file);
        assert_eq!(
            forbidden_document_content(&text),
            None,
            "{} contains a forbidden instruction",
            file.display()
        );
    }
}
