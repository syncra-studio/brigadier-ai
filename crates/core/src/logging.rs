//! Tab chats must never put identifiers or arbitrary error text in machine logs.
//!
//! Keep the diagnostic's static message, but omit every dynamic field for a tab chat:
//! provider errors and cleanup failures can echo titles, content and scratch paths.
macro_rules! private_trace {
    ($private:expr, $level:ident, $message:literal; $($fields:tt)*) => {
        if $private {
            tracing::$level!(conversation = "tab chat", $message);
        } else {
            tracing::$level!($($fields)*, $message);
        }
    };
}

pub(crate) use private_trace;
