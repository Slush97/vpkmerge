//! One permission policy for every agent. The built-in harness asks it before
//! running a tool, and approval requests from ACP agents are answered by it.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::acp::{AgentProcess, PermissionOption};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionLevel {
    /// Nothing that changes files or runs commands.
    ReadOnly,
    /// Reads run; everything else waits for the user.
    #[default]
    Ask,
    /// File edits run too; commands and other tools still wait.
    AutoEdit,
    /// Everything runs.
    FullAccess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    Read,
    Edit,
    Execute,
    /// Anything we cannot classify, so it is treated as the riskiest kind.
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Ask,
    Deny,
}

impl PermissionLevel {
    pub fn decide(self, kind: ActionKind) -> Decision {
        match (self, kind) {
            (_, ActionKind::Read) | (Self::FullAccess, _) | (Self::AutoEdit, ActionKind::Edit) => {
                Decision::Allow
            }
            (Self::ReadOnly, _) => Decision::Deny,
            (Self::Ask | Self::AutoEdit, _) => Decision::Ask,
        }
    }
}

impl ActionKind {
    /// From the `kind` of an ACP tool call.
    pub fn from_acp(kind: Option<&str>) -> Self {
        match kind {
            Some("read" | "search" | "think") => Self::Read,
            Some("edit" | "delete" | "move") => Self::Edit,
            Some("execute") => Self::Execute,
            _ => Self::Other,
        }
    }
}

/// A prompt waiting on the user.
pub(crate) enum PendingPermission {
    /// Asked by an ACP agent; the answer goes back over its RPC.
    Agent {
        process: Arc<AgentProcess>,
        rpc_id: serde_json::Value,
    },
    /// Asked by the built-in harness; the turn is waiting on the chosen option.
    Tool(oneshot::Sender<Option<String>>),
}

pub(crate) const ALLOW_ONCE: &str = "allow";
pub(crate) const ALLOW_SESSION: &str = "allowForSession";
const DENY: &str = "deny";

/// The choices for a tool the built-in harness wants to run.
pub(crate) fn tool_options() -> Vec<PermissionOption> {
    let option = |option_id: &str, name: &str, kind: &str| PermissionOption {
        option_id: option_id.into(),
        name: name.into(),
        kind: kind.into(),
    };
    vec![
        option(ALLOW_ONCE, "Allow once", "allow_once"),
        option(ALLOW_SESSION, "Allow for this session", "allow_always"),
        option(DENY, "Deny", "reject_once"),
    ]
}

#[cfg(test)]
mod tests {
    use super::ActionKind::{Edit, Execute, Other, Read};
    use super::Decision::{Allow, Ask, Deny};
    use super::*;

    #[test]
    fn levels_decide_each_kind() {
        let table = [
            (PermissionLevel::ReadOnly, [Allow, Deny, Deny, Deny]),
            (PermissionLevel::Ask, [Allow, Ask, Ask, Ask]),
            (PermissionLevel::AutoEdit, [Allow, Allow, Ask, Ask]),
            (PermissionLevel::FullAccess, [Allow, Allow, Allow, Allow]),
        ];
        for (level, expected) in table {
            let got = [Read, Edit, Execute, Other].map(|kind| level.decide(kind));
            assert_eq!(got, expected, "{level:?}");
        }
    }

    #[test]
    fn unknown_acp_kinds_are_never_read() {
        assert_eq!(ActionKind::from_acp(Some("read")), Read);
        assert_eq!(ActionKind::from_acp(Some("delete")), Edit);
        assert_eq!(ActionKind::from_acp(Some("execute")), Execute);
        assert_eq!(ActionKind::from_acp(Some("fetch")), Other);
        assert_eq!(ActionKind::from_acp(None), Other);
    }

    #[test]
    fn a_new_install_asks_first() {
        assert_eq!(PermissionLevel::default(), PermissionLevel::Ask);
        assert_eq!(
            serde_json::to_string(&PermissionLevel::AutoEdit).unwrap(),
            r#""autoEdit""#
        );
    }
}
