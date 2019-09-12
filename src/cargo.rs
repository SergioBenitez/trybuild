use std::{fs, env};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::collections::BTreeMap as Map;

use crate::env::Update;
use crate::dependencies::{self, Dependency};
use crate::manifest::{Bin, Build, Config, Manifest, Name, Package, Workspace};
use crate::error::{Error, Result};
use crate::{Test, Expected, TestRunner};
use crate::rustflags;

use serde::Deserialize;

#[derive(Default)]
pub struct CargoRunner {
    project: Option<Project>,
}

#[derive(Debug)]
pub struct Project {
    pub dir: PathBuf,
    pub(crate) source_dir: PathBuf,
    pub target_dir: PathBuf,
    pub name: String,
    pub(crate) update: Update,
    pub has_pass: bool,
    pub(crate) has_compile_fail: bool,
    pub features: Option<Vec<String>>,
    pub(crate) workspace: PathBuf,
}

impl TestRunner for CargoRunner {
    type Error = Error;

    fn prepare(&mut self, tests: &[Test]) -> Result<()> {
        let (project, manifest) = prepare_project(tests)?;
        let manifest_toml = toml::to_string(&manifest)?;

        let config = make_config();
        let config_toml = toml::to_string(&config)?;

        fs::create_dir_all(path!(project.dir / ".cargo"))?;
        fs::write(path!(project.dir / ".cargo" / "config"), config_toml)?;
        fs::write(path!(project.dir / "Cargo.toml"), manifest_toml)?;
        fs::write(path!(project.dir / "main.rs"), b"fn main() {}\n")?;

        build_dependencies(&project)?;
        self.project = Some(project);
        Ok(())
    }

    fn build(&mut self, test: &Test) -> Result<Output> {
        let project = self.project.as_ref().expect("prepared");
        build_test(project, &test.name)
    }

    // SOOOOO, the original code _knows_ about compile vs. run and emits the
    // stderr from the compile step as warnings ("preferred") if compilation
    // succeeds but running fails. Because this `run` method combines them,
    // we currently emit the stderr from the run as warnings when we only want
    // the stderr from the compilation step.
    fn run(&mut self, test: &Test) -> Result<Output> {
        let project = self.project.as_ref().expect("prepared");
        run_test(project, test)
    }
}

fn make_config() -> Config {
    Config {
        build: Build {
            rustflags: rustflags::make_vec(),
        },
    }
}

pub fn prepare_project(tests: &[Test]) -> Result<(Project, Manifest)> {
    let metadata = metadata()?;
    let target_dir = metadata.target_directory;
    let workspace = metadata.workspace_root;

    let crate_name = env::var("CARGO_PKG_NAME").map_err(Error::PkgName)?;

    let mut has_pass = false;
    let mut has_compile_fail = false;
    for e in tests {
        match e.expected {
            Expected::Pass => has_pass = true,
            Expected::CompileFail => has_compile_fail = true,
        }
    }

    let source_dir = env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .ok_or(Error::ProjectDir)?;

    let features = crate::features::find();

    let mut project = Project {
        dir: path!(target_dir / "tests" / crate_name),
        source_dir,
        target_dir,
        name: format!("{}-tests", crate_name),
        update: crate::env::Update::env()?,
        has_pass,
        has_compile_fail,
        features,
        workspace,
    };

    let manifest = make_manifest(crate_name, &project, tests)?;
    if let Some(enabled_features) = &mut project.features {
        enabled_features.retain(|f| manifest.features.contains_key(f));
    }

    Ok((project, manifest))
}

fn make_manifest(
    crate_name: String,
    project: &Project,
    tests: &[Test],
) -> Result<Manifest> {
    let source_manifest = dependencies::get_manifest(&project.source_dir);
    let workspace_manifest = dependencies::get_workspace_manifest(&project.workspace);

    let features = source_manifest
        .features
        .keys()
        .map(|feature| {
            let enable = format!("{}/{}", crate_name, feature);
            (feature.clone(), vec![enable])
        })
        .collect();

    let mut manifest = Manifest {
        package: Package {
            name: project.name.clone(),
            version: "0.0.0".to_owned(),
            edition: source_manifest.package.edition,
            publish: false,
        },
        features,
        dependencies: Map::new(),
        bins: Vec::new(),
        workspace: Some(Workspace {}),
        // Within a workspace, only the [patch] and [replace] sections in
        // the workspace root's Cargo.toml are applied by Cargo.
        patch: workspace_manifest.patch,
        replace: workspace_manifest.replace,
    };

    manifest.dependencies.extend(source_manifest.dependencies);
    manifest
        .dependencies
        .extend(source_manifest.dev_dependencies);
    manifest.dependencies.insert(
        crate_name,
        Dependency {
            version: None,
            path: Some(project.source_dir.clone()),
            default_features: false,
            features: Vec::new(),
            rest: Map::new(),
        },
    );

    manifest.bins.push(Bin {
        name: Name(project.name.to_owned()),
        path: Path::new("main.rs").to_owned(),
    });

    for (i, test) in tests.iter().enumerate() {
        manifest.bins.push(Bin {
            name: Name(Test::gen_name(i)),
            path: project.source_dir.join(&test.path),
        });
    }

    Ok(manifest)
}

#[derive(Deserialize)]
pub struct Metadata {
    pub target_directory: PathBuf,
    pub workspace_root: PathBuf,
}

fn raw_cargo() -> Command {
    Command::new(option_env!("CARGO").unwrap_or("cargo"))
}

fn cargo(project: &Project) -> Command {
    let mut cmd = raw_cargo();
    cmd.current_dir(&project.dir);
    cmd.env("CARGO_TARGET_DIR", &project.target_dir);
    rustflags::set_env(&mut cmd);
    cmd
}

pub fn build_dependencies(project: &Project) -> Result<()> {
    let status = cargo(project)
        .arg(if project.has_pass { "build" } else { "check" })
        .arg("--bin")
        .arg(&project.name)
        .status()
        .map_err(Error::Cargo)?;

    if status.success() {
        Ok(())
    } else {
        Err(Error::CargoFail)
    }
}

pub fn build_test(project: &Project, name: &str) -> Result<Output> {
    let _ = cargo(project)
        .arg("clean")
        .arg("--package")
        .arg(&project.name)
        .arg("--color=never")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    cargo(project)
        .arg(if project.has_pass { "build" } else { "check" })
        .arg("--bin")
        .arg(name)
        .args(features(project))
        .arg("--quiet")
        .arg("--color=never")
        .output()
        .map_err(Error::Cargo)
}

pub fn run_test(project: &Project, test: &Test) -> Result<Output> {
    cargo(project)
        .arg("run")
        .arg("--bin")
        .arg(&test.name)
        .args(features(project))
        .arg("--quiet")
        .arg("--color=never")
        .output()
        .map_err(Error::Cargo)
}

pub fn metadata() -> Result<Metadata> {
    let output = raw_cargo()
        .arg("metadata")
        .arg("--format-version=1")
        .output()
        .map_err(Error::Cargo)?;

    serde_json::from_slice(&output.stdout).map_err(Error::Metadata)
}

fn features(project: &Project) -> Vec<String> {
    match &project.features {
        Some(features) => vec![
            "--no-default-features".to_owned(),
            "--features".to_owned(),
            features.join(","),
        ],
        None => vec![],
    }
}
