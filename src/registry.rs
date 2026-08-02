use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use serde::Serialize;

use crate::detect::{
    ArtisanDetector, CargoDetector, ComposerDetector, DartDetector, Detector, DockerDetector,
    GoDetector, MakeDetector, NodeDetector, NodeTestBinder, PhpFileDetector, PythonFileDetector,
    ShellDetector, SwiftDetector, TargetBinder, TargetRunner, ZigDetector,
};

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
            Self::AsciiCaseInsensitiveBasename(_) | Self::Extension(_) => None,
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
pub enum DoctorProbe {
    Command {
        args: &'static [&'static str],
        timeout: Duration,
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
    pub candidate_schema: u32,
    pub detector: &'static dyn Detector,
    pub target_binders: &'static [&'static dyn TargetBinder],
    pub target_runners: &'static [&'static dyn TargetRunner],
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
            .field("candidate_schema", &self.candidate_schema)
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
        candidate_schema: 1,
        detector: &NodeDetector,
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
        candidate_schema: 1,
        detector: &CargoDetector,
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
        candidate_schema: 1,
        detector: &ComposerDetector,
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
        candidate_schema: 1,
        detector: &ArtisanDetector,
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
        candidate_schema: 1,
        detector: &GoDetector,
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
        candidate_schema: 1,
        detector: &PhpFileDetector,
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
        candidate_schema: 1,
        detector: &ZigDetector,
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
        candidate_schema: 1,
        detector: &SwiftDetector,
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
        candidate_schema: 1,
        detector: &DartDetector,
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
        candidate_schema: 1,
        detector: &PythonFileDetector,
        target_binders: &[],
        target_runners: &[&PythonFileDetector],
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
        candidate_schema: 1,
        detector: &ShellDetector,
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
        candidate_schema: 1,
        detector: &MakeDetector,
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
        candidate_schema: 1,
        detector: &DockerDetector,
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
pub fn fingerprint() -> &'static str {
    static FINGERPRINT: OnceLock<String> = OnceLock::new();
    FINGERPRINT.get_or_init(|| {
        let mut hasher = blake3::Hasher::new();
        for registration in registrations() {
            hash_text(&mut hasher, registration.id.as_str());
            hasher.update(&registration.candidate_schema.to_le_bytes());
            for source in registration.candidate_sources {
                hash_text(&mut hasher, source.id.as_str());
                hasher.update(&[source.metadata_priority]);
                for tag in source.default_tags {
                    hash_text(&mut hasher, tag);
                }
            }
            for synonym in registration.synonyms {
                hash_text(&mut hasher, synonym);
            }
            for marker in registration.markers {
                hash_text(&mut hasher, &format!("{marker:?}"));
            }
            for tool in registration.tools {
                hash_text(&mut hasher, tool.id.as_str());
                hash_text(&mut hasher, tool.program);
            }
            for root in registration.conventional_roots {
                hash_text(&mut hasher, root);
            }
        }
        hasher.finalize().to_hex().to_string()
    })
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
        assert_eq!(tools().len(), 18);
        assert!(source_by_name("vite").is_some());
        assert!(source_by_name("unknown").is_none());
        Ok(())
    }

    #[test]
    fn fingerprint_is_stable_and_nonempty() {
        assert_eq!(fingerprint(), fingerprint());
        assert_eq!(fingerprint().len(), 64);
    }
}
