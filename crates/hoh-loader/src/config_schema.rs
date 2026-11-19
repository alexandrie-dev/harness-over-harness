//! Boot-free composition inspection. Plugin schemas come from schemars, without plugin application.
use crate::{Entry, Layer, PluginRegistry, Profile, expression, parse_entries, parse_patches};
use indexmap::{IndexMap, IndexSet};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConfigSchemaDiagnostic {
    pub level: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub message: String,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSchemaEntry {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub status: String,
    pub config_ref: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tree: Option<String>,
}
struct Collector<'a> {
    registry: &'a PluginRegistry,
    entries: Vec<ConfigSchemaEntry>,
    diagnostics: Vec<ConfigSchemaDiagnostic>,
    definitions: serde_json::Map<String, Value>,
    names: IndexMap<String, IndexSet<String>>,
    targets: IndexMap<String, Option<usize>>,
    uncollected_targets: IndexMap<String, String>,
    schemas: IndexMap<usize, String>,
    ancestors: IndexSet<PathBuf>,
    required: IndexSet<String>,
}
struct Frame {
    data: Arc<Value>,
    pointer: String,
    dir: PathBuf,
    prefix: String,
    root_targets: bool,
    position: usize,
    len: usize,
    canonical: Option<PathBuf>,
}
fn reference(name: &str) -> Value {
    json!({"$ref": format!("#/$defs/{name}")})
}
fn metadata() -> Value {
    json!({"type":"object", "properties": {
        "id":{"type":"string", "description":"Entry id. Loader generates an id when an entry omits it; patches use the configured id."},
        "name":{"type":"string", "description":"Plugin module specifier. Inserted relative plugin paths are anchored beside their patch file."},
        "config":{}, "group":{"type":["boolean","null"], "description":"Allows patch indexing and insertion into an entry-list config; does not select the plugin implementation."},
        "disabled":{"anyOf":[{"type":["boolean","null"]}, reference("loaderExpression")], "description":"Boolean or !!js expression. The Loader coerces other truthy values as disabled; this schema rejects them."},
        "inject":{"anyOf":[{"type":"array","items":{"type":"string"}},{"type":"null"}]},
        "intercept":{"type":["object","null"]},
        "isolate":{"type":["object","null"],"additionalProperties":{"anyOf":[{"const":true},{"type":"string"}]}}
    }})
}
fn patch_structure() -> Value {
    json!({"type":"object", "allOf":[reference("entryMetadata")], "properties":{"insert":reference("entryList")}, "description":"An insert appends entries, optionally inside the group identified by id. Other patches replace supplied fields; config is replaced wholesale, not deep-merged. A truthy name asserts the existing plugin name rather than renaming it. Unknown targets and non-insert patches without a nonempty id are warned and skipped."})
}
pub async fn generate_config_schema(
    profile: &Profile,
    base: &[Entry],
    layers: &[Layer],
    registry: &PluginRegistry,
) -> Value {
    let mut diagnostics: Vec<_> = profile.skipped.iter().map(|bundle| ConfigSchemaDiagnostic { level: "error".into(), path: None, message: format!("Selected profile bundle {} could not be loaded; repair or remove its bundle selection.", hoh_util_values::js_json_stringify(&Value::String(bundle.name.clone()))) }).collect();
    let composition = crate::patches::compose_unlabelled(base, layers, &mut |message| {
        diagnostics.push(ConfigSchemaDiagnostic {
            level: "warning".into(),
            path: None,
            message,
        })
    });
    collect_config_schemas(
        &profile.name,
        &profile.dir,
        &crate::yaml::entries_value(&composition.entries),
        registry,
        &diagnostics,
    )
    .await
}
pub async fn collect_config_schemas(
    profile: &str,
    dir: &Path,
    rows: &Value,
    registry: &PluginRegistry,
    diagnostics: &[ConfigSchemaDiagnostic],
) -> Value {
    let mut collector = new_collector(registry, diagnostics);
    collector.walk(rows, dir, "", true).await;
    build_document(profile, collector)
}
fn new_collector<'a>(
    registry: &'a PluginRegistry,
    diagnostics: &[ConfigSchemaDiagnostic],
) -> Collector<'a> {
    let mut collector = Collector {
        registry,
        entries: vec![],
        diagnostics: diagnostics.to_vec(),
        definitions: serde_json::Map::new(),
        names: IndexMap::new(),
        targets: IndexMap::new(),
        uncollected_targets: IndexMap::new(),
        schemas: IndexMap::new(),
        ancestors: IndexSet::new(),
        required: IndexSet::from(["#/$defs/entryList".into(), "#/$defs/includeConfig".into()]),
    };
    collector.definitions.insert("loaderExpression".into(), json!({"type":"object", "properties":{"__jsExpr":{"type":"string"}}, "required":["__jsExpr"], "additionalProperties":false, "description":"Inert representation of a YAML !!js scalar from the Cordis entry-list parser. Its result is evaluated and validated only at runtime. Extra marker-object fields are ignored by interpolation."}));
    collector
        .definitions
        .insert("entryMetadata".into(), metadata());
    collector.definitions.insert(
        "entryList".into(),
        json!({"type":"array", "items": reference("entry")}),
    );
    collector.definitions.insert(
        "patchList".into(),
        json!({"type":"array", "items": reference("patch")}),
    );
    collector.definitions.insert("unknownConfig".into(), json!({"$comment":"No projected Config schema is available; this is unknown configuration, not a prohibition on fields."}));
    collector.definitions.insert("includeConfig".into(), json!({"type":"object", "required":["path"], "properties":{"path":{"type":"string", "description":"YAML/JSON filename resolved relative to the owning Loader tree. This field is literal, not a !!js expression."},"initial":reference("entryList"),"patches":{"type":"array","items":reference("includePatch")},"enableLogs":{"type":"boolean"}}, "description":"Native Include configuration stays literal. initial is used only when the file is absent. Included files have their own module-resolution base and patch target index."}));
    collector
        .definitions
        .insert("includePatch".into(), patch_structure());
    collector.names.insert(
        "cordis:group".into(),
        IndexSet::from(["#/$defs/entryList".into()]),
    );
    collector.names.insert(
        "cordis:include".into(),
        IndexSet::from(["#/$defs/includeConfig".into()]),
    );
    collector
}

/// Build from already projected schemars definitions and a distinct root-tree target index.
/// A target not present in the collected declarations deliberately uses unknownConfig.
pub fn build_config_schema_document(
    profile: &str,
    entries: &[ConfigSchemaEntry],
    targets: &IndexMap<String, ConfigSchemaEntry>,
    diagnostics: &[ConfigSchemaDiagnostic],
    definitions: &serde_json::Map<String, Value>,
) -> Value {
    let registry = PluginRegistry::default();
    let mut collector = new_collector(&registry, diagnostics);
    collector.definitions.extend(definitions.clone());
    collector.entries = entries.to_vec();
    for entry in &mut collector.entries {
        if entry.config_ref.is_empty() {
            entry.config_ref = "#/$defs/unknownConfig".into();
        }
        if let Some(name) = &entry.name {
            collector
                .names
                .entry(name.clone())
                .or_default()
                .insert(entry.config_ref.clone());
        }
    }
    for (id, target) in targets {
        let index = entries.iter().position(|entry| {
            entry.path == target.path && entry.id == target.id && entry.name == target.name
        });
        collector.targets.insert(id.clone(), index);
        if index.is_none()
            && let Some(name) = &target.name
        {
            collector
                .uncollected_targets
                .insert(id.clone(), name.clone());
        }
    }
    build_document(profile, collector)
}

fn build_document(profile: &str, mut collector: Collector<'_>) -> Value {
    let mut rules = vec![];
    for (name, choices) in &collector.names {
        let validated = json!({"properties":{"config":{"anyOf":choices.iter().map(|r| json!({"$ref":r})).collect::<Vec<_>>()}}});
        let carrier = choices
            .iter()
            .all(|r| r == "#/$defs/entryList" || r == "#/$defs/includeConfig");
        let dormant = json!({"properties":{"disabled":{"anyOf":[{"const":true}, reference("loaderExpression")]}, "group":{"not":{"const":true}}},"required":["disabled"]});
        let mut active = validated.clone();
        active["required"] = json!(["config"]);
        let then = if !choices
            .iter()
            .all(|reference| collector.required.contains(reference))
        {
            validated
        } else if carrier {
            json!({"if": dormant, "else":active})
        } else {
            json!({"if":dormant, "then":validated, "else":active})
        };
        rules.push(
            json!({"if":{"properties":{"name":{"const":name}},"required":["name"]},"then":then}),
        );
    }
    rules.insert(0, reference("entryMetadata"));
    collector.definitions.insert(
        "entry".into(),
        json!({"type":"object", "required":["name"], "allOf":rules,
            "description":"Loader entry. Unknown metadata and plugin names are accepted; plugin-specific validation is available only for the names collected in this profile. Distinct resolutions of one name use a union of their schemas."}),
    );
    let mut patch_rules = vec![reference("entryMetadata")];
    for (id, index) in &collector.targets {
        let entry = index.and_then(|i| collector.entries.get(i));
        let Some(name) = entry
            .and_then(|entry| entry.name.as_ref())
            .or_else(|| collector.uncollected_targets.get(id))
        else {
            continue;
        };
        let config_ref = entry
            .map(|entry| entry.config_ref.as_str())
            .unwrap_or("#/$defs/unknownConfig");
        patch_rules.push(json!({"if":{"required":["id"],"properties":{"id":{"const":id}},"allOf":[{"not":{"required":["insert"]}},{"anyOf":[{"not":{"required":["name"]}},{"properties":{"name":{"enum":["",name]}}}]}]},"then":{"properties":{"config":{"$ref":config_ref}}}}));
    }
    let mut patch = patch_structure();
    patch["allOf"] = patch_rules.into();
    collector.definitions.insert("patch".into(), patch);
    let positions: IndexMap<_, _> = collector
        .entries
        .iter()
        .enumerate()
        .map(|(i, e)| (e.path.clone(), i + 1))
        .collect();
    collector.diagnostics.sort_by_key(|d| {
        d.path
            .as_ref()
            .and_then(|p| positions.get(p))
            .copied()
            .unwrap_or(0)
    });
    let complete = !collector.diagnostics.iter().any(|d| d.level == "error")
        && !collector
            .entries
            .iter()
            .any(|entry| matches!(entry.status.as_str(), "error" | "partial" | "unsupported"))
        && !collector.names.values().any(|choices| choices.len() > 1);
    json!({"$schema":"https://json-schema.org/draft/2020-12/schema", "title":format!("Cordis configuration for profile {profile}"),"description":"Describes the parsed entry-list YAML printed by --dump-config. Use $defs.patchList for a profile/home/CLI overlay. Parse !!js with the Cordis entry-list YAML dialect. Expression results, service dependencies, plugin startup checks, and sequence-dependent patch targets still require runtime validation.",
        "$comment":"Bundle, profile, home, and CLI layers apply in that order. A patch config replaces the whole config. JSON Schema defaults are annotations; Schemastery validates a fallback on null/omission unless required rejects it first. Schemastery unions select the first successful branch. Root patch id constraints describe the current index, not ids introduced or changed by earlier patches. Include-local patches have a separate index. Relative plugin insertions require their source patch directory; this document does not infer future overlay locations.",
        "type":"array","items":reference("entry"),"$defs":collector.definitions,"x-cordis":{"profile":profile,"complete":complete,"entries":collector.entries,"diagnostics":collector.diagnostics,"patchSchema":"#/$defs/patchList"}})
}
impl Collector<'_> {
    fn report(&mut self, path: &str, message: impl Into<String>) {
        self.diagnostics.push(ConfigSchemaDiagnostic {
            level: "error".into(),
            path: Some(path.into()),
            message: message.into(),
        });
    }
    async fn walk(&mut self, rows: &Value, dir: &Path, prefix: &str, root_targets: bool) {
        let Some(array) = rows.as_array() else {
            self.report(
                prefix,
                "expected a literal entry list; config expressions are not evaluated",
            );
            return;
        };
        let mut stack = vec![Frame {
            data: Arc::new(rows.clone()),
            pointer: String::new(),
            dir: dir.into(),
            prefix: prefix.into(),
            root_targets,
            position: 0,
            len: array.len(),
            canonical: None,
        }];
        while let Some(frame) = stack.last_mut() {
            if frame.position == frame.len {
                let canonical = frame.canonical.take();
                stack.pop();
                if let Some(canonical) = canonical {
                    self.ancestors.shift_remove(&canonical);
                }
                continue;
            }
            let position = frame.position;
            frame.position += 1;
            let path = format!("{}/{position}", frame.prefix);
            let pointer = format!("{}/{position}", frame.pointer);
            let data = frame.data.clone();
            let row = data.pointer(&pointer).unwrap_or(&Value::Null);
            let dir = frame.dir.clone();
            let root_targets = frame.root_targets;
            let index = self.entries.len();
            let id = row["id"].as_str().map(str::to_owned);
            let name = row["name"].as_str().map(str::to_owned);
            self.entries.push(ConfigSchemaEntry {
                path: path.clone(),
                id: id.clone(),
                name: name.clone(),
                status: "error".into(),
                config_ref: "#/$defs/unknownConfig".into(),
                tree: None,
            });
            if root_targets
                && let Some(id) = &id
                && !id.is_empty()
            {
                self.targets.insert(id.clone(), None);
            }
            let Some(name) = name else {
                self.report(
                    &path,
                    "each entry must be a mapping with a literal plugin name",
                );
                continue;
            };
            if row.get("id").is_some_and(|v| !v.is_string()) {
                self.report(&path, "id must be a literal string");
                continue;
            }
            if row
                .get("group")
                .is_some_and(|v| !v.is_boolean() && !v.is_null())
            {
                self.report(&path, "group must be a literal boolean or null");
                continue;
            }
            if root_targets
                && let Some(id) = id
                && !id.is_empty()
            {
                self.targets.insert(id, Some(index));
            }
            // Included rows keep their literal names; resolve them in their owning file's tree.
            let lookup = if self.registry.resolve(&name).is_none()
                && (name.starts_with("./")
                    || name.starts_with("../")
                    || Path::new(&name).is_absolute())
            {
                let filename = dir.join(&name);
                let filename = if cfg!(windows) {
                    crate::expr::windows_normalize(&filename.to_string_lossy())
                } else {
                    crate::expr::normalize(&filename.to_string_lossy())
                };
                url::Url::from_file_path(filename).ok().map(String::from)
            } else {
                None
            };
            let lookup = lookup.as_deref().unwrap_or(&name);
            let definition = self.registry.resolve(lookup);
            let kind = definition.and_then(|d| match d.name() {
                "cordis:group" => Some("group"),
                "cordis:include" => Some("include"),
                _ => None,
            });
            if definition.is_some() && kind.is_none() && self.registry.is_carrier(lookup) {
                self.report(&path,"unrecognized Loader tree carrier; use cordis:group or cordis:include for native child collection");
            }
            let config_ref = if let Some(kind) = kind {
                self.entries[index].tree = Some(kind.into());
                if kind == "group" {
                    "#/$defs/entryList".into()
                } else {
                    "#/$defs/includeConfig".into()
                }
            } else if definition.is_some() && self.registry.schema(lookup).is_none() {
                "#/$defs/unknownConfig".into()
            } else if let Some(definition) = definition {
                let identity = std::ptr::from_ref(definition) as usize;
                if let Some(reference) = self.schemas.get(&identity) {
                    reference.clone()
                } else {
                    let key = format!("config{}", self.schemas.len());
                    let mut schema = definition.schema().clone();
                    let missing = if schema["type"] == "object" || schema["properties"].is_object()
                    {
                        json!({})
                    } else {
                        Value::Null
                    };
                    if hoh_kernel::config::validate(&schema, &missing).is_err() {
                        self.required.insert(format!("#/$defs/{key}"));
                    }
                    if let Some(definitions) = schema
                        .as_object_mut()
                        .and_then(|o| o.remove("$defs"))
                        .and_then(|v| v.as_object().cloned())
                    {
                        for (name, mut child) in definitions {
                            rewrite(&mut child, &key);
                            self.definitions
                                .insert(format!("{key}_{name}"), projected(child));
                        }
                    }
                    rewrite(&mut schema, &key);
                    self.definitions.insert(key.clone(), projected(schema));
                    let reference = format!("#/$defs/{key}");
                    self.schemas.insert(identity, reference.clone());
                    reference
                }
            } else {
                self.report(&path, format!("cannot resolve plugin {name}"));
                "#/$defs/unknownConfig".into()
            };
            self.entries[index].status =
                if definition.is_some() && kind.is_none() && self.registry.schema(lookup).is_none()
                {
                    "absent".into()
                } else if definition.is_some() {
                    "schema".into()
                } else {
                    "error".into()
                };
            self.entries[index].config_ref = config_ref.clone();
            self.names
                .entry(name.clone())
                .or_default()
                .insert(config_ref);
            let dormant = row.get("config").is_none()
                && row["group"] != true
                && (row["disabled"] == true || expression(&row["disabled"]).is_some());
            if !dormant {
                match kind {
                    Some("group") => {
                        if let Some(config) = row.get("config") {
                            if config.is_array() {
                                stack.push(Frame {
                                    data: data.clone(),
                                    pointer: format!("{pointer}/config"),
                                    dir: dir.clone(),
                                    prefix: format!("{path}/config"),
                                    root_targets: root_targets && row["group"] == true,
                                    position: 0,
                                    len: config.as_array().map(Vec::len).unwrap_or_default(),
                                    canonical: None,
                                });
                            } else {
                                self.report(&path, "expected a literal entry list; config expressions are not evaluated");
                            }
                        } else {
                            self.report(&path, "entry list config is missing");
                        }
                    }
                    Some("include") => match self.include(row.get("config"), &dir, &path).await {
                        Ok((value, filename, canonical)) => {
                            self.ancestors.insert(canonical.clone());
                            let len = value.as_array().map(Vec::len).unwrap_or_default();
                            stack.push(Frame {
                                data: Arc::new(value),
                                pointer: String::new(),
                                dir: filename.parent().unwrap_or(Path::new(".")).into(),
                                prefix: format!("{path}/include"),
                                root_targets: false,
                                position: 0,
                                len,
                                canonical: Some(canonical),
                            });
                        }
                        Err(message) => self.report(&path, message),
                    },
                    _ => {}
                }
            }
            // group metadata affects the patch index even for a non-carrier plugin.
            if root_targets && row["group"] == true && kind != Some("group") {
                self.invalidate_targets(&row["config"]);
            }
        }
    }
    fn invalidate_targets(&mut self, rows: &Value) {
        for row in rows.as_array().into_iter().flatten() {
            if let Some(id) = row["id"].as_str()
                && !id.is_empty()
            {
                self.targets.insert(id.into(), None);
            }
            if row["group"] == true {
                self.invalidate_targets(&row["config"]);
            }
        }
    }
    async fn include(
        &mut self,
        config: Option<&Value>,
        dir: &Path,
        prefix: &str,
    ) -> Result<(Value, PathBuf, PathBuf), String> {
        let config = config.ok_or("include config is missing")?;
        if !config.is_object() || expression(config).is_some() {
            return Err(
                "include config must be literal; config expressions are not evaluated".into(),
            );
        }
        let filename = config["path"]
            .as_str()
            .ok_or("include path must be literal; config expressions are not evaluated")?;
        let filename = if filename.starts_with("file:") {
            url::Url::parse(filename)
                .map_err(|e| e.to_string())?
                .to_file_path()
                .map_err(|_| "invalid include file URL")?
        } else {
            dir.join(filename)
        };
        let filename = PathBuf::from(if cfg!(windows) {
            crate::expr::windows_normalize(&filename.to_string_lossy())
        } else {
            crate::expr::normalize(&filename.to_string_lossy())
        });
        let extension = filename.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !["json", "yaml", "yml"].contains(&extension) {
            return Err(format!(
                "include extension {} is not supported",
                hoh_util_values::js_json_stringify(&Value::String(format!(".{extension}")))
            ));
        }
        let canonical = match tokio::fs::canonicalize(&filename).await {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => filename.clone(),
            Err(error) => return Err(error.to_string()),
        };
        if self.ancestors.contains(&canonical) {
            return Err(format!("include cycle at {}", filename.display()));
        }
        let mut value = match tokio::fs::read_to_string(&filename).await {
            Ok(text) if extension == "json" => hoh_util_values::js_json_parse(&text)
                .map_err(|_| format!("invalid JSON include {}", filename.display()))?,
            Ok(text) => crate::yaml::parse_value(&text, &filename).map_err(|error| {
                let at = match error {
                    crate::LoadError::Yaml { line, column, .. } => {
                        format!(" at line {line}, column {column}")
                    }
                    _ => String::new(),
                };
                format!("invalid YAML include {}{at}", filename.display())
            })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => config
                .get("initial")
                .cloned()
                .ok_or_else(|| format!("include file not found: {}", filename.display()))?,
            Err(e) => return Err(e.to_string()),
        };
        if !value.is_array() {
            return Err(
                "expected a literal entry list; config expressions are not evaluated".into(),
            );
        }
        if let Some(patches) = config.get("patches") {
            let applied = (|| {
                let base = parse_entries(&crate::yaml::write_yaml(&value), &filename)?;
                let selected = self.select_patches(&value, patches, prefix).map_err(crate::LoadError::Runtime)?;
                let patches = parse_patches(&crate::yaml::write_yaml(&selected), &filename)?;
                let result = crate::compose(&base, &[crate::Layer { source: crate::LayerSource::Launcher("schema"), file: Some(filename.clone()), patches }], &mut |message| self.diagnostics.push(ConfigSchemaDiagnostic { level:"warning".into(), path:Some(prefix.into()), message: message.strip_prefix("[schema] ").unwrap_or(&message).into() }));
                Ok::<_, crate::LoadError>(crate::yaml::entries_value(&result.entries))
            })().map_err(|_| format!("include patches could not be composed for {}; child declarations are unavailable", filename.display()))?;
            value = applied;
        }
        Ok((value, filename, canonical))
    }
    fn select_patches(
        &mut self,
        base: &Value,
        patches: &Value,
        path: &str,
    ) -> Result<Value, String> {
        fn index(rows: &Value, targets: &mut IndexMap<String, Value>) {
            for row in rows.as_array().into_iter().flatten() {
                if let Some(id) = row["id"].as_str() {
                    targets.insert(id.into(), row.clone());
                }
                if row["group"] == true {
                    index(&row["config"], targets);
                }
            }
        }
        let patches = patches.as_array().ok_or(
            "include patches must be a literal patch list; config expressions are not evaluated",
        )?;
        let mut targets = IndexMap::new();
        index(base, &mut targets);
        let mut selected = vec![];
        for patch in patches {
            if !patch.is_object() || expression(patch).is_some() {
                return Err("include patches must be literal mappings; config expressions are not evaluated".into());
            }
            for key in ["id", "name"] {
                if patch.get(key).is_some_and(|value| !value.is_string()) {
                    return Err(format!("{key} must be a literal string"));
                }
            }
            if patch
                .get("group")
                .is_some_and(|value| !value.is_boolean() && !value.is_null())
            {
                return Err("group must be a literal boolean or null".into());
            }
            if patch.get("insert").is_some_and(|value| !value.is_array()) {
                return Err(
                    "expected a literal entry list; config expressions are not evaluated".into(),
                );
            }
            let id = patch.get("id").filter(|v| crate::is_truthy(v));
            let warning = if let Some(insert) = patch.get("insert").filter(|v| crate::is_truthy(v))
            {
                let target = id.and_then(Value::as_str).and_then(|id| targets.get(id));
                if id.is_some() && target.is_none() {
                    Some(format!(
                        "patch insert: entry {} not found",
                        hoh_util_values::js_json_stringify(id.unwrap_or(&Value::Null))
                    ))
                } else if target.is_some_and(|row| row["group"] != true) {
                    Some(format!(
                        "patch insert: entry {} is not a group",
                        hoh_util_values::js_json_stringify(id.unwrap_or(&Value::Null))
                    ))
                } else {
                    index(insert, &mut targets);
                    None
                }
            } else if id.is_none() {
                Some("patch: id is required for non-insert patches".into())
            } else {
                None
            };
            if let Some(message) = warning {
                self.diagnostics.push(ConfigSchemaDiagnostic {
                    level: "warning".into(),
                    path: Some(path.into()),
                    message,
                });
            } else {
                selected.push(patch.clone());
            }
        }
        Ok(Value::Array(selected))
    }
}
fn rewrite(value: &mut Value, prefix: &str) {
    match value {
        Value::Object(object) => {
            if let Some(Value::String(reference)) = object.get_mut("$ref") {
                if reference == "#" {
                    *reference = format!("#/$defs/{prefix}");
                } else if reference.starts_with("#/$defs/") {
                    *reference = reference.replacen("#/$defs/", &format!("#/$defs/{prefix}_"), 1);
                }
            }
            for child in object.values_mut() {
                rewrite(child, prefix);
            }
        }
        Value::Array(array) => {
            for child in array {
                rewrite(child, prefix);
            }
        }
        _ => {}
    }
}
fn projected(mut schema: Value) -> Value {
    if let Some(object) = schema.as_object_mut() {
        for key in ["properties", "patternProperties"] {
            if let Some(properties) = object.get_mut(key).and_then(Value::as_object_mut) {
                for child in properties.values_mut() {
                    *child = projected(std::mem::take(child));
                }
            }
        }
        for key in ["items", "additionalProperties"] {
            if let Some(child) = object.get_mut(key)
                && !child.is_boolean()
            {
                *child = projected(std::mem::take(child));
            }
        }
        if let Some(items) = object.get_mut("prefixItems").and_then(Value::as_array_mut) {
            for item in items {
                *item = projected(std::mem::take(item));
            }
        }
        for key in ["anyOf", "oneOf", "allOf"] {
            if let Some(branches) = object.get_mut(key).and_then(Value::as_array_mut) {
                for branch in branches {
                    *branch = projected(std::mem::take(branch));
                }
            }
        }
    }
    json!({"anyOf":[schema,reference("loaderExpression")]})
}
