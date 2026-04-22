use std::collections::HashMap;
use std::sync::Arc;

/// Lifecycle status of a registered agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentStatus {
    Active,
    Done,
}

/// Metadata for a single agent in the tree.
#[derive(Debug)]
pub struct AgentInfo {
    pub agent_id: String,
    pub parent_id: Option<String>,
    pub depth: usize,
    pub status: AgentStatus,
    pub task: Option<tokio::task::JoinHandle<()>>,
}

/// Describes which fork limit was violated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForkLimitError {
    MaxConcurrentPerParent { parent_id: String, limit: usize },
    MaxDepth { depth: usize, limit: usize },
    MaxTotal { total: usize, limit: usize },
    ParentNotFound { parent_id: String },
}

impl std::fmt::Display for ForkLimitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MaxConcurrentPerParent { parent_id, limit } => {
                write!(
                    f,
                    "parent \"{parent_id}\" already has {limit} active children (limit: {limit})"
                )
            }
            Self::MaxDepth { depth, limit } => {
                write!(f, "child depth {depth} exceeds max depth {limit}")
            }
            Self::MaxTotal { total, limit } => {
                write!(f, "total agents {total} reached max {limit}")
            }
            Self::ParentNotFound { parent_id } => {
                write!(f, "parent \"{parent_id}\" not found in agent tree")
            }
        }
    }
}

impl std::error::Error for ForkLimitError {}

/// Manages a hierarchical agent tree and enforces fork limits.
pub struct AgentManager {
    pub max_concurrent_per_parent: usize,
    pub max_depth: usize,
    pub max_total: usize,
    agents: HashMap<String, AgentInfo>,
}

impl AgentManager {
    /// Create a new manager with the given limits.
    #[must_use]
    pub fn new(max_concurrent_per_parent: usize, max_depth: usize, max_total: usize) -> Self {
        Self {
            max_concurrent_per_parent,
            max_depth,
            max_total,
            agents: HashMap::new(),
        }
    }

    /// Register a root agent at depth 0.
    pub fn register_root(&mut self, agent_id: impl Into<String>) {
        let id = agent_id.into();
        self.agents.insert(
            id.clone(),
            AgentInfo {
                agent_id: id,
                parent_id: None,
                depth: 0,
                status: AgentStatus::Active,
                task: None,
            },
        );
    }

    /// Check whether all three fork limits allow a new child under `parent_id`.
    #[must_use]
    pub fn can_fork(&self, parent_id: &str) -> bool {
        let Some(parent) = self.agents.get(parent_id) else {
            return false;
        };

        let active_children = self
            .agents
            .values()
            .filter(|a| {
                a.parent_id.as_deref() == Some(parent_id) && a.status == AgentStatus::Active
            })
            .count();

        if active_children >= self.max_concurrent_per_parent {
            return false;
        }
        if parent.depth + 1 > self.max_depth {
            return false;
        }
        if self.agents.len() >= self.max_total {
            return false;
        }

        true
    }

    /// Atomically verify limits and register a child agent.
    ///
    /// # Errors
    ///
    /// Returns `ForkLimitError` when any of the three limits would be exceeded,
    /// or when the parent is not found.
    pub fn register_child(
        &mut self,
        child_id: impl Into<String>,
        parent_id: &str,
        task: Option<tokio::task::JoinHandle<()>>,
    ) -> Result<(), ForkLimitError> {
        let parent = self
            .agents
            .get(parent_id)
            .ok_or_else(|| ForkLimitError::ParentNotFound {
                parent_id: parent_id.to_owned(),
            })?;

        let active_children = self
            .agents
            .values()
            .filter(|a| {
                a.parent_id.as_deref() == Some(parent_id) && a.status == AgentStatus::Active
            })
            .count();

        if active_children >= self.max_concurrent_per_parent {
            return Err(ForkLimitError::MaxConcurrentPerParent {
                parent_id: parent_id.to_owned(),
                limit: self.max_concurrent_per_parent,
            });
        }

        let child_depth = parent.depth + 1;
        if child_depth > self.max_depth {
            return Err(ForkLimitError::MaxDepth {
                depth: child_depth,
                limit: self.max_depth,
            });
        }

        if self.agents.len() >= self.max_total {
            return Err(ForkLimitError::MaxTotal {
                total: self.agents.len(),
                limit: self.max_total,
            });
        }

        let id = child_id.into();
        self.agents.insert(
            id.clone(),
            AgentInfo {
                agent_id: id,
                parent_id: Some(parent_id.to_owned()),
                depth: child_depth,
                status: AgentStatus::Active,
                task,
            },
        );

        Ok(())
    }

    /// All children (active + done) of `parent_id`.
    #[must_use]
    pub fn get_children(&self, parent_id: &str) -> Vec<&AgentInfo> {
        self.agents
            .values()
            .filter(|a| a.parent_id.as_deref() == Some(parent_id))
            .collect()
    }

    /// Only active children of `parent_id`.
    #[must_use]
    pub fn get_active_children(&self, parent_id: &str) -> Vec<&AgentInfo> {
        self.agents
            .values()
            .filter(|a| {
                a.parent_id.as_deref() == Some(parent_id) && a.status == AgentStatus::Active
            })
            .collect()
    }

    /// Depth of the given agent (0 for root / unknown).
    #[must_use]
    pub fn get_depth(&self, agent_id: &str) -> usize {
        self.agents.get(agent_id).map_or(0, |a| a.depth)
    }

    /// Mark an agent as [`AgentStatus::Done`].
    pub fn mark_done(&mut self, agent_id: &str) {
        if let Some(agent) = self.agents.get_mut(agent_id) {
            agent.status = AgentStatus::Done;
        }
    }

    #[must_use]
    pub fn contains(&self, agent_id: &str) -> bool {
        self.agents.contains_key(agent_id)
    }

    /// Abort all active children of `parent_id` by calling
    /// [`JoinHandle::abort`](tokio::task::JoinHandle::abort) on their tasks.
    pub fn abort_children(&mut self, parent_id: &str) {
        let child_ids: Vec<String> = self
            .agents
            .values()
            .filter(|a| {
                a.parent_id.as_deref() == Some(parent_id) && a.status == AgentStatus::Active
            })
            .map(|a| a.agent_id.clone())
            .collect();

        for id in child_ids {
            if let Some(agent) = self.agents.get_mut(&id) {
                if let Some(handle) = agent.task.take() {
                    handle.abort();
                }
                agent.status = AgentStatus::Done;
            }
        }
    }
}

/// Shared, async-safe handle to an `AgentManager`.
pub type SharedAgentManager = Arc<tokio::sync::Mutex<AgentManager>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_manager_register_root() {
        let mut mgr = AgentManager::new(3, 3, 10);
        mgr.register_root("root");
        assert_eq!(mgr.get_depth("root"), 0);
        assert_eq!(mgr.agents.get("root").unwrap().status, AgentStatus::Active);
        assert!(mgr.agents.get("root").unwrap().parent_id.is_none());
    }

    #[test]
    fn test_agent_manager_can_fork_respects_concurrent_limit() {
        let mut mgr = AgentManager::new(2, 5, 10);
        mgr.register_root("root");

        mgr.register_child("c1", "root", None).unwrap();
        mgr.register_child("c2", "root", None).unwrap();

        assert!(!mgr.can_fork("root"));
    }

    #[test]
    fn test_agent_manager_can_fork_respects_depth_limit() {
        let mut mgr = AgentManager::new(10, 2, 20);
        mgr.register_root("root");
        mgr.register_child("d1", "root", None).unwrap();
        mgr.register_child("d2", "d1", None).unwrap();

        assert!(!mgr.can_fork("d2"));
    }

    #[test]
    fn test_agent_manager_can_fork_respects_total_limit() {
        let mut mgr = AgentManager::new(10, 10, 3);
        mgr.register_root("root");
        mgr.register_child("c1", "root", None).unwrap();
        mgr.register_child("c2", "root", None).unwrap();

        assert!(!mgr.can_fork("root"));
    }

    #[test]
    fn test_agent_manager_register_child_succeeds() {
        let mut mgr = AgentManager::new(5, 5, 10);
        mgr.register_root("root");

        let result = mgr.register_child("child1", "root", None);
        assert!(result.is_ok());
        assert_eq!(mgr.get_depth("child1"), 1);
        assert_eq!(
            mgr.agents.get("child1").unwrap().parent_id.as_deref(),
            Some("root")
        );
    }

    #[test]
    fn test_agent_manager_register_child_at_limit_fails() {
        let mut mgr = AgentManager::new(1, 5, 10);
        mgr.register_root("root");
        mgr.register_child("c1", "root", None).unwrap();

        let err = mgr.register_child("c2", "root", None).unwrap_err();
        assert_eq!(
            err,
            ForkLimitError::MaxConcurrentPerParent {
                parent_id: "root".into(),
                limit: 1,
            }
        );

        let err = mgr.register_child("c3", "ghost", None).unwrap_err();
        assert_eq!(
            err,
            ForkLimitError::ParentNotFound {
                parent_id: "ghost".into(),
            }
        );
    }

    #[test]
    fn test_agent_manager_mark_done() {
        let mut mgr = AgentManager::new(3, 3, 10);
        mgr.register_root("root");
        mgr.register_child("c1", "root", None).unwrap();

        mgr.mark_done("c1");
        assert_eq!(mgr.agents.get("c1").unwrap().status, AgentStatus::Done);
    }

    #[test]
    fn test_agent_manager_get_active_children_excludes_done() {
        let mut mgr = AgentManager::new(5, 5, 10);
        mgr.register_root("root");
        mgr.register_child("c1", "root", None).unwrap();
        mgr.register_child("c2", "root", None).unwrap();
        mgr.register_child("c3", "root", None).unwrap();

        mgr.mark_done("c2");

        let all = mgr.get_children("root");
        assert_eq!(all.len(), 3);

        let active = mgr.get_active_children("root");
        assert_eq!(active.len(), 2);
        assert!(active.iter().all(|a| a.status == AgentStatus::Active));
    }
}
