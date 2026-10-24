//! A connection's frames, with the live threads' news held for a frame and sent as one.
//!
//! A streaming turn says a word at a time, each its own piece of [`ThreadNews`], and several
//! sessions stream at once. Sent one by one, each would be its own task on the app's main
//! thread. Held for [`NEWS_EVERY`] instead, a session's items go in one piece, and every
//! session's in one frame. News that says more than items (a turn that opens or ends, the
//! context, background tasks, MCP servers, workers) goes at once, with what waited before it,
//! and so does every other frame: nothing is sent out of order, nor later than one interval.

use std::time::Duration;

use brigadier_core::thread_view::ThreadNews;
use brigadier_ipc::Error;
use brigadier_ipc::frame::FrameWriter;
use brigadier_ipc::protocol::{Outcome, ServerFrame};
use tokio::io::AsyncWrite;
use tokio::time::Instant;

/// The longest news waits: about a frame.
pub(crate) const NEWS_EVERY: Duration = Duration::from_millis(16);

pub(crate) struct NewsWriter<W: AsyncWrite> {
    frames: FrameWriter<W>,
    /// News waiting, items only, at most one per conversation, in the order they came.
    waiting: Vec<ThreadNews>,
    /// When what waits goes.
    due: Option<Instant>,
}

impl<W: AsyncWrite + Unpin> NewsWriter<W> {
    pub(crate) fn new(frames: FrameWriter<W>) -> Self {
        Self {
            frames,
            waiting: Vec::new(),
            due: None,
        }
    }

    /// When the news waiting goes ([`Self::send_news`]); none while none waits.
    pub(crate) fn due(&self) -> Option<Instant> {
        self.due
    }

    /// Sends `news` with the next frame, or now when it says more than items.
    pub(crate) async fn news(&mut self, news: ThreadNews) -> Result<(), Error> {
        self.news_at(news, Instant::now()).await
    }

    async fn news_at(&mut self, news: ThreadNews, now: Instant) -> Result<(), Error> {
        if !items_only(&news) {
            self.waiting.push(news);
            return self.send_news().await;
        }
        match self
            .waiting
            .iter_mut()
            .find(|held| held.conversation_id == news.conversation_id)
        {
            Some(held) => held.entries.extend(news.entries),
            None => self.waiting.push(news),
        }
        // News that keeps coming may win the connection's loop over the deadline: it is
        // sent here then.
        if *self.due.get_or_insert(now + NEWS_EVERY) <= now {
            return self.send_news().await;
        }
        Ok(())
    }

    /// Sends the news waiting, if any, and flushes.
    pub(crate) async fn send_news(&mut self) -> Result<(), Error> {
        self.write_news().await?;
        self.frames.flush().await
    }

    /// Writes and flushes one frame, after the news waiting.
    pub(crate) async fn write(&mut self, frame: &ServerFrame) -> Result<(), Error> {
        self.write_news().await?;
        self.frames.write(frame).await
    }

    /// Writes one frame without flushing, after the news waiting.
    pub(crate) async fn write_buffered(&mut self, frame: &ServerFrame) -> Result<(), Error> {
        self.write_news().await?;
        self.frames.write_buffered(frame).await
    }

    /// Sends a response ([`FrameWriter::write_response`]), after the news waiting.
    pub(crate) async fn write_response(&mut self, id: u32, result: Outcome) -> Result<(), Error> {
        self.write_news().await?;
        self.frames.write_response(id, result).await
    }

    /// Writes the news waiting as one frame, unflushed. A frame over the wire limit goes a
    /// piece at a time, as each went before.
    async fn write_news(&mut self) -> Result<(), Error> {
        self.due = None;
        if self.waiting.is_empty() {
            return Ok(());
        }
        let news = std::mem::take(&mut self.waiting);
        let frame = ServerFrame::ThreadNews { news };
        match self.frames.write_buffered(&frame).await {
            // Too large is found before a byte is written.
            Err(Error::FrameTooLarge(_)) => {
                let ServerFrame::ThreadNews { news } = frame else {
                    unreachable!()
                };
                for one in news {
                    self.frames
                        .write_buffered(&ServerFrame::ThreadNews { news: vec![one] })
                        .await?;
                }
                Ok(())
            }
            done => done,
        }
    }
}

/// Whether `news` says only items: those can wait for the next frame.
fn items_only(news: &ThreadNews) -> bool {
    !news.opened
        && news.ended.is_none()
        && news.context.is_none()
        && news.background.is_none()
        && news.mcp.is_none()
        && news.workers.is_none()
}

/// Waits until `due`; forever when none.
pub(crate) async fn news_due(due: Option<Instant>) {
    match due {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brigadier_core::ConversationId;
    use brigadier_core::thread_view::TurnEnd;
    use brigadier_ipc::frame::FrameReader;
    use brigadier_providers::ProviderEvent;
    use brigadier_providers::session_file::FileEntry;

    fn delta(conversation: &str, item: &str, text: &str) -> ThreadNews {
        ThreadNews {
            conversation_id: ConversationId(conversation.into()),
            entries: vec![FileEntry {
                event: ProviderEvent::MessageDelta {
                    item_id: item.into(),
                    text: text.into(),
                    phase: None,
                },
                at_ms: Some(1),
                updated_at_ms: None,
                parent: None,
                steer: false,
                images: Vec::new(),
                phase: None,
            }],
            opened: false,
            ended: None,
            context: None,
            background: None,
            mcp: None,
            workers: None,
        }
    }

    fn ended(conversation: &str) -> ThreadNews {
        ThreadNews {
            ended: Some(TurnEnd::Done),
            entries: Vec::new(),
            ..delta(conversation, "", "")
        }
    }

    /// Every frame the writer sent.
    async fn sent(
        run: impl AsyncFnOnce(&mut NewsWriter<tokio::io::DuplexStream>),
    ) -> Vec<ServerFrame> {
        let (send, recv) = tokio::io::duplex(1 << 20);
        let mut writer = NewsWriter::new(FrameWriter::new(send));
        run(&mut writer).await;
        drop(writer);
        let mut reader = FrameReader::new(recv);
        let mut frames = Vec::new();
        while let Some(frame) = reader.read_server().await.unwrap() {
            frames.push(frame);
        }
        frames
    }

    fn news_of(frame: &ServerFrame) -> &[ThreadNews] {
        match frame {
            ServerFrame::ThreadNews { news } => news,
            other => panic!("not news: {other:?}"),
        }
    }

    fn texts(news: &ThreadNews) -> String {
        news.entries
            .iter()
            .map(|entry| match &entry.event {
                ProviderEvent::MessageDelta { text, .. } => text.as_str(),
                _ => "?",
            })
            .collect()
    }

    /// 200 deltas from each of 5 sessions in one interval: one frame, one piece per session,
    /// every word in order; before, 1000 frames.
    #[tokio::test]
    async fn an_intervals_deltas_go_as_one_frame_one_piece_per_session() {
        let frames = sent(async |writer| {
            for n in 0..200 {
                for session in 0..5 {
                    writer
                        .news(delta(&format!("s{session}"), "m", &format!("{n} ")))
                        .await
                        .unwrap();
                }
            }
            assert!(writer.due().is_some());
            writer.send_news().await.unwrap();
            assert!(writer.due().is_none());
        })
        .await;
        assert_eq!(frames.len(), 1);
        let news = news_of(&frames[0]);
        assert_eq!(news.len(), 5);
        let words: String = (0..200).map(|n| format!("{n} ")).collect();
        for (session, piece) in news.iter().enumerate() {
            assert_eq!(piece.conversation_id.0, format!("s{session}"));
            assert_eq!(texts(piece), words);
        }
    }

    /// A turn's end goes at once, after what its session (and the others) said before it, and
    /// what comes after it waits apart; another frame goes after the news before it.
    #[tokio::test]
    async fn an_end_and_other_frames_go_at_once_after_the_news_before_them() {
        let frames = sent(async |writer| {
            writer.news(delta("a", "m", "one ")).await.unwrap();
            writer.news(delta("b", "m", "uno ")).await.unwrap();
            writer.news(ended("a")).await.unwrap();
            assert!(writer.due().is_none(), "an end goes at once");
            writer.news(delta("a", "n", "two")).await.unwrap();
            writer.write(&ServerFrame::Closing).await.unwrap();
        })
        .await;
        assert_eq!(frames.len(), 3);
        let first = news_of(&frames[0]);
        assert_eq!(first.len(), 3);
        assert_eq!(
            (
                first[0].conversation_id.0.as_str(),
                texts(&first[0]).as_str()
            ),
            ("a", "one ")
        );
        assert_eq!(
            (
                first[1].conversation_id.0.as_str(),
                texts(&first[1]).as_str()
            ),
            ("b", "uno ")
        );
        assert_eq!(first[2].ended, Some(TurnEnd::Done));
        let second = news_of(&frames[1]);
        assert_eq!((second.len(), texts(&second[0]).as_str()), (1, "two"));
        assert!(matches!(frames[2], ServerFrame::Closing));
    }

    /// The deadline is set by the first news waiting, not pushed back by the next.
    #[tokio::test]
    async fn news_waits_one_interval_at_most() {
        let (send, _recv) = tokio::io::duplex(1 << 20);
        let mut writer = NewsWriter::new(FrameWriter::new(send));
        let start = Instant::now();
        writer.news_at(delta("a", "m", "one"), start).await.unwrap();
        let later = start + Duration::from_millis(10);
        writer.news_at(delta("a", "m", "two"), later).await.unwrap();
        assert_eq!(writer.due(), Some(start + NEWS_EVERY));
        // Past its deadline, it goes with the next news even before the loop's timer.
        writer
            .news_at(delta("a", "m", "three"), start + NEWS_EVERY)
            .await
            .unwrap();
        assert_eq!(writer.due(), None);
    }

    /// Frames counted for streams as the audit measured them: 5 sessions at 60 and at 300
    /// deltas a second each, for a second, on a clock of their own, sent when due as the
    /// connection's loop sends them. Before, one frame per delta.
    #[tokio::test]
    async fn five_streaming_sessions_send_one_frame_a_frame() {
        for rate in [60u64, 300] {
            let frames = sent(async |writer| {
                let start = Instant::now();
                // Each session's deltas, evenly apart, offset per session.
                let mut arrivals: Vec<(Duration, usize)> = (0..5)
                    .flat_map(|session| {
                        (0..rate).map(move |n| {
                            let at = n * 1_000_000 / rate + session as u64 * 3_100;
                            (Duration::from_micros(at), session)
                        })
                    })
                    .collect();
                arrivals.sort();
                for (at, session) in arrivals {
                    if writer.due().is_some_and(|due| due <= start + at) {
                        writer.send_news().await.unwrap();
                    }
                    let news = delta(&format!("s{session}"), "m", "w ");
                    writer.news_at(news, start + at).await.unwrap();
                }
                writer.send_news().await.unwrap();
            })
            .await;
            let words: usize = frames
                .iter()
                .flat_map(|frame| news_of(frame).iter())
                .map(|news| news.entries.len())
                .sum();
            assert_eq!(words as u64, 5 * rate, "every delta arrives");
            assert!(
                frames.len() <= 63,
                "{} frames for {words} deltas",
                frames.len()
            );
            eprintln!(
                "5 sessions × {rate} deltas/s for 1 s: {} frames, was {words}",
                frames.len()
            );
        }
    }
}
