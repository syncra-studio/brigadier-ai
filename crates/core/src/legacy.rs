//! Records written by earlier versions, brought in line with how Brigadier works now as they
//! are read, so a store from before a change keeps loading and nothing it holds acts again.

use crate::model::DomainEvent;
use crate::work::{ApprovalSubject, CardState};

/// Brings a stored event written by an earlier version up to date.
pub(crate) fn normalize(event: &mut DomainEvent) {
    if let DomainEvent::ApprovalUpdated { approval } = event
        && matches!(approval.subject, ApprovalSubject::Landing { .. })
        && approval.state == CardState::Pending
    {
        // Landings no longer ask: the card that asked is over, whatever happened to it.
        approval.state = CardState::Expired {
            reason: "Brigadier lands finished work on its own now.".into(),
        };
        approval.resolved_at_ms = Some(approval.created_at_ms);
    }
}
