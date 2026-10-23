//! Length-prefixed JSON frames: a big-endian `u32` byte length, then that many bytes of JSON.

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader, BufWriter};

use crate::protocol::{ClientFrame, ErrorCode, IpcError, Outcome, ServerFrame};
use crate::{Error, MAX_FRAME_BYTES};

/// A complete authenticated frame: either a client message or its parse-error response.
/// Rejected frames have been consumed, so the reader can continue at the next boundary.
#[derive(Debug)]
pub enum ClientRead {
    Frame(ClientFrame),
    Rejected(ServerFrame),
}

pub struct FrameReader<R> {
    inner: BufReader<R>,
    buffer: Vec<u8>,
}

impl<R: AsyncRead + Unpin> FrameReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner: BufReader::new(inner),
            buffer: Vec::new(),
        }
    }

    /// Reads an authenticated client frame without dropping malformed requests silently.
    /// A recoverable request id gets its normal error response; id 0 names a frame whose id
    /// could not be read. Authentication still uses the strict first-frame reader.
    pub async fn read_client(&mut self) -> Result<Option<ClientRead>, Error> {
        let raw: Box<serde_json::value::RawValue> = match self.read().await {
            Ok(Some(value)) => value,
            Ok(None) => return Ok(None),
            Err(Error::Json(error)) => return Ok(Some(rejected(0, "frame", error))),
            Err(error) => return Err(error),
        };
        // Keep the regular typed decoder first: normalizing through Value would silently
        // accept duplicate fields that ClientFrame rejects. Only failures need ID recovery.
        let error = match serde_json::from_str(raw.get()) {
            Ok(frame) => return Ok(Some(ClientRead::Frame(frame))),
            Err(error) => error,
        };
        let value: serde_json::Value = serde_json::from_str(raw.get()).unwrap_or_default();
        let id = value
            .get("id")
            .and_then(serde_json::Value::as_u64)
            .and_then(|id| u32::try_from(id).ok());
        if value.get("type").and_then(serde_json::Value::as_str) != Some("request") {
            return Ok(Some(rejected(
                id.unwrap_or(0),
                "type",
                "expected a request frame",
            )));
        }
        let Some(id) = id else {
            return Ok(Some(rejected(
                0,
                "id",
                "expected an unsigned 32-bit request id",
            )));
        };
        Ok(Some(rejected(id, "request", error)))
    }

    /// The next frame, or `None` when the peer closed the connection between frames.
    pub async fn read<T: DeserializeOwned>(&mut self) -> Result<Option<T>, Error> {
        let mut len = [0u8; 4];
        match self.inner.read_exact(&mut len).await {
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(err) => return Err(err.into()),
        }
        let len = u32::from_be_bytes(len) as usize;
        // Checked before allocating, so a hostile length cannot exhaust memory.
        if len > MAX_FRAME_BYTES {
            return Err(Error::FrameTooLarge(len));
        }
        self.buffer.resize(len, 0);
        self.inner.read_exact(&mut self.buffer).await?;
        let frame = serde_json::from_slice(&self.buffer);
        // Don't keep a huge buffer alive after an occasional large frame.
        if self.buffer.capacity() > 64 * 1024 {
            self.buffer = Vec::new();
        }
        frame.map(Some).map_err(Into::into)
    }
}

pub struct FrameWriter<W: AsyncWrite> {
    inner: BufWriter<W>,
}

impl<W: AsyncWrite + Unpin> FrameWriter<W> {
    pub fn new(inner: W) -> Self {
        Self {
            inner: BufWriter::new(inner),
        }
    }

    /// Writes and flushes one frame.
    pub async fn write<T: Serialize>(&mut self, frame: &T) -> Result<(), Error> {
        self.write_buffered(frame).await?;
        self.flush().await
    }

    /// Writes one frame without flushing, for sending several frames in a row.
    pub async fn write_buffered<T: Serialize>(&mut self, frame: &T) -> Result<(), Error> {
        let bytes = serde_json::to_vec(frame)?;
        if bytes.len() > MAX_FRAME_BYTES {
            return Err(Error::FrameTooLarge(bytes.len()));
        }
        self.inner
            .write_all(&(bytes.len() as u32).to_be_bytes())
            .await?;
        self.inner.write_all(&bytes).await?;
        Ok(())
    }

    pub async fn flush(&mut self) -> Result<(), Error> {
        self.inner.flush().await?;
        Ok(())
    }
}

fn rejected(id: u32, field: &str, error: impl std::fmt::Display) -> ClientRead {
    ClientRead::Rejected(ServerFrame::Response {
        id,
        result: Outcome::Err {
            error: IpcError {
                code: ErrorCode::Invalid,
                message: format!("Invalid field `{field}`: {error}"),
            },
        },
    })
}

#[cfg(test)]
mod client_tests {
    use super::*;
    use crate::protocol::{Request, Response};
    use serde_json::json;

    /// The daemon uses this reader after authentication. Errors are written back as regular
    /// response frames, and a valid request on the same byte stream still goes through.
    #[tokio::test]
    async fn malformed_requests_get_errors_and_the_next_request_still_parses() {
        let cases = [
            (json!({"type":"request", "id":1, "request":{"method":"sendMessage", "conversationId":"c1", "attachments":[]}}).to_string(), 1, "text"),
            (json!({"type":"request", "id":2, "request":{}}).to_string(), 2, "method"),
            (json!({"type":"request", "id":3, "request":{"method":"notARequest"}}).to_string(), 3, "request"),
            (json!({"type":"request", "id":"wrong", "request":{"method":"getCatalog"}}).to_string(), 0, "id"),
            (json!({"type":false, "id":4}).to_string(), 4, "type"),
            ("{broken json".into(), 0, "frame"),
            (r#"{"type":"request","id":7,"id":7,"request":{"method":"keepTabChats","chatIds":[]}}"#.into(), 7, "id"),
        ];
        let (client, server) = tokio::io::duplex(8192);
        let (client_read, mut client_write) = tokio::io::split(client);
        let (server_read, server_write) = tokio::io::split(server);
        let mut reader = FrameReader::new(server_read);
        let mut writer = FrameWriter::new(server_write);
        let serve = async {
            for _ in &cases {
                let ClientRead::Rejected(response) = reader.read_client().await.unwrap().unwrap()
                else {
                    panic!("malformed request was admitted");
                };
                writer.write(&response).await.unwrap();
            }
            let ClientRead::Frame(ClientFrame::Request {
                id: 99,
                request: Request::KeepTabChats { chat_ids },
            }) = reader.read_client().await.unwrap().unwrap()
            else {
                panic!("the valid request after parse failures was lost");
            };
            assert!(chat_ids.is_empty());
            writer
                .write(&ServerFrame::Response {
                    id: 99,
                    result: Outcome::Ok {
                        value: Box::new(Response::KeepTabChats),
                    },
                })
                .await
                .unwrap();
        };
        let ask = async {
            let mut reader = FrameReader::new(client_read);
            for (raw, expected_id, field) in &cases {
                client_write
                    .write_all(&(raw.len() as u32).to_be_bytes())
                    .await
                    .unwrap();
                client_write.write_all(raw.as_bytes()).await.unwrap();
                let ServerFrame::Response {
                    id,
                    result: Outcome::Err { error },
                } = reader.read().await.unwrap().unwrap()
                else {
                    panic!("parse failure did not return an error frame");
                };
                assert_eq!(id, *expected_id);
                assert_eq!(error.code, ErrorCode::Invalid);
                assert!(
                    error.message.contains(field),
                    "{} should name {field}",
                    error.message
                );
            }
            FrameWriter::new(client_write)
                .write(&ClientFrame::Request {
                    id: 99,
                    request: Request::KeepTabChats { chat_ids: vec![] },
                })
                .await
                .unwrap();
            assert!(matches!(
                reader.read().await.unwrap(),
                Some(ServerFrame::Response {
                    id: 99,
                    result: Outcome::Ok { .. }
                })
            ));
        };
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            tokio::join!(serve, ask);
        })
        .await
        .unwrap();
    }
}
