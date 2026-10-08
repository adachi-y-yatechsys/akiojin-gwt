//! Opt-in total-start admission for one bounded managed-agent trial.
//! A durable spent marker precedes every permitted physical start. There is
//! deliberately no refund/reset API: uncertain starts remain spent.
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetScope {
    pub trial_id: String,
    pub project_root: PathBuf,
    pub issue_number: u64,
    pub max_starts: u32,
}

impl BudgetScope {
    fn validate(&self) -> io::Result<()> {
        if self.max_starts != 1
            || self.issue_number == 0
            || self.trial_id.is_empty()
            || self.trial_id.len() > 128
            || !self
                .trial_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            || !self.project_root.is_absolute()
            || !self.project_root.is_dir()
            || dunce::canonicalize(&self.project_root)? != self.project_root
        {
            return Err(invalid("launch budget requires an exact canonical project, Issue, trial ID and max_starts=1"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetStatus {
    pub schema_version: u32,
    pub scope: BudgetScope,
    pub used_starts: u32,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpentStart {
    schema_version: u32,
    scope: BudgetScope,
    session_id: String,
}

#[derive(Debug)]
pub struct BudgetStore {
    path: PathBuf,
}

/// Hold until physical process creation finishes, including when unarmed.
/// `arm` takes the same lock and cannot cross an in-flight unbounded start.
#[derive(Debug)]
pub struct BudgetAdmission {
    _lock: File,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

impl BudgetStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn spent_path(&self) -> PathBuf {
        self.path.with_extension("spent.json")
    }
    pub fn for_project(project_root: &Path) -> Self {
        Self::new(
            // Trial scope is the canonical location, not Git origin. A remote
            // change or failed repository discovery must not select a new slot.
            gwt_core::paths::gwt_home()
                .join("launch-budgets")
                .join(gwt_core::repo_hash::compute_path_hash(project_root).as_str())
                .join("launch-budget.json"),
        )
    }

    // Locking never truncates either durable record. Dropping the File unlocks
    // it even on a returned error or panic; another process shares this lock.
    fn lock(&self) -> io::Result<File> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| invalid("budget path has no parent"))?;
        fs::create_dir_all(parent)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.path.with_extension("lock"))?;
        FileExt::lock_exclusive(&lock)?;
        Ok(lock)
    }

    fn write_new<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
        let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
        file.write_all(&bytes)?;
        file.sync_all()
        // A partial/failed write is retained. Readers refuse it rather than
        // interpreting corruption as a fresh allowance.
    }

    pub fn status(&self) -> io::Result<Option<BudgetStatus>> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                match fs::symlink_metadata(&self.path) {
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Ok(_) => return Err(invalid("launch budget entry exists but cannot be read")),
                    Err(error) => return Err(error),
                }
                match fs::symlink_metadata(self.spent_path()) {
                    Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                    Ok(_) => {
                        return Err(invalid(
                            "spent launch has lost its scope; budget outcome is unknown",
                        ))
                    }
                    Err(error) => return Err(error),
                }
            }
            Err(error) => return Err(error),
        };
        let mut status: BudgetStatus = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        status.scope.validate()?;
        if status.schema_version != 1 || status.used_starts != 0 {
            return Err(invalid("unsupported or changed launch budget record"));
        }
        match fs::read(self.spent_path()) {
            Ok(bytes) => {
                let spent: SpentStart = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
                if spent.schema_version != 1
                    || spent.scope != status.scope
                    || spent.session_id.is_empty()
                {
                    return Err(invalid(
                        "spent launch scope is inconsistent; budget outcome is unknown",
                    ));
                }
                status.used_starts = 1;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                match fs::symlink_metadata(self.spent_path()) {
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Ok(_) => return Err(invalid("spent launch entry exists but cannot be read")),
                    Err(error) => return Err(error),
                }
            }
            Err(error) => return Err(error),
        }
        Ok(Some(status))
    }

    pub fn arm(&self, scope: &BudgetScope) -> io::Result<BudgetStatus> {
        scope.validate()?;
        let _lock = self.lock()?;
        if let Some(status) = self.status()? {
            if status.scope != *scope {
                return Err(invalid("an existing launch budget cannot be replaced"));
            }
            return Ok(status); // idempotent readback, never a refill
        }
        let status = BudgetStatus {
            schema_version: 1,
            scope: scope.clone(),
            used_starts: 0,
        };
        Self::write_new(&self.path, &status)?;
        Ok(status)
    }

    pub fn run<T>(
        &self,
        scope: &BudgetScope,
        session_id: &str,
        start: impl FnOnce() -> io::Result<T>,
    ) -> io::Result<T> {
        let _admission = BudgetAdmission {
            _lock: self.lock()?,
        };
        let Some(status) = self.status()? else {
            return start();
        };
        self.consume(&status, scope, session_id)?;
        // The durable intent is never removed, including on start failure.
        // Keep admission locked through actual creation so arming is ordered.
        start()
    }

    fn consume(
        &self,
        status: &BudgetStatus,
        scope: &BudgetScope,
        session_id: &str,
    ) -> io::Result<()> {
        scope.validate()?;
        if session_id.is_empty() {
            return Err(invalid("bounded launch has no saved Session"));
        }
        if status.scope != *scope || status.used_starts != 0 {
            return Err(invalid(
                "launch budget target differs or its only start is already spent",
            ));
        }
        Self::write_new(
            &self.spent_path(),
            &SpentStart {
                schema_version: 1,
                scope: scope.clone(),
                session_id: session_id.to_string(),
            },
        )
    }
}

/// Physical managed-agent starts call this on both bound and unbound paths.
/// Review/retry/restore Sessions use the same project record. A direct Agent
/// preset without a saved exact Issue cannot consume an armed trial.
pub fn admit_saved_agent(
    project_root: &Path,
    observation: Option<(&Path, &str)>,
) -> Result<BudgetAdmission, String> {
    let store = BudgetStore::for_project(project_root);
    admit_saved_agent_with_observation(&store, project_root, observation).map_err(|e| e.to_string())
}

pub fn admit_saved_agent_at(
    store: &BudgetStore,
    project_root: &Path,
    sessions_dir: &Path,
    session_id: &str,
) -> io::Result<BudgetAdmission> {
    admit_saved_agent_with_observation(store, project_root, Some((sessions_dir, session_id)))
}

fn admit_saved_agent_with_observation(
    store: &BudgetStore,
    project_root: &Path,
    observation: Option<(&Path, &str)>,
) -> io::Result<BudgetAdmission> {
    let admission = BudgetAdmission {
        _lock: store.lock()?,
    };
    let Some(status) = store.status()? else {
        return Ok(admission);
    };
    let (sessions_dir, session_id) = observation
        .ok_or_else(|| invalid("armed launch budget requires a saved Issue-linked Session"))?;
    if session_id.is_empty()
        || session_id.contains(['/', '\\'])
        || session_id == "."
        || session_id == ".."
    {
        return Err(invalid("invalid saved launch Session ID"));
    }
    let session = gwt_agent::Session::load(&sessions_dir.join(format!("{session_id}.toml")))?;
    let root = dunce::canonicalize(project_root)?;
    if session.id != session_id
        || session
            .project_state_root
            .as_deref()
            .map(dunce::canonicalize)
            .transpose()?
            .as_ref()
            != Some(&root)
    {
        return Err(invalid(
            "bounded launch Session belongs to a different or unknown project",
        ));
    }
    let scope = BudgetScope {
        trial_id: session
            .launch_budget_trial_id
            .ok_or_else(|| invalid("bounded launch Session has no exact trial ID"))?,
        project_root: root,
        issue_number: session
            .linked_issue_number
            .ok_or_else(|| invalid("bounded launch Session has no exact Issue"))?,
        ..status.scope
    };
    store.consume(&status, &scope, session_id)?;
    Ok(admission)
}
