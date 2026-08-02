use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::PathBuf;

use crate::candidate::{
    Availability, Candidate, CandidateOrigin, CommandLayer, Evidence, Lifecycle, PassthroughStyle,
    Points, SearchDocument, SelectionPolicy,
};
use crate::diagnostic::Diagnostic;
use crate::intent::Intent;
use crate::registry::{CandidateSourceId, DetectorId, ToolId};

use super::Detection;

#[derive(Clone, Debug)]
enum Program {
    Tool(ToolId),
    Path(OsString),
}

#[derive(Debug, thiserror::Error)]
pub enum CandidateBuildError {
    #[error("candidate source `{0}` is not registered")]
    UnknownSource(CandidateSourceId),
    #[error("tool `{tool}` does not belong to detector `{detector}`")]
    ForeignTool { detector: DetectorId, tool: ToolId },
    #[error("candidate is missing required field `{0}`")]
    Missing(&'static str),
}

#[derive(Clone, Debug)]
pub struct CandidateBuilder {
    source: CandidateSourceId,
    intent: Intent,
    action_name: String,
    scope_root: PathBuf,
    layer: CommandLayer,
    action_key: Option<String>,
    program: Option<Program>,
    args: Vec<OsString>,
    cwd: Option<PathBuf>,
    env: BTreeMap<OsString, OsString>,
    passthrough: PassthroughStyle,
    lifecycle: Lifecycle,
    origin: CandidateOrigin,
    selection: Option<SelectionPolicy>,
    availability: Option<Availability>,
    base_points: Option<Points>,
    evidence: Vec<Evidence>,
    search: Option<SearchDocument>,
    label: Option<String>,
    description: Option<String>,
}

impl CandidateBuilder {
    #[must_use]
    pub fn project_facade(
        source: CandidateSourceId,
        intent: Intent,
        scope_root: PathBuf,
        action_name: impl Into<String>,
    ) -> Self {
        Self::new(
            source,
            intent,
            scope_root,
            action_name,
            CommandLayer::ProjectFacade,
        )
    }

    #[must_use]
    pub fn ecosystem_task(
        source: CandidateSourceId,
        intent: Intent,
        scope_root: PathBuf,
        action_name: impl Into<String>,
    ) -> Self {
        Self::new(
            source,
            intent,
            scope_root,
            action_name,
            CommandLayer::EcosystemTask,
        )
    }

    #[must_use]
    pub fn tool_default(
        source: CandidateSourceId,
        intent: Intent,
        scope_root: PathBuf,
        action_name: impl Into<String>,
    ) -> Self {
        Self::new(
            source,
            intent,
            scope_root,
            action_name,
            CommandLayer::ToolDefault,
        )
    }

    #[must_use]
    pub fn direct_target(
        source: CandidateSourceId,
        intent: Intent,
        scope_root: PathBuf,
        action_name: impl Into<String>,
    ) -> Self {
        Self::new(
            source,
            intent,
            scope_root,
            action_name,
            CommandLayer::DirectTarget,
        )
    }

    fn new(
        source: CandidateSourceId,
        intent: Intent,
        scope_root: PathBuf,
        action_name: impl Into<String>,
        layer: CommandLayer,
    ) -> Self {
        Self {
            source,
            intent,
            action_name: action_name.into(),
            scope_root,
            layer,
            action_key: None,
            program: None,
            args: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            passthrough: PassthroughStyle::Append,
            lifecycle: Lifecycle::Finite,
            origin: CandidateOrigin::Declared,
            selection: None,
            availability: None,
            base_points: None,
            evidence: Vec::new(),
            search: None,
            label: None,
            description: None,
        }
    }

    #[must_use]
    pub fn action_key(mut self, action_key: impl Into<String>) -> Self {
        self.action_key = Some(action_key.into());
        self
    }

    #[must_use]
    pub fn tool(mut self, tool: ToolId) -> Self {
        self.program = Some(Program::Tool(tool));
        self
    }

    #[must_use]
    pub fn program_path(mut self, program: impl Into<OsString>) -> Self {
        self.program = Some(Program::Path(program.into()));
        self
    }

    #[must_use]
    pub fn args<I, T>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString>,
    {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    #[must_use]
    pub fn cwd(mut self, cwd: PathBuf) -> Self {
        self.cwd = Some(cwd);
        self
    }

    #[must_use]
    pub fn env(mut self, env: BTreeMap<OsString, OsString>) -> Self {
        self.env = env;
        self
    }

    #[must_use]
    pub fn passthrough(mut self, passthrough: PassthroughStyle) -> Self {
        self.passthrough = passthrough;
        self
    }

    #[must_use]
    pub fn lifecycle(mut self, lifecycle: Lifecycle) -> Self {
        self.lifecycle = lifecycle;
        self
    }

    #[must_use]
    pub fn origin(mut self, origin: CandidateOrigin) -> Self {
        self.origin = origin;
        self
    }

    #[must_use]
    pub fn selection(mut self, selection: SelectionPolicy) -> Self {
        self.selection = Some(selection);
        self
    }

    #[must_use]
    pub fn availability(mut self, availability: Availability) -> Self {
        self.availability = Some(availability);
        self
    }

    #[must_use]
    pub fn base_points(mut self, points: Points) -> Self {
        self.base_points = Some(points);
        self
    }

    #[must_use]
    pub fn evidence(mut self, evidence: Evidence) -> Self {
        self.evidence.push(evidence);
        self
    }

    #[must_use]
    pub fn evidence_all(mut self, evidence: impl IntoIterator<Item = Evidence>) -> Self {
        self.evidence.extend(evidence);
        self
    }

    #[must_use]
    pub fn search(mut self, search: SearchDocument) -> Self {
        self.search = Some(search);
        self
    }

    #[must_use]
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn emit(self, output: &mut Detection) {
        let source = self.source;
        match self.build() {
            Ok(candidate) => output.candidates.push(candidate),
            Err(error) => {
                if let Some((registration, _)) = crate::registry::source(source) {
                    output.diagnostics.push(Diagnostic::error(
                        registration.id,
                        format!("candidate construction failed: {error}"),
                        None,
                    ));
                }
            }
        }
    }

    pub fn build(self) -> Result<Candidate, CandidateBuildError> {
        let (registration, source) = crate::registry::source(self.source)
            .ok_or(CandidateBuildError::UnknownSource(self.source))?;
        let action_key = self
            .action_key
            .ok_or(CandidateBuildError::Missing("action_key"))?;
        let cwd = self.cwd.ok_or(CandidateBuildError::Missing("cwd"))?;
        let selection = self
            .selection
            .ok_or(CandidateBuildError::Missing("selection"))?;
        let base_points = self
            .base_points
            .ok_or(CandidateBuildError::Missing("base_points"))?;
        if self.evidence.is_empty() {
            return Err(CandidateBuildError::Missing("evidence"));
        }
        let mut search = self.search.ok_or(CandidateBuildError::Missing("search"))?;
        let program = match self
            .program
            .ok_or(CandidateBuildError::Missing("program"))?
        {
            Program::Path(program) => program,
            Program::Tool(tool) => registration
                .tools
                .iter()
                .find(|registered| registered.id == tool)
                .map(|registered| OsString::from(registered.program))
                .ok_or(CandidateBuildError::ForeignTool {
                    detector: registration.id,
                    tool,
                })?,
        };
        search.tags.extend(
            source
                .default_tags
                .iter()
                .chain(registration.synonyms)
                .map(|tag| (*tag).to_owned()),
        );
        search.tags = search
            .tags
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut candidate = Candidate::new(
            action_key,
            registration.id,
            source.id,
            self.intent,
            self.action_name,
            program,
            self.args,
            cwd,
            base_points,
            selection,
        );
        candidate.scope_root = self.scope_root;
        candidate.env = self.env;
        candidate.passthrough = self.passthrough;
        candidate.lifecycle = self.lifecycle;
        candidate.origin = self.origin;
        candidate.layer = self.layer;
        candidate.evidence = self.evidence;
        candidate.search = search;
        candidate.label = self.label.ok_or(CandidateBuildError::Missing("label"))?;
        candidate.description = self
            .description
            .ok_or(CandidateBuildError::Missing("description"))?;
        candidate.availability = self.availability.unwrap_or_else(|| {
            crate::path::resolve_program(&candidate.program, &candidate.cwd, &candidate.env)
        });
        candidate.refresh_id();
        Ok(candidate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::candidate::{EvidenceKind, SelectionPolicy};
    use crate::registry::{NODE_SOURCE, NPM_TOOL};

    #[test]
    fn builder_attaches_registry_identity_tags_and_program() -> anyhow::Result<()> {
        let candidate = CandidateBuilder::ecosystem_task(
            NODE_SOURCE,
            Intent::Run,
            PathBuf::from("/tmp"),
            "dev",
        )
        .action_key("node:test:dev")
        .tool(NPM_TOOL)
        .args(["run", "dev"])
        .cwd(PathBuf::from("/tmp"))
        .selection(SelectionPolicy::Automatic)
        .base_points(95)
        .evidence(Evidence {
            kind: EvidenceKind::Manifest,
            reason: "fixture".to_owned(),
            points: 0,
            source: Some(PathBuf::from("package.json")),
        })
        .search(SearchDocument {
            identities: vec!["dev".to_owned()],
            ..SearchDocument::default()
        })
        .label("npm script `dev`")
        .description("fixture")
        .build()?;

        assert_eq!(candidate.program, "npm");
        assert_eq!(candidate.layer, CommandLayer::EcosystemTask);
        assert!(candidate.search.tags.iter().any(|tag| tag == "javascript"));
        Ok(())
    }
}
