use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use crate::event::{LogLine, LogStream};

/// Bounded so a chatty service cannot grow the engine without limit.
pub(crate) struct RingBuffer {
    lines: VecDeque<LogLine>,
    capacity: usize,
}

impl RingBuffer {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            lines: VecDeque::with_capacity(capacity.min(256)),
            capacity,
        }
    }

    pub(crate) fn push(&mut self, line: LogLine) {
        if self.lines.len() == self.capacity {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }

    pub(crate) fn tail(&self, lines: usize) -> Vec<LogLine> {
        let skip = self.lines.len().saturating_sub(lines);
        self.lines.iter().skip(skip).cloned().collect()
    }
}

/// Fans one captured line out to the bounded history and to live subscribers.
#[derive(Clone)]
pub(crate) struct LogSink {
    pub(crate) buffer: Arc<Mutex<RingBuffer>>,
    pub(crate) live: broadcast::Sender<LogLine>,
}

impl LogSink {
    fn push(&self, line: LogLine) {
        if let Ok(mut buffer) = self.buffer.lock() {
            buffer.push(line.clone());
        }
        // An error only means nobody is subscribed.
        let _ = self.live.send(line);
    }
}

pub(crate) fn pump<R>(reader: R, stream: LogStream, sink: LogSink) -> JoinHandle<()>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(text)) = lines.next_line().await {
            sink.push(LogLine::new(stream, text));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_buffer_keeps_only_the_newest_lines() {
        let mut buffer = RingBuffer::new(3);
        for n in 1..=5 {
            buffer.push(LogLine::new(LogStream::Stdout, format!("line {n}")));
        }

        let tail: Vec<String> = buffer.tail(10).into_iter().map(|l| l.text).collect();
        assert_eq!(tail, ["line 3", "line 4", "line 5"]);
        assert_eq!(buffer.tail(1)[0].text, "line 5");
    }
}
