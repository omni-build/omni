mod env_loader;

pub(crate) use env_loader::{EnvLoader, GetVarsArgs};
use maps::Map;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use ::env_loader::EnvLoaderError;
use dir_walker::{DirEntry as _, DirWalker};
use eyre::{Context as _, ContextCompat, OptionExt};
use globset::{Glob, GlobMatcher, GlobSetBuilder};
use omni_core::{ProjectGraph, TaskExecutionNode};
use system_traits::{
    EnvCurrentDir, EnvVar, EnvVars, FsCanonicalize, FsMetadata, FsRead,
    auto_impl, impls::RealSys as RealSysSync,
};

use crate::{
    commands::CliArgs,
    configurations::{
        ExtensionGraph, ProjectConfiguration, WorkspaceConfiguration,
    },
    constants::{self, PROJECT_DIR_VAR, WORKSPACE_DIR_VAR},
    core::{Project, Task},
    utils::env::EnvVarsMap,
};

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Context<TSys: ContextSys = RealSysSync> {
    env_loader: EnvLoader<TSys>,
    task_env_vars: Map<String, EnvVarsMap>,
    env_root_dir_marker: String,
    env_files: Vec<String>,
    projects: Option<Vec<Project>>,
    workspace: WorkspaceConfiguration,
    root_dir: PathBuf,
    sys: TSys,
}

#[auto_impl]
pub trait ContextSys:
    EnvCurrentDir + FsRead + FsMetadata + FsCanonicalize + Clone + EnvVar + EnvVars
{
}

impl<TSys: ContextSys> Context<TSys> {
    pub fn sys(&self) -> &TSys {
        &self.sys
    }

    pub fn env_files(&self) -> &[String] {
        &self.env_files
    }

    pub fn env_root_dir_marker(&self) -> &str {
        &self.env_root_dir_marker
    }

    pub fn from_args_and_sys(
        cli: &CliArgs,
        sys: TSys,
    ) -> eyre::Result<Context<TSys>> {
        let env = cli.env.as_deref().unwrap_or("development");
        let env_files = cli
            .env_file
            .iter()
            .map(|s| {
                if s.contains("{ENV}") {
                    s.replace("{ENV}", env)
                } else {
                    s.to_string()
                }
            })
            .collect::<Vec<_>>();

        let root_dir = get_root_dir(&sys)?;
        let root_marker =
            cli.env_root_dir_marker.clone().unwrap_or_else(|| {
                constants::WORKSPACE_OMNI.replace("{ext}", "yaml")
            });
        let ctx = Context {
            projects: None,
            task_env_vars: maps::map!(),
            env_loader: EnvLoader::new(
                sys.clone(),
                PathBuf::from(&root_marker),
                env_files
                    .iter()
                    .map(|s| Path::new(&s).to_path_buf())
                    .collect(),
            ),
            env_files,
            workspace: get_workspace_configuration(&root_dir, &sys)?,
            root_dir,
            env_root_dir_marker: root_marker,
            sys,
        };

        Ok(ctx)
    }

    pub fn get_project_graph(&self) -> eyre::Result<ProjectGraph> {
        let projects = self.get_projects().ok_or_else(|| {
            eyre::eyre!(
                "Failed to get projects. Did you run load_projects first?"
            )
        })?;

        Ok(ProjectGraph::from_projects(projects.to_vec())?)
    }

    pub fn get_current_dir(&self) -> std::io::Result<PathBuf> {
        self.sys.env_current_dir()
    }

    pub fn get_env_vars(
        &mut self,
        args: Option<&GetVarsArgs>,
    ) -> Result<EnvVarsMap, EnvLoaderError> {
        let envs = self.env_loader.get(args.unwrap_or(&GetVarsArgs {
            ..Default::default()
        }))?;

        Ok(envs)
    }

    pub fn get_task_env_vars(
        &self,
        task: &TaskExecutionNode,
    ) -> Option<&EnvVarsMap> {
        self.task_env_vars.get(task.full_task_name())
    }

    pub fn get_cached_env_vars(
        &self,
        path: &Path,
    ) -> eyre::Result<&EnvVarsMap> {
        let envs = self.env_loader.get_cached(path).ok_or_else(|| {
            eyre::eyre!(
                "env vars not found for path {} not found in cache",
                path.display()
            )
        })?;

        Ok(envs)
    }

    pub fn get_projects(&self) -> Option<&Vec<Project>> {
        self.projects.as_ref()
    }

    pub fn load_projects<TDirWalker: DirWalker>(
        &mut self,
        walker: &TDirWalker,
        load_env: bool,
    ) -> eyre::Result<&Vec<Project>> {
        #[cfg(feature = "enable-tracing")]
        let start_time = std::time::SystemTime::now();

        let mut project_paths = vec![];

        let mut match_b = GlobSetBuilder::new();

        for p in &self.workspace.projects {
            match_b.add(Glob::new(
                format!("{}/{}", &self.root_dir.display(), p).as_str(),
            )?);
        }

        let matcher = match_b.build()?;

        #[cfg(feature = "enable-tracing")]
        let start_walk_time = std::time::SystemTime::now();

        let mut num_iterations = 0;

        let project_files: Vec<_> = constants::SUPPORTED_EXTENSIONS
            .iter()
            .map(|ext| constants::PROJECT_OMNI.replace("{ext}", ext))
            .collect();

        for f in walker.walk_dir(&self.root_dir) {
            num_iterations += 1;
            let f = f.map_err(|_| eyre::eyre!("Failed to walk dir"))?;

            if !self.sys.fs_is_dir(f.path())? {
                continue;
            }

            let dir = self.sys.fs_canonicalize(f.path())?;
            let dir_str = dir.display().to_string();

            if matcher.is_match(&dir_str) {
                for project_file in &project_files {
                    let p = f.path().join(project_file);
                    if self.sys.fs_exists(&p)? && self.sys.fs_is_file(p)? {
                        trace::debug!("Found project directory: {:?}", dir);
                        project_paths.push(
                            self.sys.fs_canonicalize(dir.join(project_file))?,
                        );
                        break;
                    }
                }
            }
        }

        #[cfg(feature = "enable-tracing")]
        trace::debug!(
            "Found {} project directories in {} ms, walked {} items",
            project_paths.len(),
            start_walk_time.elapsed().unwrap_or_default().as_millis(),
            num_iterations
        );

        let mut project_configs = vec![];
        {
            let mut loaded = HashSet::new();
            let mut to_process = project_paths.clone();
            while let Some(conf) = to_process.pop() {
                #[cfg(feature = "enable-tracing")]
                let start_time = std::time::SystemTime::now();

                let mut project = ProjectConfiguration::load(
                    &conf as &Path,
                    self.sys.clone(),
                )
                .wrap_err_with(|| {
                    format!(
                        "Failed to load project configuration file at {}",
                        conf.display()
                    )
                })?;

                #[cfg(feature = "enable-tracing")]
                {
                    let elapsed = start_time.elapsed().unwrap_or_default();
                    trace::debug!(
                        "Loaded project configuration file {:?} in {} ms",
                        conf,
                        elapsed.as_millis()
                    );
                }

                project.path = conf.to_string_lossy().to_string();
                loaded.insert(project.path.clone());

                for dep in &mut project.extends {
                    if dep.contains(WORKSPACE_DIR_VAR)
                        || dep.contains(PROJECT_DIR_VAR)
                    {
                        let env = maps::map![
                            WORKSPACE_DIR_VAR.to_string() => self.root_dir.to_string_lossy().to_string(),
                            PROJECT_DIR_VAR.to_string() => project.path.clone(),
                        ];
                        *dep = env::expand(dep, &env);
                    }

                    *dep = self
                        .sys
                        .fs_canonicalize(
                            Path::new(&project.path)
                                .parent()
                                .ok_or_eyre("Can't get parent")?
                                .join(&dep),
                        )?
                        .to_string_lossy()
                        .to_string();

                    if !loaded.contains(dep) {
                        to_process.push(PathBuf::from(dep.clone()));
                    }
                }

                project_configs.push(project);
            }
        }
        let project_configs = ExtensionGraph::from_nodes(project_configs)?
            .get_or_process_all_nodes()?;

        let mut projects = vec![];

        let project_paths = project_paths
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect::<HashSet<_>>();

        for config in project_configs
            .into_iter()
            .filter(|config| project_paths.contains(&config.path))
        {
            let dir = Path::new(&config.path)
                .parent()
                .ok_or_eyre("Can't get parent")?
                .to_path_buf();
            if load_env {
                let mut extras = maps::map![
                    "PROJECT_NAME".to_string() => config.name.to_string(),
                    "PROJECT_DIR".to_string() => config.path.to_string()
                ];

                let overrides = &config.env.vars;
                if !overrides.as_map().is_empty() {
                    extras.extend(overrides.to_map_to_inner());
                }

                let env_files = config
                    .env
                    .files
                    .as_vec()
                    .iter()
                    .map::<&Path, _>(|s| s)
                    .collect::<Vec<_>>();

                // load the env vars for the project
                _ = self.get_env_vars(Some(&GetVarsArgs {
                    start_dir: Some(&dir),
                    override_vars: Some(&extras),
                    env_files: if env_files.is_empty() {
                        Some(&env_files[..])
                    } else {
                        None
                    },
                }))?;

                for (name, task) in config.tasks.iter() {
                    if let Some(env) = task.env()
                        && let Some(vars) = env.vars.as_ref()
                    {
                        let key = format!("{}#{}", config.name, name);

                        self.task_env_vars.insert(key, vars.to_map_to_inner());
                    }
                }
            }
            projects.push(Project::new(
                config.name,
                dir,
                config.dependencies.to_vec_inner(),
                config
                    .tasks
                    .iter()
                    .map(|(task_name, v)| {
                        let mapped: Task = v.clone().get_task(task_name);

                        (task_name.clone(), mapped)
                    })
                    .collect(),
            ));
        }

        // check duplicate names
        let mut names = HashSet::new();
        for project in &projects {
            if names.contains(&project.name) {
                let projects = projects
                    .iter()
                    .filter(|p| p.name == project.name)
                    .map(|p| format!("  -> {}", p.dir.display()))
                    .collect::<Vec<_>>()
                    .join("\n");

                return Err(eyre::eyre!(
                    "Duplicate project name: {}\n\nProjects with same name:\n{}",
                    project.name,
                    projects
                ));
            }

            names.insert(project.name.clone());
        }

        self.projects = Some(projects);

        #[cfg(feature = "enable-tracing")]
        {
            let elapsed = start_time.elapsed().unwrap_or_default();
            trace::debug!(
                "Loaded {} projects in {} ms",
                self.projects.as_ref().map_or(0, |p| p.len()),
                elapsed.as_millis()
            );
        }

        Ok(self
            .projects
            .as_ref()
            .expect("Should be able to load projects"))
    }

    pub fn root_dir(&self) -> &Path {
        &self.root_dir
    }

    pub fn get_filter_matcher(
        &self,
        glob_filter: &str,
    ) -> eyre::Result<GlobMatcher> {
        Ok(Glob::new(glob_filter)?.compile_matcher())
    }

    pub fn get_filtered_projects(
        &self,
        glob_filter: &str,
    ) -> eyre::Result<Vec<&Project>> {
        // short circuit if glob_filter is *, micro-optimization
        if glob_filter == "*" || glob_filter == "**" {
            return Ok(self
                .get_projects()
                .wrap_err("Failed to get projects")?
                .iter()
                .collect());
        }

        let matcher = self.get_filter_matcher(glob_filter)?;
        let result = self
            .get_projects()
            .expect("Should be able to get projects after load");

        Ok(result
            .iter()
            .filter(|p| matcher.is_match(&p.name))
            .collect())
    }

    pub fn get_workspace_configuration(&self) -> &WorkspaceConfiguration {
        &self.workspace
    }
}

fn get_root_dir(sys: &impl ContextSys) -> eyre::Result<PathBuf> {
    let current_dir = sys.env_current_dir()?;

    for p in current_dir.ancestors() {
        let workspace_files = constants::SUPPORTED_EXTENSIONS
            .iter()
            .map(|ext| constants::WORKSPACE_OMNI.replace("{ext}", ext));

        for workspace_file in workspace_files {
            let f = p.join(workspace_file);
            if sys.fs_exists(&f)? && sys.fs_is_file(&f)? {
                return Ok(p.to_path_buf());
            }
        }
    }

    eyre::bail!("Could not find workspace configuration file");
}

fn get_workspace_configuration(
    root_dir: &Path,
    sys: &impl ContextSys,
) -> eyre::Result<WorkspaceConfiguration> {
    let workspace_files = constants::SUPPORTED_EXTENSIONS
        .iter()
        .map(|ext| constants::WORKSPACE_OMNI.replace("{ext}", ext));

    let mut ws_path = None;

    for workspace_file in workspace_files {
        let f = root_dir.join(workspace_file);
        if sys.fs_exists(&f)? && sys.fs_is_file(&f)? {
            ws_path = Some(f);
            break;
        }
    }

    let ws_path = ws_path.ok_or_else(|| {
        eyre::eyre!("Could not find workspace configuration file")
    })?;

    let f = sys.fs_read(&ws_path)?;
    let w = serde_yml::from_slice::<WorkspaceConfiguration>(&f).wrap_err_with(
        || {
            format!(
                "Failed to load workspace configuration: '{}'",
                ws_path.to_string_lossy()
            )
        },
    )?;

    Ok(w)
}

#[cfg(test)]
mod tests {
    use crate::tracer::TraceLevel;

    use super::*;
    use dir_walker::impls::{
        InMemoryDirEntry, InMemoryDirWalker, InMemoryMetadata,
    };
    use system_traits::impls::InMemorySys;
    use system_traits::*;

    fn create_sys() -> InMemorySys {
        let sys = InMemorySys::default();

        sys.fs_create_dir_all("/root/nested/project-1")
            .expect("Can't create project-1 dir");

        sys.fs_create_dir_all("/root/nested/project-2")
            .expect("Can't create project-2 dir");
        sys.fs_create_dir_all("/root/nested/project-3")
            .expect("Can't create project-3 dir");

        sys.fs_create_dir_all("/root/base")
            .expect("Can't create project-2 dir");

        sys.fs_write(
            "/root/.env",
            include_str!("../../test_fixtures/.env.root"),
        )
        .expect("Can't write root env file");
        sys.fs_write(
            "/root/.env.local",
            include_str!("../../test_fixtures/.env.root.local"),
        )
        .expect("Can't write root local env file");

        sys.fs_write(
            "/root/nested/.env",
            include_str!("../../test_fixtures/.env.nested"),
        )
        .expect("Can't write nested env file");
        sys.fs_write(
            "/root/nested/.env.local",
            include_str!("../../test_fixtures/.env.nested.local"),
        )
        .expect("Can't write nested local env file");

        sys.fs_write(
            "/root/nested/project-1/.env",
            include_str!("../../test_fixtures/.env.project-1"),
        )
        .expect("Can't write project env file");
        sys.fs_write(
            "/root/nested/project-1/.env.local",
            include_str!("../../test_fixtures/.env.project-1.local"),
        )
        .expect("Can't write project local env file");
        sys.fs_write(
            "/root/nested/project-1/project.omni.yaml",
            include_str!("../../test_fixtures/project-1.omni.yaml"),
        )
        .expect("Can't write project config file");

        sys.fs_write(
            "/root/nested/project-2/.env",
            include_str!("../../test_fixtures/.env.project-2"),
        )
        .expect("Can't write project env file");
        sys.fs_write(
            "/root/nested/project-2/.env.local",
            include_str!("../../test_fixtures/.env.project-2.local"),
        )
        .expect("Can't write project local env file");
        sys.fs_write(
            "/root/nested/project-2/project.omni.yaml",
            include_str!("../../test_fixtures/project-2.omni.yaml"),
        )
        .expect("Can't write project config file");
        sys.fs_write(
            "/root/nested/project-3/project.omni.yaml",
            include_str!("../../test_fixtures/project-3.omni.yaml"),
        )
        .expect("Can't write project config file");

        sys.fs_write(
            "/root/workspace.omni.yaml",
            include_str!("../../test_fixtures/workspace.omni.yaml"),
        )
        .expect("Can't write workspace config file");

        sys.fs_write(
            "/root/base/base-1.omni.yaml",
            include_str!("../../test_fixtures/base-1.omni.yaml"),
        )
        .expect("Can't write project config file");
        sys.fs_write(
            "/root/base/base-2.omni.yaml",
            include_str!("../../test_fixtures/base-2.omni.yaml"),
        )
        .expect("Can't write project config file");

        sys.env_set_current_dir("/root/nested/project-1")
            .expect("Can't set current dir");

        sys
    }

    fn create_ctx(env: &str, sys: Option<InMemorySys>) -> Context<InMemorySys> {
        let sys = sys.unwrap_or_else(create_sys);

        let cli = &CliArgs {
            env_root_dir_marker: None,
            env_file: vec![
                ".env".to_string(),
                ".env.local".to_string(),
                ".env.{ENV}".to_string(),
                ".env.{ENV}.local".to_string(),
            ],
            env: Some(String::from(env)),
            stdout_trace_level: TraceLevel::None,
            file_trace_level: TraceLevel::None,
            stderr_trace: false,
            file_trace_output: None,
        };

        Context::from_args_and_sys(cli, sys).expect("Can't create context")
    }

    fn create_dir_entries() -> Vec<InMemoryDirEntry> {
        vec![
            InMemoryDirEntry::new(
                PathBuf::from("/root"),
                false,
                InMemoryMetadata::default(),
            ),
            InMemoryDirEntry::new(
                PathBuf::from("/root/base"),
                false,
                InMemoryMetadata::default(),
            ),
            InMemoryDirEntry::new(
                PathBuf::from("/root/nested"),
                false,
                InMemoryMetadata::default(),
            ),
            InMemoryDirEntry::new(
                PathBuf::from("/root/nested/project-1"),
                false,
                InMemoryMetadata::default(),
            ),
            InMemoryDirEntry::new(
                PathBuf::from("/root/nested/project-2"),
                false,
                InMemoryMetadata::default(),
            ),
            InMemoryDirEntry::new(
                PathBuf::from("/root/nested/project-3"),
                false,
                InMemoryMetadata::default(),
            ),
        ]
    }

    fn create_dir_walker(
        dir_entries: Option<Vec<InMemoryDirEntry>>,
    ) -> InMemoryDirWalker {
        let entries = dir_entries.unwrap_or_else(create_dir_entries);
        let walker = InMemoryDirWalker::new(entries);

        walker
    }

    #[test]
    pub fn test_load_env_vars() {
        let mut ctx = create_ctx("testing", None);

        let env = ctx.get_env_vars(None).expect("Can't load env vars");

        assert_eq!(
            env.get("SHARED_ENV").map(String::as_str),
            Some("root-local-nested-local-project-local")
        );
    }

    #[test]
    fn test_load_projects() {
        let mut ctx = create_ctx("testing", None);

        ctx.load_projects(&create_dir_walker(None), true)
            .expect("Can't load projects");

        let projects = ctx.get_projects().expect("Can't get projects");

        assert_eq!(projects.len(), 3, "Should be 3 projects");

        let project_1 = projects.iter().find(|p| p.name == "project-1");

        assert!(project_1.is_some(), "Can't find project-1");

        let project_2 = projects.iter().find(|p| p.name == "project-2");

        assert!(project_2.is_some(), "Can't find project-2");

        let project_3 = projects.iter().find(|p| p.name == "project-3");

        assert!(project_3.is_some(), "Can't find project-3");
    }

    #[test]
    fn test_load_projects_with_duplicate_names() {
        let sys = create_sys();
        sys.fs_create_dir_all("/root/nested/project-4")
            .expect("Can't create project-4 dir");
        sys.fs_write(
            "/root/nested/project-4/project.omni.yaml",
            include_str!("../../test_fixtures/project-1.omni.yaml"),
        )
        .expect("Can't write project config file");

        let mut ctx = create_ctx("testing", Some(sys));

        let mut dir_walker = create_dir_walker(None);

        dir_walker.add(InMemoryDirEntry::new(
            PathBuf::from("/root/nested/project-4"),
            false,
            InMemoryMetadata::default(),
        ));

        let projects = ctx.load_projects(&dir_walker, true);

        assert!(
            projects
                .expect_err("Should be an error")
                .to_string()
                .contains("Duplicate project name: project-1"),
            "Should report duplicate project name"
        );
    }

    #[test]
    fn test_get_project_graph() {
        let mut ctx = create_ctx("testing", None);

        ctx.load_projects(&create_dir_walker(None), true)
            .expect("Can't load projects");

        let project_graph = ctx.get_project_graph().expect("Can't get graph");

        assert_eq!(project_graph.count(), 3);
    }

    #[test]
    fn test_project_extensions() {
        let mut ctx = create_ctx("testing", None);

        ctx.load_projects(&create_dir_walker(None), true)
            .expect("Can't load projects");

        let project_graph = ctx.get_project_graph().expect("Can't get graph");
        let project3 = project_graph
            .get_project_by_name("project-3")
            .expect("Can't get project-3");

        assert_eq!(project3.tasks.len(), 2, "Should be 2 tasks");
        assert_eq!(
            project3.tasks["from-base-1"].command,
            "echo \"from base-1\""
        );
        assert_eq!(
            project3.tasks["from-base-2"].command,
            "echo \"from base-2\""
        );
    }
}
