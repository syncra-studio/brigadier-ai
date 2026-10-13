use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::fmt::writer::MakeWriterExt;
use tracing_subscriber::layer::Context;
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::{EnvFilter, Layer};

/// Dependency transports log whole requests and replies. A tab chat's MCP connection
/// carries this span through their spawned service tasks, so none of that reaches disk.
/// This unfiltered layer keeps the sentinel enabled even if the environment disables
/// this module's logging while enabling a dependency's diagnostics.
struct PublicEvents;

impl<S> Layer<S> for PublicEvents
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fn register_callsite(
        &self,
        _metadata: &'static tracing::Metadata<'static>,
    ) -> tracing::subscriber::Interest {
        tracing::subscriber::Interest::sometimes()
    }

    fn enabled(&self, metadata: &tracing::Metadata<'_>, context: Context<'_, S>) -> bool {
        metadata.is_span()
            || !context
                .lookup_current()
                .is_some_and(|span| span.scope().any(|span| span.name() == "tab_chat_private"))
    }

    fn event_enabled(&self, event: &tracing::Event<'_>, context: Context<'_, S>) -> bool {
        !context
            .event_span(event)
            .is_some_and(|span| span.scope().any(|span| span.name() == "tab_chat_private"))
    }
}

/// Environment variable with a `tracing` filter, e.g. `brigadier=debug`.
const FILTER_ENV: &str = "BRIGADIER_LOG";
const KEPT_LOG_FILES: usize = 7;

/// Structured JSON logs in daily files under `logs_dir`, written by a background thread so
/// logging never blocks the runtime. In the foreground, logs also go to stderr.
pub fn init(logs_dir: &Path, foreground: bool) -> anyhow::Result<WorkerGuard> {
    std::fs::create_dir_all(logs_dir)?;
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("brigadierd")
        .filename_suffix("log")
        .max_log_files(KEPT_LOG_FILES)
        .build(logs_dir)?;
    let (file, guard) = tracing_appender::non_blocking(appender);
    let filter = EnvFilter::try_from_env(FILTER_ENV).unwrap_or_else(|_| EnvFilter::new("info"));
    let layer = tracing_subscriber::fmt::layer()
        .json()
        .with_current_span(false)
        .with_thread_names(true);
    if foreground {
        tracing_subscriber::registry()
            .with(
                layer
                    .with_writer(file.and(std::io::stderr))
                    .with_filter(filter),
            )
            .with(PublicEvents)
            .init();
    } else {
        tracing_subscriber::registry()
            .with(layer.with_writer(file).with_filter(filter))
            .with(PublicEvents)
            .init();
    }

    // Panics anywhere (any thread, any task) are recorded before the default hook runs.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        tracing::error!(
            thread = thread.name().unwrap_or("unnamed"),
            panic = %info,
            "panic"
        );
        default_hook(info);
    }));
    Ok(guard)
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tracing::Instrument;
    use tracing::instrument::WithSubscriber;

    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    struct Host {
        private: bool,
    }

    impl brigadier_core::tools::ToolHost for Host {
        fn role(&self, _grant: &str) -> Option<brigadier_core::tools::Role> {
            Some(brigadier_core::tools::Role::TabChat {
                conversation_id: brigadier_core::ConversationId(
                    if self.private {
                        "canary-id"
                    } else {
                        "normal-id"
                    }
                    .into(),
                ),
                tools: brigadier_core::tools::TabTools::Page,
            })
        }
        fn call(
            &self,
            _grant: &str,
            _call: brigadier_core::tools::ToolCall,
        ) -> brigadier_providers::BoxFuture<'_, brigadier_core::tools::ToolReply> {
            Box::pin(async move {
                brigadier_core::tools::ToolReply::ok(if self.private {
                    "canary-content"
                } else {
                    "normal-content"
                })
            })
        }
    }

    async fn roundtrip(private: bool, span: tracing::Span) {
        let (server, client) = tokio::io::duplex(16_384);
        let cancel = tokio_util::sync::CancellationToken::new();
        let task = tokio::spawn(
            brigadier_mcp_server::serve(
                Arc::new(Host { private }),
                "grant".into(),
                server,
                cancel.clone(),
            )
            .instrument(span),
        );
        let (read, mut write) = tokio::io::split(client);
        let mut read = BufReader::new(read);
        let title = if private {
            "canary-title"
        } else {
            "normal-title"
        };
        let id = if private { "canary-id" } else { "normal-id" };
        let init = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": title, "version": "1"}
        }});
        write
            .write_all(format!("{init}\n").as_bytes())
            .await
            .unwrap();
        let mut line = String::new();
        read.read_line(&mut line).await.unwrap();
        assert!(line.contains("result"), "{line}");
        let call = serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
            "name": "read_page", "arguments": {"page_id": id}
        }});
        write
            .write_all(
                format!(
                    "{{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}}\n{call}\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        line.clear();
        read.read_line(&mut line).await.unwrap();
        assert!(
            line.contains(if private {
                "canary-content"
            } else {
                "normal-content"
            }),
            "{line}"
        );
        cancel.cancel();
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn a_tab_mcp_transport_cannot_log_requests_replies_or_formatted_errors() {
        let capture = Capture::default();
        let writer = capture.clone();
        let subscriber = tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_writer(move || writer.clone())
                    .with_filter(EnvFilter::new("warn,rmcp=trace")),
            )
            .with(PublicEvents);
        tracing::subscriber::set_global_default(subscriber).unwrap();
        async {
            let private = tracing::info_span!("tab_chat_private");
            assert!(!private.is_disabled());
            tokio::time::timeout(std::time::Duration::from_secs(10), roundtrip(true, private.clone())).await.unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(10), roundtrip(false, tracing::Span::none())).await.unwrap();
            // The transport inherits its parent's span across tasks, as rmcp does.
            tokio::spawn(async {
                let transport = tracing::info_span!("transport");
                async {
                    tracing::trace!(target: "rmcp", request = "canary-id canary-title canary-content", "new event");
                    tracing::debug!(target: "rmcp", error = "failed canary-id: canary-title: canary-content", "response error");
                    tracing::info!(target: "rmcp", reply = "canary-content", "received notification");
                }.instrument(transport).await;
            }.instrument(private).with_current_subscriber()).await.unwrap();
            tracing::warn!(conversation = "normal-conversation", error = "normal error", "normal log");
        }.await;
        let output = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
        for canary in ["canary-id", "canary-title", "canary-content"] {
            assert!(!output.contains(canary), "{output}");
        }
        assert!(
            output.contains("normal-content"),
            "normal transport tracing was not captured: {output}"
        );
        assert!(output.contains("normal-title"));
        assert!(output.contains("normal-conversation"));
        assert!(output.contains("normal error"));
    }
}
