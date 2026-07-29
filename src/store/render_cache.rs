use super::*;

const RENDER_CACHE_SCHEMA_NAME: &str = "render_cache";
const RENDER_CACHE_SCHEMA_VERSION: u64 = 2;
const RENDER_ENGINE_CONTRACT_VERSION: u64 = 2;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(super) struct RenderCache {
    schema_name: String,
    schema_version: u64,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    entries: BTreeMap<String, RenderCacheEntry>,
    #[serde(skip)]
    dirty: bool,
    #[serde(skip)]
    template_hashes: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(super) struct RenderCacheEntry {
    kind: String,
    input_hash: String,
    output_hash: String,
    updated_rfc3339: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct RenderWriteStats {
    pub(super) written_count: usize,
    pub(super) unchanged_count: usize,
}

impl RenderWriteStats {
    pub(super) fn record(&mut self, outcome: FileWriteOutcome) {
        match outcome {
            FileWriteOutcome::Written => self.written_count += 1,
            FileWriteOutcome::Unchanged => self.unchanged_count += 1,
        }
    }
}

impl RenderCache {
    fn normalized(mut self) -> Self {
        self.schema_name = RENDER_CACHE_SCHEMA_NAME.to_string();
        self.schema_version = RENDER_CACHE_SCHEMA_VERSION;
        self.dirty = false;
        self
    }

    fn entry_matches(
        &self,
        output_path: &str,
        kind: &str,
        input_hash: &str,
        output_hash: &str,
    ) -> bool {
        self.entries.get(output_path).is_some_and(|entry| {
            entry.kind == kind && entry.input_hash == input_hash && entry.output_hash == output_hash
        })
    }

    pub(super) fn update(
        &mut self,
        output_path: String,
        kind: &str,
        input_hash: String,
        output_hash: String,
    ) {
        let next = RenderCacheEntry {
            kind: kind.to_string(),
            input_hash,
            output_hash,
            updated_rfc3339: now_rfc3339(),
        };
        if self.entries.get(&output_path) != Some(&next) {
            self.entries.insert(output_path, next);
            self.dirty = true;
        }
    }
}

impl Workspace {
    pub(super) fn read_render_cache(&self) -> Result<RenderCache> {
        let path = self.render_cache_path();
        let data = match fs::read_to_string(&path) {
            Ok(data) => data,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(RenderCache::default().normalized());
            }
            Err(err) => return Err(AppError::io("read render cache", &err)),
        };
        let cache: RenderCache = match serde_json::from_str(&data) {
            Ok(cache) => cache,
            Err(_) => return Ok(RenderCache::default().normalized()),
        };
        if cache.schema_name != RENDER_CACHE_SCHEMA_NAME
            || cache.schema_version != RENDER_CACHE_SCHEMA_VERSION
        {
            return Ok(RenderCache::default().normalized());
        }
        Ok(cache.normalized())
    }

    pub(super) fn write_render_cache_if_changed(&self, cache: &mut RenderCache) -> Result<()> {
        if !cache.dirty {
            return Ok(());
        }
        let normalized = cache.clone().normalized();
        write_json_pretty_if_changed(&self.render_cache_path(), &normalized)?;
        *cache = normalized;
        Ok(())
    }

    pub(super) fn render_cached_string<F>(
        &self,
        cache: &mut RenderCache,
        output_path: &Path,
        kind: &str,
        input_hash: String,
        render: F,
    ) -> Result<FileWriteOutcome>
    where
        F: FnOnce() -> Result<String>,
    {
        let output_rel = rel_path(&self.root, output_path);
        if self.render_cache_entry_matches(cache, output_path, kind, &input_hash)? {
            return Ok(FileWriteOutcome::Unchanged);
        }

        let rendered = render()?;
        let output_hash = blake3_fingerprint_str(&rendered);
        let outcome = write_string_if_changed(output_path, &rendered)?;
        cache.update(output_rel, kind, input_hash, output_hash);
        Ok(outcome)
    }

    pub(super) fn render_cache_entry_matches(
        &self,
        cache: &RenderCache,
        output_path: &Path,
        kind: &str,
        input_hash: &str,
    ) -> Result<bool> {
        if !output_path.is_file() {
            return Ok(false);
        }
        let output = fs::read(output_path).map_err(|error| {
            AppError::new(
                "store_error",
                format!(
                    "read rendered output {}: {error}",
                    rel_path(&self.root, output_path)
                ),
            )
        })?;
        let output_hash = blake3_fingerprint(&output);
        Ok(cache.entry_matches(
            &rel_path(&self.root, output_path),
            kind,
            input_hash,
            &output_hash,
        ))
    }

    pub(super) fn render_input_hash_cached<T: Serialize>(
        &self,
        cache: &mut RenderCache,
        kind: &str,
        output_path: &Path,
        document: &T,
    ) -> Result<String> {
        let config = MailConfig::load(&self.root)?;
        let language = config.template_language();
        let default_identity_email = config.default_identity()?.email.clone();
        let workspace_template_tree_hash =
            self.workspace_template_tree_hash_cached(cache, language)?;
        let builtin_template_tree_hash = self.builtin_template_tree_hash_cached(cache, language)?;
        let input = json!({
            "kind": kind,
            "output_path": rel_path(&self.root, output_path),
            "language": language.as_str(),
            "render_contract": {
                "render_document_schema_version": RENDER_DOCUMENT_SCHEMA_VERSION,
                "render_engine_contract_version": RENDER_ENGINE_CONTRACT_VERSION,
                "render_engine_source_hash": render_engine_source_hash(),
                "timezone_utc_offset": config.resolved_timezone_offset().to_string(),
                "default_identity_email": default_identity_email,
            },
            "builtin_template_tree_hash": builtin_template_tree_hash,
            "workspace_template_tree_hash": workspace_template_tree_hash,
            "document": document,
        });
        blake3_json_fingerprint(&input)
    }

    fn workspace_template_tree_hash_cached(
        &self,
        cache: &mut RenderCache,
        language: TemplateLanguage,
    ) -> Result<String> {
        let cache_key = format!("workspace-tree:{}", language.as_str());
        if let Some(hash) = cache.template_hashes.get(&cache_key) {
            return Ok(hash.clone());
        }
        let templates_root = self.root.join(layout::TEMPLATES);
        let mut files = Vec::new();
        collect_template_files(
            &templates_root,
            &templates_root.join(language.as_str()),
            &mut files,
        )?;
        collect_template_files(&templates_root, &templates_root.join("shared"), &mut files)?;
        files.sort_by(|left, right| left.0.cmp(&right.0));
        let hash = blake3_json_fingerprint(&files)?;
        cache.template_hashes.insert(cache_key, hash.clone());
        Ok(hash)
    }

    fn builtin_template_tree_hash_cached(
        &self,
        cache: &mut RenderCache,
        language: TemplateLanguage,
    ) -> Result<String> {
        let cache_key = format!("builtin-tree:{}", language.as_str());
        if let Some(hash) = cache.template_hashes.get(&cache_key) {
            return Ok(hash.clone());
        }
        let templates = TemplateKey::ALL
            .into_iter()
            .map(|key| {
                (
                    language_template_path(language, key)
                        .to_string_lossy()
                        .replace('\\', "/"),
                    blake3_fingerprint_str(key.builtin_text(language)),
                )
            })
            .collect::<Vec<_>>();
        let hash = blake3_json_fingerprint(&templates)?;
        cache.template_hashes.insert(cache_key, hash.clone());
        Ok(hash)
    }

    fn render_cache_path(&self) -> PathBuf {
        self.root
            .join(layout::afmail_path(layout::AFMAIL_RENDER_CACHE))
    }
}

fn render_engine_source_hash() -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(include_bytes!("../render_document.rs"));
    hasher.update(include_bytes!("../templates.rs"));
    hasher.update(include_bytes!("render.rs"));
    hasher.finalize().to_hex().to_string()
}

fn collect_template_files(
    templates_root: &Path,
    directory: &Path,
    files: &mut Vec<(String, String)>,
) -> Result<()> {
    ensure_workspace_template_path_safe(templates_root, directory)?;
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(AppError::io("read template directory", &error)),
    };
    for entry in entries {
        let entry = entry.map_err(|error| AppError::io("read template directory entry", &error))?;
        let file_type = entry
            .file_type()
            .map_err(|error| AppError::io("read template file type", &error))?;
        let path = entry.path();
        if file_type.is_symlink() {
            return Err(AppError::new(
                "template_path_unsafe",
                format!("workspace template path {} is a symlink", path.display()),
            )
            .with_hint(
                "Use regular files and directories inside templates/; symlinks are rejected.",
            ));
        } else if file_type.is_dir() {
            collect_template_files(templates_root, &path, files)?;
        } else if file_type.is_file() {
            let relative = path
                .strip_prefix(templates_root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let content = fs::read(&path).map_err(|error| AppError::io("read template", &error))?;
            files.push((relative, blake3_fingerprint(&content)));
        }
    }
    Ok(())
}

pub(super) fn blake3_json_fingerprint<T: Serialize>(value: &T) -> Result<String> {
    let data = serde_json::to_vec(value).map_err(|e| AppError::json("serialize hash input", &e))?;
    Ok(blake3_fingerprint(&data))
}
