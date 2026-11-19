use super::peer::{ensure, parse_file};
use hoh_conformance::{Result, read};
use serde_json::Value;
use std::path::Path;

pub fn check(plan: &Value, cwd: &Path) -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let patches = plan["patches"].as_array().ok_or("patch list missing")?;
    ensure(
        args.len() == 2 + patches.len() * 2 + usize::from(plan["suite"] == "session"),
        "argv length mismatch",
    )?;
    ensure(
        args[0] == "--profile" && args[1] == plan["manifest"]["profile"],
        "profile argv mismatch",
    )?;
    for (i, patch) in patches.iter().enumerate() {
        ensure(args[2 + i * 2] == "--patch", "patch flag missing")?;
        let file = Path::new(&args[3 + i * 2]);
        let source = Path::new(patch["source"].as_str().ok_or("patch source missing")?);
        ensure(
            file.file_name().is_some_and(|n| {
                n.to_string_lossy()
                    == format!(
                        "{i}-{}",
                        source.file_name().unwrap_or_default().to_string_lossy()
                    )
            }),
            "patch argv order mismatch",
        )?;
        patch_value(file, patch, plan)?;
    }
    if plan["suite"] == "session" {
        ensure(
            args.last().map(String::as_str) == plan["task"].as_str(),
            "headless task mismatch",
        )?;
    }
    for (key, expected) in plan["environment"]
        .as_object()
        .ok_or("environment oracle missing")?
    {
        let actual = std::env::var(key).ok();
        if expected == "{{spill}}" {
            let spill = actual.as_ref().ok_or("spill root missing")?;
            let spill = Path::new(spill);
            ensure(
                spill.is_dir()
                    && spill.parent() == cwd.parent()
                    && spill
                        .file_name()
                        .is_some_and(|s| s.to_string_lossy().starts_with("spill-")),
                "spill root is not isolated",
            )?;
        } else {
            let expected = expected
                .as_str()
                .map(|s| s.replace("{{cwd}}", &cwd.to_string_lossy()));
            ensure(actual == expected, format!("environment mismatch: {key}"))?;
        }
    }
    for (i, original) in plan["inputs"]
        .as_array()
        .ok_or("inputs missing")?
        .iter()
        .enumerate()
    {
        let original = Path::new(original.as_str().ok_or("input path missing")?);
        let file = if plan["suite"] == "sdk" {
            cwd.join(".replay-fixtures")
                .join(original.file_name().unwrap_or_default())
        } else {
            original.to_owned()
        };
        let content = plan["inputContents"][i]
            .as_str()
            .ok_or("input bytes missing")?;
        ensure(
            read(&file)?
                == if plan["suite"] == "sdk" {
                    content.replace("{{cwd}}", &cwd.to_string_lossy())
                } else {
                    content.to_owned()
                },
            "selected replay input bytes mismatch",
        )?;
    }
    if !plan["profilePatch"].is_null() {
        patch_value(
            &cwd.join(".dsh/profiles/headless/cordis.patch.yml"),
            &plan["profilePatch"],
            plan,
        )?;
    }
    if plan["manifest"]["workspace"]["parent"] == "outside-temp" {
        ensure(
            !cwd.starts_with(std::env::temp_dir()) && !cwd.starts_with("/tmp"),
            "outside-temp cwd is temporary",
        )?;
    }
    if [
        "compaction-recovery",
        "manual-compact",
        "minimal-image-compaction",
    ]
    .contains(&plan["name"].as_str().unwrap_or(""))
    {
        ensure(
            hoh_util_values::js_json_stringify(&Value::from(cwd.to_string_lossy().into_owned()))
                .encode_utf16()
                .count()
                == 192,
            "fixed cwd width mismatch",
        )?;
    }
    if plan["name"] == "subagent-dsh-sdk-dynamic-route" {
        for i in 0..2 {
            ensure(
                cwd.join(format!(".snapshot-patches/child-{i}.yml"))
                    .is_file(),
                "child patch missing",
            )?;
        }
    }
    Ok(())
}
fn patch_value(path: &Path, patch: &Value, plan: &Value) -> Result<()> {
    let source = Path::new(patch["source"].as_str().ok_or("patch source missing")?);
    let refs = Path::new(plan["refs"].as_str().ok_or("refs missing")?);
    let mut expected = patch["value"].clone();
    fn entry(v: &mut Value, source: &Path, refs: &Path) -> Result<()> {
        if let Some(name) = v["name"].as_str().map(str::to_owned)
            && (name.starts_with("./") || name.starts_with("../"))
        {
            let resolved = hoh_util_workspace_path::node_path::resolve(
                &source
                    .parent()
                    .ok_or("patch parent missing")?
                    .to_string_lossy(),
                &[&name],
            );
            v["name"] = Value::from(format!(
                "local:{}",
                Path::new(&resolved)
                    .strip_prefix(refs)
                    .map_err(|e| e.to_string())?
                    .display()
            ));
        }
        if v["group"] == true
            && let Some(rows) = v["config"].as_array_mut()
        {
            for row in rows {
                entry(row, source, refs)?;
            }
        }
        Ok(())
    }
    for patch in expected.as_array_mut().ok_or("patch array missing")? {
        if let Some(rows) = patch["insert"].as_array_mut() {
            for row in rows {
                entry(row, source, refs)?;
            }
        }
    }
    ensure(
        hoh_util_values::deep_equal_json(&expected, &parse_file(path)?),
        format!("materialized patch differs from {}", source.display()),
    )
}
