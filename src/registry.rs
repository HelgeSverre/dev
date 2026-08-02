use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use serde::Serialize;

use crate::detect::{
    ArtisanDetector, CargoDetector, CargoWorkspaceContributor, ComposerDetector, DartDetector,
    Detector, DockerDetector, DotnetDetector, DotnetWorkspaceContributor, GoDetector,
    GoWorkspaceContributor, GradleDetector, GradleWorkspaceContributor, JakeDetector, JustDetector,
    MakeDetector, MavenDetector, MavenWorkspaceContributor, MiseDetector, NodeDetector,
    NodeTestBinder, NodeWorkspaceContributor, PhpFileDetector, PythonFileDetector, SemaDetector,
    ShellDetector, SwiftDetector, TargetBinder, TargetRunner, TaskfileDetector, ZigDetector,
};
use crate::scan::DiscoveryFiles;

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Serialize)]
#[serde(transparent)]
pub struct DetectorId(&'static str);

impl DetectorId {
    #[must_use]
    pub const fn new(value: &'static str) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for DetectorId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Serialize)]
#[serde(transparent)]
pub struct CandidateSourceId(&'static str);

impl CandidateSourceId {
    #[must_use]
    pub const fn new(value: &'static str) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for CandidateSourceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Serialize)]
#[serde(transparent)]
pub struct ToolId(&'static str);

impl ToolId {
    #[must_use]
    pub const fn new(value: &'static str) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for ToolId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

pub const NODE: DetectorId = DetectorId::new("node");
pub const CARGO: DetectorId = DetectorId::new("cargo");
pub const COMPOSER: DetectorId = DetectorId::new("composer");
pub const ARTISAN: DetectorId = DetectorId::new("artisan");
pub const GO: DetectorId = DetectorId::new("go");
pub const GRADLE: DetectorId = DetectorId::new("gradle");
pub const MAVEN: DetectorId = DetectorId::new("maven");
pub const DOTNET: DetectorId = DetectorId::new("dotnet");
pub const JAKE: DetectorId = DetectorId::new("jake");
pub const JUST: DetectorId = DetectorId::new("just");
pub const TASKFILE: DetectorId = DetectorId::new("taskfile");
pub const MISE: DetectorId = DetectorId::new("mise");
pub const SEMA: DetectorId = DetectorId::new("sema");
pub const PHP_FILE: DetectorId = DetectorId::new("php-file");
pub const ZIG: DetectorId = DetectorId::new("zig");
pub const SWIFT: DetectorId = DetectorId::new("swift");
pub const DART: DetectorId = DetectorId::new("dart");
pub const PYTHON_FILE: DetectorId = DetectorId::new("python-file");
pub const SHELL: DetectorId = DetectorId::new("shell");
pub const MAKE: DetectorId = DetectorId::new("make");
pub const DOCKER: DetectorId = DetectorId::new("docker");

pub const NODE_SOURCE: CandidateSourceId = CandidateSourceId::new("node");
pub const VITE_SOURCE: CandidateSourceId = CandidateSourceId::new("vite");
pub const NEXT_SOURCE: CandidateSourceId = CandidateSourceId::new("next");
pub const CARGO_SOURCE: CandidateSourceId = CandidateSourceId::new("cargo");
pub const COMPOSER_SOURCE: CandidateSourceId = CandidateSourceId::new("composer");
pub const ARTISAN_SOURCE: CandidateSourceId = CandidateSourceId::new("artisan");
pub const GO_SOURCE: CandidateSourceId = CandidateSourceId::new("go");
pub const GRADLE_SOURCE: CandidateSourceId = CandidateSourceId::new("gradle");
pub const MAVEN_SOURCE: CandidateSourceId = CandidateSourceId::new("maven");
pub const DOTNET_SOURCE: CandidateSourceId = CandidateSourceId::new("dotnet");
pub const JAKE_SOURCE: CandidateSourceId = CandidateSourceId::new("jake");
pub const JUST_SOURCE: CandidateSourceId = CandidateSourceId::new("just");
pub const TASKFILE_SOURCE: CandidateSourceId = CandidateSourceId::new("taskfile");
pub const MISE_SOURCE: CandidateSourceId = CandidateSourceId::new("mise");
pub const SEMA_SOURCE: CandidateSourceId = CandidateSourceId::new("sema");
pub const PHP_FILE_SOURCE: CandidateSourceId = CandidateSourceId::new("php-file");
pub const ZIG_SOURCE: CandidateSourceId = CandidateSourceId::new("zig");
pub const SWIFT_SOURCE: CandidateSourceId = CandidateSourceId::new("swift");
pub const DART_SOURCE: CandidateSourceId = CandidateSourceId::new("dart");
pub const FLUTTER_SOURCE: CandidateSourceId = CandidateSourceId::new("flutter");
pub const PYTHON_FILE_SOURCE: CandidateSourceId = CandidateSourceId::new("python-file");
pub const SHELL_SOURCE: CandidateSourceId = CandidateSourceId::new("shell");
pub const MAKE_SOURCE: CandidateSourceId = CandidateSourceId::new("make");
pub const DOCKER_SOURCE: CandidateSourceId = CandidateSourceId::new("docker");

pub const NODE_TOOL: ToolId = ToolId::new("node");
pub const NPM_TOOL: ToolId = ToolId::new("npm");
pub const PNPM_TOOL: ToolId = ToolId::new("pnpm");
pub const YARN_TOOL: ToolId = ToolId::new("yarn");
pub const BUN_TOOL: ToolId = ToolId::new("bun");
pub const CARGO_TOOL: ToolId = ToolId::new("cargo");
pub const RUSTC_TOOL: ToolId = ToolId::new("rustc");
pub const COMPOSER_TOOL: ToolId = ToolId::new("composer");
pub const PHP_TOOL: ToolId = ToolId::new("php");
pub const GO_TOOL: ToolId = ToolId::new("go");
pub const GRADLE_TOOL: ToolId = ToolId::new("gradle");
pub const MAVEN_TOOL: ToolId = ToolId::new("mvn");
pub const DOTNET_TOOL: ToolId = ToolId::new("dotnet");
pub const JAKE_TOOL: ToolId = ToolId::new("jake");
pub const JUST_TOOL: ToolId = ToolId::new("just");
pub const TASK_TOOL: ToolId = ToolId::new("task");
pub const MISE_TOOL: ToolId = ToolId::new("mise");
pub const SEMA_TOOL: ToolId = ToolId::new("sema");
pub const ZIG_TOOL: ToolId = ToolId::new("zig");
pub const SWIFT_TOOL: ToolId = ToolId::new("swift");
pub const FLUTTER_TOOL: ToolId = ToolId::new("flutter");
pub const DART_TOOL: ToolId = ToolId::new("dart");
pub const PYTHON3_TOOL: ToolId = ToolId::new("python3");
pub const PYTHON_TOOL: ToolId = ToolId::new("python");
pub const MAKE_TOOL: ToolId = ToolId::new("make");
pub const DOCKER_TOOL: ToolId = ToolId::new("docker");

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub struct CandidateSourceRegistration {
    pub id: CandidateSourceId,
    pub metadata_priority: u8,
    pub default_tags: &'static [&'static str],
}

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Debug)]
pub enum MarkerPattern {
    Exact(&'static str),
    AsciiCaseInsensitiveBasename(&'static str),
    BasenamePrefixSuffix {
        prefix: &'static str,
        suffix: &'static str,
    },
    Extension(&'static str),
}

impl MarkerPattern {
    #[must_use]
    pub fn matches(self, path: &Path) -> bool {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return false;
        };
        match self {
            Self::Exact(expected) => name == expected,
            Self::AsciiCaseInsensitiveBasename(expected) => name.eq_ignore_ascii_case(expected),
            Self::BasenamePrefixSuffix { prefix, suffix } => {
                name.starts_with(prefix)
                    && name.ends_with(suffix)
                    && name.len() > prefix.len() + suffix.len()
            }
            Self::Extension(expected) => path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case(expected)),
        }
    }

    #[must_use]
    pub const fn exact_name(self) -> Option<&'static str> {
        match self {
            Self::Exact(name) => Some(name),
            Self::AsciiCaseInsensitiveBasename(_)
            | Self::BasenamePrefixSuffix { .. }
            | Self::Extension(_) => None,
        }
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Debug)]
pub enum RootRole {
    Package,
    Workspace,
    Classified,
    Auxiliary,
}

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Debug)]
pub struct ProjectMarker {
    pub pattern: MarkerPattern,
    pub root_role: RootRole,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum LocalMetadataProbe {
    FlutterSdk,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum CommandOutput {
    FirstNonEmptyLine,
    LinePrefix(&'static str),
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum DoctorProbe {
    Command {
        args: &'static [&'static str],
        timeout: Duration,
        output: CommandOutput,
    },
    LocalMetadata(LocalMetadataProbe),
    PresenceOnly {
        reason: &'static str,
    },
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub struct ToolRegistration {
    pub id: ToolId,
    pub program: &'static str,
    pub doctor: DoctorProbe,
}

pub struct DetectorRegistration {
    pub id: DetectorId,
    pub candidate_sources: &'static [CandidateSourceRegistration],
    pub synonyms: &'static [&'static str],
    pub markers: &'static [ProjectMarker],
    pub tools: &'static [ToolRegistration],
    pub conventional_roots: &'static [&'static str],
    pub cache_environment: &'static [&'static str],
    pub candidate_schema: u32,
    pub detector: &'static dyn Detector,
    pub workspace: Option<&'static dyn WorkspaceContributor>,
    pub target_binders: &'static [&'static dyn TargetBinder],
    pub target_runners: &'static [&'static dyn TargetRunner],
}

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub enum RootClassification {
    #[default]
    Neither,
    Package,
    Workspace,
    PackageAndWorkspace,
}

impl RootClassification {
    #[must_use]
    pub const fn is_package(self) -> bool {
        matches!(self, Self::Package | Self::PackageAndWorkspace)
    }

    #[must_use]
    pub const fn is_workspace(self) -> bool {
        matches!(self, Self::Workspace | Self::PackageAndWorkspace)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ScanContribution {
    pub includes: Vec<String>,
    pub excludes: Vec<String>,
}

pub trait WorkspaceContributor: Send + Sync {
    fn classify_root(&self, marker: &Path, files: &DiscoveryFiles) -> RootClassification {
        let _ = (marker, files);
        RootClassification::Neither
    }

    fn scan_contribution(&self, root: &Path, files: &DiscoveryFiles) -> ScanContribution;
}

impl fmt::Debug for DetectorRegistration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DetectorRegistration")
            .field("id", &self.id)
            .field("candidate_sources", &self.candidate_sources)
            .field("synonyms", &self.synonyms)
            .field("markers", &self.markers)
            .field("tools", &self.tools)
            .field("conventional_roots", &self.conventional_roots)
            .field("cache_environment", &self.cache_environment)
            .field("candidate_schema", &self.candidate_schema)
            .field("workspace", &self.workspace.is_some())
            .finish_non_exhaustive()
    }
}

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);

const fn command_tool(
    id: ToolId,
    program: &'static str,
    args: &'static [&'static str],
) -> ToolRegistration {
    ToolRegistration {
        id,
        program,
        doctor: DoctorProbe::Command {
            args,
            timeout: DEFAULT_TIMEOUT,
            output: CommandOutput::FirstNonEmptyLine,
        },
    }
}

const fn command_tool_with_output(
    id: ToolId,
    program: &'static str,
    args: &'static [&'static str],
    output: CommandOutput,
) -> ToolRegistration {
    ToolRegistration {
        id,
        program,
        doctor: DoctorProbe::Command {
            args,
            timeout: DEFAULT_TIMEOUT,
            output,
        },
    }
}

const NODE_PROGRAM: ToolRegistration = command_tool(NODE_TOOL, "node", &["--version"]);
const NPM_PROGRAM: ToolRegistration = command_tool(NPM_TOOL, "npm", &["--version"]);
const PNPM_PROGRAM: ToolRegistration = command_tool(PNPM_TOOL, "pnpm", &["--version"]);
const YARN_PROGRAM: ToolRegistration = command_tool(YARN_TOOL, "yarn", &["--version"]);
const BUN_PROGRAM: ToolRegistration = command_tool(BUN_TOOL, "bun", &["--version"]);
const CARGO_PROGRAM: ToolRegistration = command_tool(CARGO_TOOL, "cargo", &["--version"]);
const RUSTC_PROGRAM: ToolRegistration = command_tool(RUSTC_TOOL, "rustc", &["--version"]);
const COMPOSER_PROGRAM: ToolRegistration = command_tool(COMPOSER_TOOL, "composer", &["--version"]);
const PHP_PROGRAM: ToolRegistration = command_tool(PHP_TOOL, "php", &["--version"]);
const GO_PROGRAM: ToolRegistration = command_tool(GO_TOOL, "go", &["version"]);
const GRADLE_PROGRAM: ToolRegistration = command_tool_with_output(
    GRADLE_TOOL,
    "gradle",
    &["--version"],
    CommandOutput::LinePrefix("Gradle "),
);
const MAVEN_PROGRAM: ToolRegistration = command_tool(MAVEN_TOOL, "mvn", &["--version"]);
const DOTNET_PROGRAM: ToolRegistration = command_tool(DOTNET_TOOL, "dotnet", &["--version"]);
const JAKE_PROGRAM: ToolRegistration = command_tool(JAKE_TOOL, "jake", &["--version"]);
const JUST_PROGRAM: ToolRegistration = command_tool(JUST_TOOL, "just", &["--version"]);
const TASK_PROGRAM: ToolRegistration = command_tool(TASK_TOOL, "task", &["--version"]);
const MISE_PROGRAM: ToolRegistration = command_tool(MISE_TOOL, "mise", &["--version"]);
const SEMA_PROGRAM: ToolRegistration = command_tool(SEMA_TOOL, "sema", &["--version"]);
const ZIG_PROGRAM: ToolRegistration = command_tool(ZIG_TOOL, "zig", &["version"]);
const SWIFT_PROGRAM: ToolRegistration = command_tool(SWIFT_TOOL, "swift", &["--version"]);
const FLUTTER_PROGRAM: ToolRegistration = ToolRegistration {
    id: FLUTTER_TOOL,
    program: "flutter",
    doctor: DoctorProbe::LocalMetadata(LocalMetadataProbe::FlutterSdk),
};
const DART_PROGRAM: ToolRegistration = command_tool(DART_TOOL, "dart", &["--version"]);
const PYTHON3_PROGRAM: ToolRegistration = command_tool(PYTHON3_TOOL, "python3", &["--version"]);
const PYTHON_PROGRAM: ToolRegistration = command_tool(PYTHON_TOOL, "python", &["--version"]);
const MAKE_PROGRAM: ToolRegistration = command_tool(MAKE_TOOL, "make", &["--version"]);
const DOCKER_PROGRAM: ToolRegistration = command_tool(DOCKER_TOOL, "docker", &["--version"]);

const ROOTS: &[&str] = &[
    "bin", "cmd", "examples", "scripts", "spec", "test", "tests", "tools",
];

static REGISTRATIONS: &[DetectorRegistration] = &[
    DetectorRegistration {
        id: NODE,
        candidate_sources: &[
            CandidateSourceRegistration {
                id: NODE_SOURCE,
                metadata_priority: 2,
                default_tags: &["node", "javascript"],
            },
            CandidateSourceRegistration {
                id: VITE_SOURCE,
                metadata_priority: 3,
                default_tags: &["node", "vite"],
            },
            CandidateSourceRegistration {
                id: NEXT_SOURCE,
                metadata_priority: 3,
                default_tags: &["node", "next"],
            },
        ],
        synonyms: &[
            "javascript",
            "js",
            "typescript",
            "ts",
            "node",
            "npm",
            "pnpm",
            "yarn",
            "bun",
        ],
        markers: &[
            ProjectMarker {
                pattern: MarkerPattern::Exact("package.json"),
                root_role: RootRole::Classified,
            },
            ProjectMarker {
                pattern: MarkerPattern::Exact("pnpm-workspace.yaml"),
                root_role: RootRole::Workspace,
            },
            ProjectMarker {
                pattern: MarkerPattern::Exact("pnpm-lock.yaml"),
                root_role: RootRole::Auxiliary,
            },
            ProjectMarker {
                pattern: MarkerPattern::Exact("yarn.lock"),
                root_role: RootRole::Auxiliary,
            },
            ProjectMarker {
                pattern: MarkerPattern::Exact("package-lock.json"),
                root_role: RootRole::Auxiliary,
            },
            ProjectMarker {
                pattern: MarkerPattern::Exact("bun.lock"),
                root_role: RootRole::Auxiliary,
            },
            ProjectMarker {
                pattern: MarkerPattern::Exact("bun.lockb"),
                root_role: RootRole::Auxiliary,
            },
        ],
        tools: &[
            NODE_PROGRAM,
            NPM_PROGRAM,
            PNPM_PROGRAM,
            YARN_PROGRAM,
            BUN_PROGRAM,
        ],
        conventional_roots: ROOTS,
        cache_environment: &[],
        candidate_schema: 2,
        detector: &NodeDetector,
        workspace: Some(&NodeWorkspaceContributor),
        target_binders: &[&NodeTestBinder],
        target_runners: &[],
    },
    DetectorRegistration {
        id: CARGO,
        candidate_sources: &[CandidateSourceRegistration {
            id: CARGO_SOURCE,
            metadata_priority: 2,
            default_tags: &["cargo", "rust"],
        }],
        synonyms: &["rust", "rs", "cargo", "crate"],
        markers: &[
            ProjectMarker {
                pattern: MarkerPattern::Exact("Cargo.toml"),
                root_role: RootRole::Classified,
            },
            ProjectMarker {
                pattern: MarkerPattern::Exact("Cargo.lock"),
                root_role: RootRole::Auxiliary,
            },
        ],
        tools: &[CARGO_PROGRAM, RUSTC_PROGRAM],
        conventional_roots: ROOTS,
        cache_environment: &[],
        candidate_schema: 2,
        detector: &CargoDetector,
        workspace: Some(&CargoWorkspaceContributor),
        target_binders: &[],
        target_runners: &[],
    },
    DetectorRegistration {
        id: COMPOSER,
        candidate_sources: &[CandidateSourceRegistration {
            id: COMPOSER_SOURCE,
            metadata_priority: 2,
            default_tags: &["composer", "php"],
        }],
        synonyms: &["php", "composer"],
        markers: &[
            ProjectMarker {
                pattern: MarkerPattern::Exact("composer.json"),
                root_role: RootRole::Package,
            },
            ProjectMarker {
                pattern: MarkerPattern::Exact("composer.lock"),
                root_role: RootRole::Auxiliary,
            },
        ],
        tools: &[COMPOSER_PROGRAM, PHP_PROGRAM],
        conventional_roots: ROOTS,
        cache_environment: &[],
        candidate_schema: 1,
        detector: &ComposerDetector,
        workspace: None,
        target_binders: &[],
        target_runners: &[],
    },
    DetectorRegistration {
        id: ARTISAN,
        candidate_sources: &[CandidateSourceRegistration {
            id: ARTISAN_SOURCE,
            metadata_priority: 3,
            default_tags: &["artisan", "laravel", "php"],
        }],
        synonyms: &["laravel", "artisan", "php"],
        markers: &[ProjectMarker {
            pattern: MarkerPattern::Exact("artisan"),
            root_role: RootRole::Package,
        }],
        tools: &[PHP_PROGRAM],
        conventional_roots: ROOTS,
        cache_environment: &[],
        candidate_schema: 1,
        detector: &ArtisanDetector,
        workspace: None,
        target_binders: &[],
        target_runners: &[],
    },
    DetectorRegistration {
        id: GO,
        candidate_sources: &[CandidateSourceRegistration {
            id: GO_SOURCE,
            metadata_priority: 2,
            default_tags: &["go", "golang"],
        }],
        synonyms: &["go", "golang", "module"],
        markers: &[
            ProjectMarker {
                pattern: MarkerPattern::Exact("go.mod"),
                root_role: RootRole::Package,
            },
            ProjectMarker {
                pattern: MarkerPattern::Exact("go.work"),
                root_role: RootRole::Workspace,
            },
            ProjectMarker {
                pattern: MarkerPattern::Exact("go.sum"),
                root_role: RootRole::Auxiliary,
            },
        ],
        tools: &[GO_PROGRAM],
        conventional_roots: ROOTS,
        cache_environment: &[],
        candidate_schema: 1,
        detector: &GoDetector,
        workspace: Some(&GoWorkspaceContributor),
        target_binders: &[],
        target_runners: &[],
    },
    DetectorRegistration {
        id: GRADLE,
        candidate_sources: &[CandidateSourceRegistration {
            id: GRADLE_SOURCE,
            metadata_priority: 2,
            default_tags: &["gradle", "jvm", "java", "kotlin"],
        }],
        synonyms: &["gradle", "jvm", "java", "kotlin", "groovy"],
        markers: &[
            ProjectMarker {
                pattern: MarkerPattern::Exact("settings.gradle"),
                root_role: RootRole::Workspace,
            },
            ProjectMarker {
                pattern: MarkerPattern::Exact("settings.gradle.kts"),
                root_role: RootRole::Workspace,
            },
            ProjectMarker {
                pattern: MarkerPattern::Exact("build.gradle"),
                root_role: RootRole::Package,
            },
            ProjectMarker {
                pattern: MarkerPattern::Exact("build.gradle.kts"),
                root_role: RootRole::Package,
            },
        ],
        tools: &[GRADLE_PROGRAM],
        conventional_roots: ROOTS,
        cache_environment: &[
            "GRADLE_OPTS",
            "GRADLE_USER_HOME",
            "HOME",
            "JAVA_OPTS",
            "JAVA_TOOL_OPTIONS",
            "_JAVA_OPTIONS",
        ],
        candidate_schema: 2,
        detector: &GradleDetector,
        workspace: Some(&GradleWorkspaceContributor),
        target_binders: &[],
        target_runners: &[],
    },
    DetectorRegistration {
        id: MAVEN,
        candidate_sources: &[CandidateSourceRegistration {
            id: MAVEN_SOURCE,
            metadata_priority: 2,
            default_tags: &["maven", "jvm", "java"],
        }],
        synonyms: &["maven", "mvn", "jvm", "java"],
        markers: &[ProjectMarker {
            pattern: MarkerPattern::Exact("pom.xml"),
            root_role: RootRole::Classified,
        }],
        tools: &[MAVEN_PROGRAM],
        conventional_roots: ROOTS,
        cache_environment: &[
            "HOME",
            "MAVEN_USER_HOME",
            "MAVEN_OPTS",
            "MAVEN_WRAPPER_ALWAYS_DOWNLOAD",
            "MAVEN_WRAPPER_ALWAYS_UNPACK",
            "JAVA_OPTS",
            "JAVA_TOOL_OPTIONS",
            "_JAVA_OPTIONS",
        ],
        candidate_schema: 2,
        detector: &MavenDetector,
        workspace: Some(&MavenWorkspaceContributor),
        target_binders: &[],
        target_runners: &[],
    },
    DetectorRegistration {
        id: DOTNET,
        candidate_sources: &[CandidateSourceRegistration {
            id: DOTNET_SOURCE,
            metadata_priority: 2,
            default_tags: &["dotnet", "csharp", "fsharp", "visual-basic"],
        }],
        synonyms: &["dotnet", ".net", "csharp", "c#", "fsharp", "f#", "msbuild"],
        markers: &[
            ProjectMarker {
                pattern: MarkerPattern::Extension("sln"),
                root_role: RootRole::Workspace,
            },
            ProjectMarker {
                pattern: MarkerPattern::Extension("slnx"),
                root_role: RootRole::Workspace,
            },
            ProjectMarker {
                pattern: MarkerPattern::Extension("slnf"),
                root_role: RootRole::Workspace,
            },
            ProjectMarker {
                pattern: MarkerPattern::Extension("csproj"),
                root_role: RootRole::Package,
            },
            ProjectMarker {
                pattern: MarkerPattern::Extension("fsproj"),
                root_role: RootRole::Package,
            },
            ProjectMarker {
                pattern: MarkerPattern::Extension("vbproj"),
                root_role: RootRole::Package,
            },
        ],
        tools: &[DOTNET_PROGRAM],
        conventional_roots: ROOTS,
        cache_environment: &[],
        candidate_schema: 1,
        detector: &DotnetDetector,
        workspace: Some(&DotnetWorkspaceContributor),
        target_binders: &[],
        target_runners: &[],
    },
    DetectorRegistration {
        id: PHP_FILE,
        candidate_sources: &[CandidateSourceRegistration {
            id: PHP_FILE_SOURCE,
            metadata_priority: 1,
            default_tags: &["php"],
        }],
        synonyms: &["php", "script"],
        markers: &[],
        tools: &[PHP_PROGRAM],
        conventional_roots: ROOTS,
        cache_environment: &[],
        candidate_schema: 1,
        detector: &PhpFileDetector,
        workspace: None,
        target_binders: &[],
        target_runners: &[&PhpFileDetector],
    },
    DetectorRegistration {
        id: ZIG,
        candidate_sources: &[CandidateSourceRegistration {
            id: ZIG_SOURCE,
            metadata_priority: 2,
            default_tags: &["zig"],
        }],
        synonyms: &["zig"],
        markers: &[
            ProjectMarker {
                pattern: MarkerPattern::Exact("build.zig"),
                root_role: RootRole::Package,
            },
            ProjectMarker {
                pattern: MarkerPattern::Exact("build.zig.zon"),
                root_role: RootRole::Auxiliary,
            },
        ],
        tools: &[ZIG_PROGRAM],
        conventional_roots: ROOTS,
        cache_environment: &[],
        candidate_schema: 1,
        detector: &ZigDetector,
        workspace: None,
        target_binders: &[],
        target_runners: &[&ZigDetector],
    },
    DetectorRegistration {
        id: SWIFT,
        candidate_sources: &[CandidateSourceRegistration {
            id: SWIFT_SOURCE,
            metadata_priority: 2,
            default_tags: &["swift", "swiftpm"],
        }],
        synonyms: &["swift", "swiftpm", "spm"],
        markers: &[
            ProjectMarker {
                pattern: MarkerPattern::Exact("Package.swift"),
                root_role: RootRole::Package,
            },
            ProjectMarker {
                pattern: MarkerPattern::Exact("Package.resolved"),
                root_role: RootRole::Auxiliary,
            },
        ],
        tools: &[SWIFT_PROGRAM],
        conventional_roots: ROOTS,
        cache_environment: &[],
        candidate_schema: 1,
        detector: &SwiftDetector,
        workspace: None,
        target_binders: &[],
        target_runners: &[],
    },
    DetectorRegistration {
        id: DART,
        candidate_sources: &[
            CandidateSourceRegistration {
                id: DART_SOURCE,
                metadata_priority: 2,
                default_tags: &["dart", "pub"],
            },
            CandidateSourceRegistration {
                id: FLUTTER_SOURCE,
                metadata_priority: 2,
                default_tags: &["dart", "flutter"],
            },
        ],
        synonyms: &["dart", "flutter", "pub"],
        markers: &[
            ProjectMarker {
                pattern: MarkerPattern::Exact("pubspec.yaml"),
                root_role: RootRole::Package,
            },
            ProjectMarker {
                pattern: MarkerPattern::Exact("pubspec.lock"),
                root_role: RootRole::Auxiliary,
            },
        ],
        tools: &[DART_PROGRAM, FLUTTER_PROGRAM],
        conventional_roots: ROOTS,
        cache_environment: &[],
        candidate_schema: 1,
        detector: &DartDetector,
        workspace: None,
        target_binders: &[],
        target_runners: &[&DartDetector],
    },
    DetectorRegistration {
        id: PYTHON_FILE,
        candidate_sources: &[CandidateSourceRegistration {
            id: PYTHON_FILE_SOURCE,
            metadata_priority: 1,
            default_tags: &["python"],
        }],
        synonyms: &["python", "py", "script"],
        markers: &[],
        tools: &[PYTHON3_PROGRAM, PYTHON_PROGRAM],
        conventional_roots: ROOTS,
        cache_environment: &["VIRTUAL_ENV"],
        candidate_schema: 1,
        detector: &PythonFileDetector,
        workspace: None,
        target_binders: &[],
        target_runners: &[&PythonFileDetector],
    },
    DetectorRegistration {
        id: JUST,
        candidate_sources: &[CandidateSourceRegistration {
            id: JUST_SOURCE,
            metadata_priority: 3,
            default_tags: &["just", "justfile", "task"],
        }],
        synonyms: &["just", "justfile", "recipe", "task"],
        markers: &[
            ProjectMarker {
                pattern: MarkerPattern::Exact(".justfile"),
                root_role: RootRole::Package,
            },
            ProjectMarker {
                pattern: MarkerPattern::AsciiCaseInsensitiveBasename("justfile"),
                root_role: RootRole::Package,
            },
        ],
        tools: &[JUST_PROGRAM],
        conventional_roots: ROOTS,
        cache_environment: &[],
        candidate_schema: 2,
        detector: &JustDetector,
        workspace: None,
        target_binders: &[],
        target_runners: &[],
    },
    DetectorRegistration {
        id: JAKE,
        candidate_sources: &[CandidateSourceRegistration {
            id: JAKE_SOURCE,
            metadata_priority: 3,
            default_tags: &["jake", "jakefile", "task"],
        }],
        synonyms: &["jake", "jakefile", "recipe", "task"],
        markers: &[ProjectMarker {
            pattern: MarkerPattern::Exact("Jakefile"),
            root_role: RootRole::Package,
        }],
        tools: &[JAKE_PROGRAM],
        conventional_roots: ROOTS,
        cache_environment: &[],
        candidate_schema: 2,
        detector: &JakeDetector,
        workspace: None,
        target_binders: &[],
        target_runners: &[],
    },
    DetectorRegistration {
        id: TASKFILE,
        candidate_sources: &[CandidateSourceRegistration {
            id: TASKFILE_SOURCE,
            metadata_priority: 3,
            default_tags: &["task", "taskfile", "go-task"],
        }],
        synonyms: &["task", "taskfile", "go-task", "recipe"],
        markers: &[
            ProjectMarker {
                pattern: MarkerPattern::Exact("Taskfile.yml"),
                root_role: RootRole::Package,
            },
            ProjectMarker {
                pattern: MarkerPattern::Exact("Taskfile.yaml"),
                root_role: RootRole::Package,
            },
            ProjectMarker {
                pattern: MarkerPattern::Exact("taskfile.yml"),
                root_role: RootRole::Package,
            },
            ProjectMarker {
                pattern: MarkerPattern::Exact("taskfile.yaml"),
                root_role: RootRole::Package,
            },
            ProjectMarker {
                pattern: MarkerPattern::Exact("Taskfile.dist.yml"),
                root_role: RootRole::Package,
            },
            ProjectMarker {
                pattern: MarkerPattern::Exact("Taskfile.dist.yaml"),
                root_role: RootRole::Package,
            },
            ProjectMarker {
                pattern: MarkerPattern::Exact("taskfile.dist.yml"),
                root_role: RootRole::Package,
            },
            ProjectMarker {
                pattern: MarkerPattern::Exact("taskfile.dist.yaml"),
                root_role: RootRole::Package,
            },
        ],
        tools: &[TASK_PROGRAM],
        conventional_roots: ROOTS,
        cache_environment: &[],
        candidate_schema: 2,
        detector: &TaskfileDetector,
        workspace: None,
        target_binders: &[],
        target_runners: &[],
    },
    DetectorRegistration {
        id: MISE,
        candidate_sources: &[CandidateSourceRegistration {
            id: MISE_SOURCE,
            metadata_priority: 3,
            default_tags: &["mise", "task"],
        }],
        synonyms: &["mise", "mise-en-place", "task", "recipe"],
        markers: &[
            ProjectMarker {
                pattern: MarkerPattern::Exact("mise.toml"),
                root_role: RootRole::Package,
            },
            ProjectMarker {
                pattern: MarkerPattern::Exact(".mise.toml"),
                root_role: RootRole::Package,
            },
            ProjectMarker {
                pattern: MarkerPattern::BasenamePrefixSuffix {
                    prefix: "mise.",
                    suffix: ".toml",
                },
                root_role: RootRole::Package,
            },
            ProjectMarker {
                pattern: MarkerPattern::BasenamePrefixSuffix {
                    prefix: ".mise.",
                    suffix: ".toml",
                },
                root_role: RootRole::Package,
            },
        ],
        tools: &[MISE_PROGRAM],
        conventional_roots: ROOTS,
        cache_environment: &["MISE_ENV"],
        candidate_schema: 2,
        detector: &MiseDetector,
        workspace: None,
        target_binders: &[],
        target_runners: &[],
    },
    DetectorRegistration {
        id: SEMA,
        candidate_sources: &[CandidateSourceRegistration {
            id: SEMA_SOURCE,
            metadata_priority: 2,
            default_tags: &["sema", "lisp"],
        }],
        synonyms: &["sema", "sema-lang", "lisp", "sexpr"],
        markers: &[
            ProjectMarker {
                pattern: MarkerPattern::Exact("sema.toml"),
                root_role: RootRole::Package,
            },
            ProjectMarker {
                pattern: MarkerPattern::Exact("sema.lock"),
                root_role: RootRole::Auxiliary,
            },
            ProjectMarker {
                pattern: MarkerPattern::Extension("sema"),
                root_role: RootRole::Package,
            },
        ],
        tools: &[SEMA_PROGRAM],
        conventional_roots: ROOTS,
        cache_environment: &[],
        candidate_schema: 2,
        detector: &SemaDetector,
        workspace: None,
        target_binders: &[],
        target_runners: &[],
    },
    DetectorRegistration {
        id: SHELL,
        candidate_sources: &[CandidateSourceRegistration {
            id: SHELL_SOURCE,
            metadata_priority: 1,
            default_tags: &["shell", "script"],
        }],
        synonyms: &["shell", "sh", "bash", "script", "executable"],
        markers: &[],
        tools: &[],
        conventional_roots: ROOTS,
        cache_environment: &[],
        candidate_schema: 1,
        detector: &ShellDetector,
        workspace: None,
        target_binders: &[],
        target_runners: &[&ShellDetector],
    },
    DetectorRegistration {
        id: MAKE,
        candidate_sources: &[CandidateSourceRegistration {
            id: MAKE_SOURCE,
            metadata_priority: 1,
            default_tags: &["make", "makefile"],
        }],
        synonyms: &["make", "makefile"],
        markers: &[
            ProjectMarker {
                pattern: MarkerPattern::Exact("GNUmakefile"),
                root_role: RootRole::Package,
            },
            ProjectMarker {
                pattern: MarkerPattern::Exact("makefile"),
                root_role: RootRole::Package,
            },
            ProjectMarker {
                pattern: MarkerPattern::Exact("Makefile"),
                root_role: RootRole::Package,
            },
        ],
        tools: &[MAKE_PROGRAM],
        conventional_roots: ROOTS,
        cache_environment: &[],
        candidate_schema: 1,
        detector: &MakeDetector,
        workspace: None,
        target_binders: &[],
        target_runners: &[],
    },
    DetectorRegistration {
        id: DOCKER,
        candidate_sources: &[CandidateSourceRegistration {
            id: DOCKER_SOURCE,
            metadata_priority: 1,
            default_tags: &["docker", "compose", "container"],
        }],
        synonyms: &["docker", "compose", "container"],
        markers: &[
            ProjectMarker {
                pattern: MarkerPattern::Exact("compose.yaml"),
                root_role: RootRole::Package,
            },
            ProjectMarker {
                pattern: MarkerPattern::Exact("compose.yml"),
                root_role: RootRole::Package,
            },
            ProjectMarker {
                pattern: MarkerPattern::Exact("docker-compose.yaml"),
                root_role: RootRole::Package,
            },
            ProjectMarker {
                pattern: MarkerPattern::Exact("docker-compose.yml"),
                root_role: RootRole::Package,
            },
            ProjectMarker {
                pattern: MarkerPattern::Exact("Dockerfile"),
                root_role: RootRole::Package,
            },
        ],
        tools: &[DOCKER_PROGRAM],
        conventional_roots: ROOTS,
        cache_environment: &[],
        candidate_schema: 1,
        detector: &DockerDetector,
        workspace: None,
        target_binders: &[],
        target_runners: &[],
    },
];

#[must_use]
pub fn registrations() -> &'static [DetectorRegistration] {
    REGISTRATIONS
}

#[must_use]
pub fn registration(id: DetectorId) -> Option<&'static DetectorRegistration> {
    registrations()
        .iter()
        .find(|registration| registration.id == id)
}

#[must_use]
pub fn workspace(id: DetectorId) -> Option<&'static dyn WorkspaceContributor> {
    registration(id).and_then(|registration| registration.workspace)
}

#[must_use]
pub fn workspace_contains_manifest(
    id: DetectorId,
    root: &Path,
    relative: &Path,
    files: &DiscoveryFiles,
) -> bool {
    let Some(workspace) = workspace(id) else {
        return false;
    };
    let contribution = workspace.scan_contribution(root, files);
    let Ok(includes) = compile_workspace_globs(&contribution.includes) else {
        return false;
    };
    let excludes = compile_workspace_globs(&contribution.excludes).ok();
    includes.is_match(relative)
        && !excludes
            .as_ref()
            .is_some_and(|patterns| patterns.is_match(relative))
}

fn compile_workspace_globs(patterns: &[String]) -> Result<globset::GlobSet, globset::Error> {
    let mut builder = globset::GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(globset::Glob::new(pattern)?);
    }
    builder.build()
}

#[must_use]
pub fn synonyms(id: DetectorId) -> &'static [&'static str] {
    registration(id).map_or(&[], |registration| registration.synonyms)
}

#[must_use]
pub fn source(
    id: CandidateSourceId,
) -> Option<(
    &'static DetectorRegistration,
    &'static CandidateSourceRegistration,
)> {
    registrations().iter().find_map(|registration| {
        registration
            .candidate_sources
            .iter()
            .find(|source| source.id == id)
            .map(|source| (registration, source))
    })
}

#[must_use]
pub fn source_by_name(
    name: &str,
) -> Option<(
    &'static DetectorRegistration,
    &'static CandidateSourceRegistration,
)> {
    registrations().iter().find_map(|registration| {
        registration
            .candidate_sources
            .iter()
            .find(|source| source.id.as_str() == name)
            .map(|source| (registration, source))
    })
}

#[must_use]
pub fn tools() -> &'static [ToolRegistration] {
    static TOOLS: OnceLock<Vec<ToolRegistration>> = OnceLock::new();
    TOOLS
        .get_or_init(|| {
            let mut tools = BTreeMap::new();
            for tool in registrations()
                .iter()
                .flat_map(|registration| registration.tools)
            {
                tools.entry(tool.id).or_insert(*tool);
            }
            tools.into_values().collect()
        })
        .as_slice()
}

#[must_use]
pub fn markers() -> &'static [ProjectMarker] {
    static MARKERS: OnceLock<Vec<ProjectMarker>> = OnceLock::new();
    MARKERS
        .get_or_init(|| {
            registrations()
                .iter()
                .flat_map(|registration| registration.markers)
                .copied()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect()
        })
        .as_slice()
}

#[must_use]
pub fn conventional_roots() -> &'static [&'static str] {
    static ROOTS: OnceLock<Vec<&'static str>> = OnceLock::new();
    ROOTS
        .get_or_init(|| {
            registrations()
                .iter()
                .flat_map(|registration| registration.conventional_roots)
                .copied()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect()
        })
        .as_slice()
}

#[must_use]
pub fn cache_environment() -> &'static [&'static str] {
    static KEYS: OnceLock<Vec<&'static str>> = OnceLock::new();
    KEYS.get_or_init(|| {
        registrations()
            .iter()
            .flat_map(|registration| registration.cache_environment)
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    })
    .as_slice()
}

#[must_use]
pub fn fingerprint() -> &'static str {
    static FINGERPRINT: OnceLock<String> = OnceLock::new();
    FINGERPRINT.get_or_init(|| fingerprint_for(registrations().iter()))
}

fn fingerprint_for<'a>(
    registrations: impl IntoIterator<Item = &'a DetectorRegistration>,
) -> String {
    let mut registrations = registrations.into_iter().collect::<Vec<_>>();
    registrations.sort_by_key(|registration| registration.id);
    let mut hasher = blake3::Hasher::new();
    for registration in registrations {
        hash_text(&mut hasher, registration.id.as_str());
        hasher.update(&registration.candidate_schema.to_le_bytes());

        let mut sources = registration.candidate_sources.iter().collect::<Vec<_>>();
        sources.sort_by_key(|source| source.id);
        for source in sources {
            hash_text(&mut hasher, source.id.as_str());
            hasher.update(&[source.metadata_priority]);
            for tag in source.default_tags.iter().copied().collect::<BTreeSet<_>>() {
                hash_text(&mut hasher, tag);
            }
        }
        for synonym in registration
            .synonyms
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
        {
            hash_text(&mut hasher, synonym);
        }
        for marker in registration
            .markers
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
        {
            hash_text(&mut hasher, &format!("{marker:?}"));
        }
        let mut tools = registration.tools.iter().collect::<Vec<_>>();
        tools.sort_by_key(|tool| tool.id);
        for tool in tools {
            hash_text(&mut hasher, tool.id.as_str());
            hash_text(&mut hasher, tool.program);
            hash_text(&mut hasher, &format!("{:?}", tool.doctor));
        }
        for root in registration
            .conventional_roots
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
        {
            hash_text(&mut hasher, root);
        }
        for key in registration
            .cache_environment
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
        {
            hash_text(&mut hasher, key);
        }
        hasher.update(&[u8::from(registration.workspace.is_some())]);
    }
    hasher.finalize().to_hex().to_string()
}

fn hash_text(hasher: &mut blake3::Hasher, value: &str) {
    hasher.update(&(value.len() as u64).to_le_bytes());
    hasher.update(value.as_bytes());
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("duplicate detector id `{0}`")]
    DuplicateDetector(DetectorId),
    #[error("duplicate candidate source id `{0}`")]
    DuplicateSource(CandidateSourceId),
    #[error("conflicting tool registration `{0}`")]
    ConflictingTool(ToolId),
    #[error("detector `{0}` has candidate schema zero")]
    ZeroCandidateSchema(DetectorId),
    #[error("detector `{detector}` has invalid exact marker `{marker}`")]
    InvalidExactMarker {
        detector: DetectorId,
        marker: &'static str,
    },
    #[error("detector `{detector}` has invalid cache environment key `{key}`")]
    InvalidCacheEnvironment {
        detector: DetectorId,
        key: &'static str,
    },
    #[error("detector `{0}` has a classified marker but no workspace contributor")]
    ClassifiedMarkerWithoutWorkspace(DetectorId),
}

pub fn validate() -> Result<(), RegistryError> {
    let mut detector_ids = BTreeSet::new();
    let mut source_ids = BTreeSet::new();
    let mut tools = BTreeMap::<ToolId, ToolRegistration>::new();
    for registration in registrations() {
        if !detector_ids.insert(registration.id) {
            return Err(RegistryError::DuplicateDetector(registration.id));
        }
        if registration.candidate_schema == 0 {
            return Err(RegistryError::ZeroCandidateSchema(registration.id));
        }
        if registration.workspace.is_none()
            && registration
                .markers
                .iter()
                .any(|marker| marker.root_role == RootRole::Classified)
        {
            return Err(RegistryError::ClassifiedMarkerWithoutWorkspace(
                registration.id,
            ));
        }
        for marker in registration.markers {
            if let MarkerPattern::Exact(name) = marker.pattern {
                if name.is_empty() || name.contains(['/', '\\']) {
                    return Err(RegistryError::InvalidExactMarker {
                        detector: registration.id,
                        marker: name,
                    });
                }
            }
        }
        for key in registration.cache_environment {
            if key.is_empty() || key.contains(['=', '\0']) {
                return Err(RegistryError::InvalidCacheEnvironment {
                    detector: registration.id,
                    key,
                });
            }
        }
        for source in registration.candidate_sources {
            if !source_ids.insert(source.id) {
                return Err(RegistryError::DuplicateSource(source.id));
            }
        }
        for tool in registration.tools {
            if let Some(existing) = tools.insert(tool.id, *tool) {
                if existing != *tool {
                    return Err(RegistryError::ConflictingTool(tool.id));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_is_internally_consistent() -> anyhow::Result<()> {
        validate()?;
        assert_eq!(tools().len(), 26);
        assert_eq!(
            registrations()
                .iter()
                .filter(|registration| registration.workspace.is_some())
                .count(),
            6
        );
        assert!(source_by_name("vite").is_some());
        assert!(source_by_name("unknown").is_none());
        assert_eq!(
            cache_environment(),
            [
                "GRADLE_OPTS",
                "GRADLE_USER_HOME",
                "HOME",
                "JAVA_OPTS",
                "JAVA_TOOL_OPTIONS",
                "MAVEN_OPTS",
                "MAVEN_USER_HOME",
                "MAVEN_WRAPPER_ALWAYS_DOWNLOAD",
                "MAVEN_WRAPPER_ALWAYS_UNPACK",
                "MISE_ENV",
                "VIRTUAL_ENV",
                "_JAVA_OPTIONS"
            ]
        );
        Ok(())
    }

    #[test]
    fn fingerprint_is_stable_and_nonempty() {
        assert_eq!(fingerprint(), fingerprint());
        assert_eq!(fingerprint().len(), 64);
        assert_eq!(
            fingerprint_for(registrations().iter()),
            fingerprint_for(registrations().iter().rev())
        );
    }
}
