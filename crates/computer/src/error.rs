//! Stable error codes. Each one carries a line on what to do next, because the reader is a
//! model choosing its next call (§4.4 of docs/COMPUTER-USE-PLAN.md).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    StaleRef,
    StaleGeometry,
    Invalidated,
    Occluded,
    BackgroundUnavailable,
    UnsupportedCapability,
    SecureField,
    Blocked,
    Busy,
    NotSettable,
    NoSuchAction,
    NoSuchTarget,
    AppNotResponding,
    Deadline,
    PermissionMissing,
    StoppedByUser,
    Cancelled,
    BadRequest,
    Failed,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StaleRef => "stale_ref",
            Self::StaleGeometry => "stale_geometry",
            Self::Invalidated => "invalidated",
            Self::Occluded => "occluded",
            Self::BackgroundUnavailable => "background_unavailable",
            Self::UnsupportedCapability => "unsupported_capability",
            Self::SecureField => "secure_field",
            Self::Blocked => "blocked",
            Self::Busy => "busy",
            Self::NotSettable => "not_settable",
            Self::NoSuchAction => "no_such_action",
            Self::NoSuchTarget => "no_such_target",
            Self::AppNotResponding => "app_not_responding",
            Self::Deadline => "deadline",
            Self::PermissionMissing => "permission_missing",
            Self::StoppedByUser => "stopped_by_user",
            Self::Cancelled => "cancelled",
            Self::BadRequest => "bad_request",
            Self::Failed => "failed",
        }
    }

    /// What the caller should do next.
    pub fn next_step(self) -> &'static str {
        match self {
            Self::StaleRef => "observe again; that element changed or is gone",
            Self::StaleGeometry => {
                "observe again with a screenshot; the window changed size since that image"
            }
            Self::Invalidated => {
                "observe again; an earlier action changed the window, so later targets are gone"
            }
            Self::Occluded => {
                "the window can't be reached where it is; use an element action instead"
            }
            Self::BackgroundUnavailable => {
                "this can't be done without taking the user's focus; use an element action or report it"
            }
            Self::UnsupportedCapability => "this system can't do that; use another action",
            Self::SecureField => {
                "a password field has focus; Brigadier never types into or reads one"
            }
            Self::Blocked => "that app or window is off limits to workers",
            Self::Busy => "another worker is using that window; wait or pick another",
            Self::NotSettable => "that element can't take a value; click or type instead",
            Self::NoSuchAction => {
                "the element doesn't list that action; observe shows the ones it has"
            }
            Self::NoSuchTarget => "no such app, window, element or image; check the id",
            Self::AppNotResponding => "the app isn't answering; wait and observe again",
            Self::Deadline => "the request ran out of time; observe to see where it stopped",
            Self::PermissionMissing => {
                "Brigadier lacks a system permission; ask the user to grant it"
            }
            Self::StoppedByUser => "the user stopped computer use; don't retry unless asked",
            Self::Cancelled => "the request was cancelled",
            Self::BadRequest => "fix the arguments and call again",
            Self::Failed => "observe again before retrying",
        }
    }
}

/// An error with its code and a short detail for the caller.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[error("{}: {detail} ({})", code.as_str(), code.next_step())]
pub struct CuError {
    pub code: ErrorCode,
    pub detail: String,
}

impl CuError {
    pub fn new(code: ErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

pub type CuResult<T> = Result<T, CuError>;

/// Shorthand for building an error.
pub fn err<T>(code: ErrorCode, detail: impl Into<String>) -> CuResult<T> {
    Err(CuError::new(code, detail))
}
