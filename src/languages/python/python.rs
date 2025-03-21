use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use tracing::debug;

use constants::env_vars::EnvVars;

use crate::config::LanguageVersion;
use crate::hook::Hook;
use crate::languages::LanguageImpl;
use crate::languages::python::uv::UvInstaller;
use crate::process::Cmd;
use crate::run::run_by_batch;
use crate::store::{Store, ToolBucket};

#[derive(Debug, Copy, Clone)]
pub struct Python;

impl LanguageImpl for Python {
    fn supports_dependency(&self) -> bool {
        true
    }

    // TODO: fallback to virtualenv, pip
    async fn install(&self, hook: &Hook) -> anyhow::Result<()> {
        let venv = hook.env_path().expect("Python must have env path");

        let uv = UvInstaller::install().await?;

        let store = Store::from_settings()?;
        let python_install_dir = store.tools_path(ToolBucket::Python);

        let uv_cmd = |summary| {
            let mut cmd = Cmd::new(&uv, summary);
            cmd.env(EnvVars::UV_PYTHON_INSTALL_DIR, &python_install_dir);
            cmd
        };

        // Create venv
        let mut cmd = uv_cmd("create venv");
        cmd.arg("venv").arg(venv);

        match hook.language_version {
            LanguageVersion::Specific(ref version) => {
                cmd.arg("--python").arg(version);
            }
            LanguageVersion::System => {
                cmd.arg("--python-preference").arg("only-system");
            }
            // uv will try to use system Python and download if not found
            LanguageVersion::Default => {}
        }

        cmd.check(true).output().await?;

        // Install dependencies
        if let Some(repo_path) = hook.repo_path() {
            uv_cmd("install dependencies")
                .arg("pip")
                .arg("install")
                .arg(".")
                .args(&hook.additional_dependencies)
                .current_dir(repo_path)
                .env("VIRTUAL_ENV", venv)
                .check(true)
                .output()
                .await?;
        } else if !hook.additional_dependencies.is_empty() {
            uv_cmd("install dependencies")
                .arg("pip")
                .arg("install")
                .args(&hook.additional_dependencies)
                .env("VIRTUAL_ENV", venv)
                .check(true)
                .output()
                .await?;
        } else {
            debug!("No dependencies to install");
        }
        Ok(())
    }

    async fn check_health(&self) -> Result<()> {
        todo!()
    }

    async fn run(
        &self,
        hook: &Hook,
        filenames: &[&String],
        env_vars: &HashMap<&'static str, String>,
    ) -> Result<(i32, Vec<u8>)> {
        // Get environment directory and parse command
        let env_dir = hook.env_path().expect("Python must have env path");

        let cmds = shlex::split(&hook.entry)
            .ok_or_else(|| anyhow::anyhow!("Failed to parse entry command"))?;

        // Construct PATH with venv bin directory first
        let new_path = std::env::join_paths(
            std::iter::once(bin_dir(env_dir)).chain(
                EnvVars::var_os(EnvVars::PATH)
                    .as_ref()
                    .iter()
                    .flat_map(std::env::split_paths),
            ),
        )?;

        let run = async move |batch: Vec<String>| {
            // TODO: combine stdout and stderr
            let mut output = Cmd::new(&cmds[0], "run python command")
                .args(&cmds[1..])
                .env("VIRTUAL_ENV", env_dir)
                .env("PATH", &new_path)
                .env_remove("PYTHONHOME")
                .envs(env_vars)
                .args(&hook.args)
                .args(batch)
                .check(false)
                .output()
                .await?;

            output.stdout.extend(output.stderr);
            let code = output.status.code().unwrap_or(1);
            anyhow::Ok((code, output.stdout))
        };

        let results = run_by_batch(hook, filenames, run).await?;

        // Collect results
        let mut combined_status = 0;
        let mut combined_output = Vec::new();

        for (code, output) in results {
            combined_status |= code;
            combined_output.extend(output);
        }

        Ok((combined_status, combined_output))
    }
}

fn bin_dir(venv: &Path) -> PathBuf {
    if cfg!(windows) {
        venv.join("Scripts")
    } else {
        venv.join("bin")
    }
}
