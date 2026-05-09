use crate::spec::ToolSpec;
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub struct Registry {
    tools: BTreeMap<String, ToolSpec>,
}

impl Registry {
    pub fn empty() -> Self {
        Self { tools: BTreeMap::new() }
    }

    pub fn load_from_dirs(dirs: &[PathBuf], disabled: &[String]) -> Result<Self> {
        let mut reg = Registry::empty();
        for d in dirs {
            if !d.exists() {
                tracing::debug!("tool dir missing: {}", d.display());
                continue;
            }
            reg.load_dir(d, disabled)?;
        }
        Ok(reg)
    }

    fn load_dir(&mut self, dir: &Path, disabled: &[String]) -> Result<()> {
        for entry in std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                self.load_dir(&path, disabled)?;
                continue;
            }
            if path.extension().and_then(|s| s.to_str()) != Some("yaml") {
                continue;
            }
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("read {}", path.display()))?;
            let spec: ToolSpec = serde_yaml::from_str(&text)
                .with_context(|| format!("parse {}", path.display()))?;
            if disabled.iter().any(|n| n == &spec.name) {
                tracing::info!("tool {} disabled by config", spec.name);
                continue;
            }
            match self.tools.get(&spec.name) {
                Some(existing) if existing.version >= spec.version => {
                    tracing::warn!(
                        "ignoring {} v{} from {}: existing v{} is at least as new",
                        spec.name, spec.version, path.display(), existing.version,
                    );
                }
                _ => {
                    self.tools.insert(spec.name.clone(), spec);
                }
            }
        }
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&ToolSpec> {
        self.tools.get(name)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.tools.keys().map(|s| s.as_str())
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &ToolSpec)> {
        self.tools.iter().map(|(k, v)| (k.as_str(), v))
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}
