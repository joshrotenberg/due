use super::{Language, contract};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: u32,
    pub entry: PathBuf,
    pub input_schema: PathBuf,
    pub output_schema: PathBuf,
    pub max_runs: usize,
    pub max_concurrency: usize,
    pub profiles: BTreeMap<String, Profile>,
    #[serde(default)]
    pub modules: BTreeMap<String, PathBuf>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub request: Value,
    pub binary: Option<PathBuf>,
}
pub struct ResolvedProfile {
    pub request: Value,
    pub binary: PathBuf,
    pub schema: Value,
    pub validator: jsonschema::Validator,
}
pub struct Bundle {
    pub manifest: Manifest,
    pub language: Language,
    pub entry: String,
    pub modules: BTreeMap<String, String>,
    pub input: Value,
    pub output_validator: jsonschema::Validator,
    pub profiles: BTreeMap<String, ResolvedProfile>,
    pub snapshot: Value,
}

pub fn read(path: &Path) -> Result<String, String> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .take((contract::MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > contract::MAX_BYTES {
        return Err(format!("{} exceeds 1 MiB", path.display()));
    }
    String::from_utf8(bytes).map_err(|e| e.to_string())
}
fn canonical(base: &Path, path: &Path) -> Result<PathBuf, String> {
    let path = base
        .join(path)
        .canonicalize()
        .map_err(|e| format!("{}: {e}", base.join(path).display()))?;
    if path.to_str().is_none() {
        return Err("workflow paths must be UTF-8".into());
    }
    Ok(path)
}
pub fn name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return Err("names must be 1..64 ASCII letters, digits, '-' or '_'".into());
    }
    Ok(())
}

impl Bundle {
    pub fn load(manifest_path: &Path, input_path: &Path) -> Result<Self, String> {
        let manifest_path = manifest_path.canonicalize().map_err(|e| e.to_string())?;
        let base = manifest_path.parent().ok_or("manifest has no parent")?;
        let source = read(&manifest_path)?;
        let manifest: Manifest =
            serde_json::from_str(&source).map_err(|e| format!("manifest: {e}"))?;
        if manifest.schema_version != 1 {
            return Err("manifest schema_version must be 1".into());
        }
        if !(1..=64).contains(&manifest.max_runs)
            || !(1..=8).contains(&manifest.max_concurrency)
            || manifest.max_concurrency > manifest.max_runs
        {
            return Err(
                "require max_runs 1..64 and max_concurrency 1..8, no greater than max_runs".into(),
            );
        }
        if manifest.profiles.is_empty()
            || manifest.profiles.len() > 32
            || manifest.modules.len() > 32
        {
            return Err("require 1..32 profiles and at most 32 local modules".into());
        }
        let mut sources: BTreeMap<PathBuf, String> = BTreeMap::new();
        let mut snapshot_file = |path: &Path| -> Result<String, String> {
            let path = canonical(base, path)?;
            if let Some(text) = sources.get(&path) {
                return Ok(text.clone());
            }
            let text = read(&path)?;
            sources.insert(path, text.clone());
            Ok(text)
        };
        let language = Language::from_path(&manifest.entry)?;
        let entry = snapshot_file(&manifest.entry)?;
        let input_schema: Value = serde_json::from_str(&snapshot_file(&manifest.input_schema)?)
            .map_err(|e| e.to_string())?;
        let output_schema: Value = serde_json::from_str(&snapshot_file(&manifest.output_schema)?)
            .map_err(|e| e.to_string())?;
        let input_source = snapshot_file(input_path)?;
        let input: Value =
            serde_json::from_str(&input_source).map_err(|e| format!("input: {e}"))?;
        contract::validate(&contract::validator(&input_schema)?, &input)
            .map_err(|e| format!("input: {e}"))?;
        contract::check_value(&input)?;
        let output_validator = contract::validator(&output_schema)?;
        let mut modules = BTreeMap::new();
        modules.insert("patterns/v1".into(), language.patterns().into());
        for (key, path) in &manifest.modules {
            name(key.strip_prefix("local/").ok_or(
                "module names must start with local/; bundled and host names are reserved",
            )?)?;
            if Language::from_path(path)? != language {
                return Err("module language must match entry".into());
            }
            modules.insert(key.clone(), snapshot_file(path)?);
        }
        let mut profiles = BTreeMap::new();
        for (key, profile) in &manifest.profiles {
            name(key)?;
            let mut request = profile.request.clone();
            let object = request
                .as_object_mut()
                .ok_or("profile request must be an object")?;
            if object.contains_key("prompt") {
                return Err("profile request must omit prompt; jobs provide it".into());
            }
            let cwd = match object.get("cwd") {
                Some(value) => value.as_str().ok_or("profile cwd must be a string")?,
                None => ".",
            };
            let cwd = canonical(base, Path::new(cwd))?;
            if !cwd.is_dir() {
                return Err("profile cwd must be a directory".into());
            }
            object.insert("cwd".into(), json!(cwd));
            let schema_path = canonical(
                base,
                Path::new(
                    object
                        .get("schema")
                        .and_then(Value::as_str)
                        .ok_or("every profile requires a schema path")?,
                ),
            )?;
            let schema: Value =
                serde_json::from_str(&snapshot_file(&schema_path)?).map_err(|e| e.to_string())?;
            let validator = contract::validator(&schema)?;
            object.insert("schema".into(), json!(schema_path));
            let resolved = crate::request::workflow_profile(&request)?;
            crate::validate::native_args(&resolved).map_err(|e| e.message)?;
            let binary = match &profile.binary {
                Some(path) => canonical(base, path)?,
                None => which::which(resolved.provider.as_str())
                    .map_err(|e| format!("profile {key} binary: {e}"))?
                    .canonicalize()
                    .map_err(|e| e.to_string())?,
            };
            if !binary.is_file() {
                return Err("profile binary must be a file".into());
            }
            profiles.insert(
                key.clone(),
                ResolvedProfile {
                    request,
                    binary,
                    schema,
                    validator,
                },
            );
        }
        let resolved: BTreeMap<_, _> = profiles
            .iter()
            .map(|(key, p)| (key.clone(), json!({"request":p.request,"binary":p.binary})))
            .collect();
        let snapshot = json!({"manifest_path":manifest_path,"manifest_source":source,"sources":sources,"modules":modules,"input":input,"profiles":resolved});
        Ok(Self {
            manifest,
            language,
            entry,
            modules,
            input,
            output_validator,
            profiles,
            snapshot,
        })
    }
}
