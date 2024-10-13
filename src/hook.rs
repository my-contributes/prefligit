use std::collections::HashMap;
use std::fmt::Display;
use std::path::PathBuf;

use anyhow::Result;
use thiserror::Error;
use url::Url;

use crate::config::{
    self, read_config, read_manifest, ConfigWire, ConfigHook, Language, ManifestHook, RepoLocation,
    ConfigRepo, Stage, CONFIG_FILE, MANIFEST_FILE,
};
use crate::fs::CWD;
use crate::store::{Repo, Store};

#[derive(Debug, Error)]
pub enum Error {
    #[error("Failed to parse URL: {0}")]
    InvalidUrl(#[from] url::ParseError),
    #[error("Failed to read config file: {0}")]
    ReadConfig(#[from] config::Error),
    #[error("Failed to initialize repo: {0}")]
    InitRepo(#[from] anyhow::Error),
    #[error("Hook not found: {hook} in repo {repo}")]
    HookNotFound { hook: String, repo: RepoLocation },
}

#[derive(Debug)]
pub struct RemoteRepo {
    /// Path to the stored repo.
    path: PathBuf,
    url: Url,
    rev: String,
    hooks: HashMap<String, ManifestHook>,
}

#[derive(Debug)]
pub enum Repo {
    Remote(RemoteRepo),
    Local(HashMap<String, ConfigHook>),
    Meta,
}

impl Repo {
    pub fn new(config: &ConfigRepo, store: &Store) -> Result<Self> {
        match &config.repo {
            RepoLocation::Remote(url) => store.clone_repo(config, url, None),
            RepoLocation::Local => Self::local(config.hooks.clone()),
            RepoLocation::Meta => Ok(Self::Meta),
        }
    }

    pub fn remote(url: String, rev: String, path: String) -> Result<Self> {
        let url = Url::parse(&url).map_err(Error::InvalidUrl)?;

        let path = PathBuf::from(path);
        let path = path.join(MANIFEST_FILE);
        let manifest = read_manifest(&path)?;
        let hooks = manifest
            .hooks
            .into_iter()
            .map(|hook| (hook.id.clone(), hook))
            .collect();

        Ok(Self::Remote(Self {
            path,
            url,
            rev,
            hooks,
        }))
    }

    pub fn local(hooks: Vec<ConfigHook>) -> Result<Self> {
        let hooks = hooks
            .into_iter()
            .map(|hook| (hook.id.clone(), hook))
            .collect();

        Ok(Self::Local(hooks))
    }

    pub fn meta() -> Self {
        todo!()
    }

    pub fn get_hook(&self, id: &str) -> Option<&ManifestHook> {
        match self {
            Repo::Remote(repo) => repo.hooks.get(id),
            Repo::Local(hooks) => hooks.get(id),
            Repo::Meta => None,
        }
    }
}

impl Display for Repo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Repo::Remote(repo) => write!(f, "{}@{}", repo.url, repo.rev),
            Repo::Local(_) => write!(f, "local"),
            Repo::Meta => write!(f, "meta"),
        }
    }
}

pub struct Project {
    root: PathBuf,
    config: ConfigWire,
}

impl Project {
    pub fn from_directory(root: PathBuf, config: Option<PathBuf>) -> Result<Self> {
        let config_path = config.unwrap_or_else(|| root.join(CONFIG_FILE));
        let config = read_config(&config_path).map_err(Error::ReadConfig)?;
        Ok(Self { root, config })
    }

    pub fn current(config: Option<PathBuf>) -> Result<Self> {
        Self::from_directory(CWD.clone(), config)
    }

    pub fn repos(&self, store: &Store) -> Result<Vec<Repo>> {
        // TODO: init in parallel
        self.config
            .repos
            .iter()
            .map(|repo| store.init_repo(repo, None))
            .collect::<Result<_>>()
    }

    pub fn hooks(&self, store: &Store) -> Result<Vec<Hook>> {
        let mut hooks = Vec::new();

        for repo_config in &self.config.repos {
            let repo = Repo::new(repo_config, store)?;

            match repo {
                Repo::Remote(repo) => {
                    for hook_config in &repo_config.hooks {
                        let Some(manifest_hook) = repo.hooks.get(&hook_config.id) else {
                            // Check hook id is valid.
                            return Err(Error::HookNotFound {
                                hook: hook_config.id.clone(),
                                repo: repo_config.repo.clone(),
                            })?;
                        };

                        // TODO: avoid clone
                        let mut hook = Hook::from(manifest_hook.clone());
                        hook.update(hook_config);
                        hook.fill(&self.config);
                        hooks.push(hook);
                    }
                }
                Repo::Local(local_hooks) => {
                    for hook_config in local_hooks.values() {
                        let mut hook = Hook::from(hook_config.clone());
                        hook.fill(&self.config);
                        hooks.push(hook);
                    }
                }
                Repo::Meta => {}
            }
        }
        Ok(hooks)
    }
}

#[derive(Debug)]
pub struct Hook {
    // Basic hook fields from the manifest.
    pub id: String,
    pub name: String,
    pub entry: String,
    pub language: Language,
    pub files: Option<String>,
    pub exclude: Option<String>,
    pub types: Option<Vec<String>>,
    pub types_or: Option<Vec<String>>,
    pub exclude_types: Option<Vec<String>>,
    pub always_run: Option<bool>,
    pub fail_fast: Option<bool>,
    pub verbose: Option<bool>,
    pub pass_filenames: Option<bool>,
    pub require_serial: Option<bool>,
    pub description: Option<String>,
    pub language_version: Option<String>,
    pub minimum_pre_commit_version: Option<String>,
    pub args: Option<Vec<String>>,
    pub stages: Option<Vec<Stage>>,

    // Additional fields from the repo configuration.
    pub alias: Option<String>,
    pub additional_dependencies: Option<Vec<String>>,
    pub log_file: Option<String>,
}

impl From<ConfigHook> for Hook {
    fn from(hook: ConfigHook) -> Self {
        Self {
            id: hook.id,
            name: hook.name,
        }
    }
}

impl Hook {
    pub fn update(&mut self, repo_hook: &ConfigHook) {
        self.alias = repo_hook.alias.clone();

        if let Some(name) = &repo_hook.name {
            self.name = name.clone();
        }
        if let Some(language_version) = &repo_hook.language_version {
            self.language_version = Some(language_version.clone());
        }
        if let Some(files) = &repo_hook.files {
            self.files = Some(files.clone());
        }
        if let Some(exclude) = &repo_hook.exclude {
            self.exclude = Some(exclude.clone());
        }
        if let Some(types) = &repo_hook.types {
            self.types = Some(types.clone());
        }
        if let Some(types_or) = &repo_hook.types_or {
            self.types_or = Some(types_or.clone());
        }
        if let Some(exclude_types) = &repo_hook.exclude_types {
            self.exclude_types = Some(exclude_types.clone());
        }
        if let Some(args) = &repo_hook.args {
            self.args = Some(args.clone());
        }
        if let Some(stages) = &repo_hook.stages {
            self.stages = Some(stages.clone());
        }
        if let Some(additional_dependencies) = &repo_hook.additional_dependencies {
            self.additional_dependencies = Some(additional_dependencies.clone());
        }
        if let Some(always_run) = &repo_hook.always_run {
            self.always_run = Some(*always_run);
        }
        if let Some(verbose) = &repo_hook.verbose {
            self.verbose = Some(*verbose);
        }
        if let Some(log_file) = &repo_hook.log_file {
            self.log_file = Some(log_file.clone());
        }
    }

    pub fn fill(&mut self, config: &ConfigWire) {
        let language = self.language;
        if self.language_version.is_none() {
            self.language_version = config
                .default_language_version
                .as_ref()
                .and_then(|v| v.get(&language).cloned())
        }
        if self.language_version.is_none() {
            self.language_version = Some(language.default_version());
        }

        if self.stages.is_none() {
            self.stages = config.default_stages.clone();
        }

        // TODO: check ENVIRONMENT_DIR with language_version and additional_dependencies
    }
}
