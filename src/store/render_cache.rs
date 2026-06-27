use super::*;

const RENDER_CACHE_SCHEMA_NAME: &str = "render_cache";
const RENDER_CACHE_SCHEMA_VERSION: u64 = 1;

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

    pub(super) fn entry_matches(&self, output_path: &str, kind: &str, input_hash: &str) -> bool {
        self.entries
            .get(output_path)
            .is_some_and(|entry| entry.kind == kind && entry.input_hash == input_hash)
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
        if output_path.is_file() && cache.entry_matches(&output_rel, kind, &input_hash) {
            return Ok(FileWriteOutcome::Unchanged);
        }

        let rendered = render()?;
        let output_hash = blake3_fingerprint_str(&rendered);
        let outcome = write_string_if_changed(output_path, &rendered)?;
        cache.update(output_rel, kind, input_hash, output_hash);
        Ok(outcome)
    }

    pub(super) fn render_input_hash_cached(
        &self,
        cache: &mut RenderCache,
        kind: &str,
        output_path: &Path,
        template_keys: &[TemplateKey],
        payload: Value,
    ) -> Result<String> {
        let language = self.template_language()?;
        let templates = template_keys
            .iter()
            .map(|key| {
                self.template_content_hash_cached(cache, language, *key)
                    .map(|hash| {
                        json!({
                            "key": key.as_str(),
                            "hash": hash,
                        })
                    })
            })
            .collect::<Result<Vec<_>>>()?;
        let input = json!({
            "kind": kind,
            "output_path": rel_path(&self.root, output_path),
            "language": language.as_str(),
            "templates": templates,
            "payload": payload,
        });
        blake3_json_fingerprint(&input)
    }

    pub(super) fn template_content_hash(
        &self,
        language: TemplateLanguage,
        key: TemplateKey,
    ) -> Result<String> {
        let rel = PathBuf::from("templates").join(language_template_path(language, key));
        let path = self.root.join(&rel);
        let text = if path.exists() {
            read_to_string(&path, "read template")?
        } else {
            key.builtin_text(language).to_string()
        };
        Ok(blake3_fingerprint_str(&text))
    }

    fn template_content_hash_cached(
        &self,
        cache: &mut RenderCache,
        language: TemplateLanguage,
        key: TemplateKey,
    ) -> Result<String> {
        let cache_key = format!("{}:{}", language.as_str(), key.as_str());
        if let Some(hash) = cache.template_hashes.get(&cache_key) {
            return Ok(hash.clone());
        }
        let hash = self.template_content_hash(language, key)?;
        cache.template_hashes.insert(cache_key, hash.clone());
        Ok(hash)
    }

    fn render_cache_path(&self) -> PathBuf {
        self.root.join(".afmail/render-cache.json")
    }
}

pub(super) fn blake3_json_fingerprint<T: Serialize>(value: &T) -> Result<String> {
    let data = serde_json::to_vec(value).map_err(|e| AppError::json("serialize hash input", &e))?;
    Ok(blake3_fingerprint(&data))
}
