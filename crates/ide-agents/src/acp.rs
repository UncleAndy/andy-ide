//! The ACP connection as the standard's own SDK makes it, with this project's record kept.
//!
//! Two rules meet here. The panel's rule about the wire is not negotiable ([`crate::wire`]): every line that
//! crosses must be written down as it crosses, because a panel that draws the conversation can hide a line,
//! and a hidden line looks exactly like silence. The standard's rule is that a client brings its own runtime
//! (`agent_client_protocol` carries none), and that its transport is anything that reads and writes.
//!
//! So the child's pipes are wrapped in two small reporting types and the SDK is handed the wrappers
//! (`ByteStreams` takes any pair, splitting the lines itself). The agent's own error output is not a frame of
//! the protocol and goes into the record as a note, the way the session's own reader does it.

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use agent_client_protocol::{AcpAgent, AcpAgentConfig, ByteStreams};
use async_process::{Child, ChildStderr, ChildStdin, ChildStdout};
use futures::StreamExt;
use futures::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, BufReader};

use crate::wire::Wire;

/// An agent started for one session, with the transport the SDK drives.
pub struct Started {
    /// Lines to and from the agent, every one of them written into the record as it crosses.
    pub transport: ByteStreams<ReportingWriter<ChildStdin>, ReportingReader<ChildStdout>>,
    /// The agent's own error output. Nothing reads it by itself: whoever drives the connection hands it to
    /// [`note_the_agent_words`], or a full pipe stops the agent mid-sentence.
    pub stderr: ChildStderr,
    /// The process itself: it lives exactly as long as this does.
    pub child: Child,
}

/// Starts `command` and opens the transport the SDK talks over, with the record kept.
pub fn start(
    command: &str,
    args: &[String],
    env: &BTreeMap<String, String>,
    wire: Arc<dyn Wire>,
) -> anyhow::Result<Started> {
    let mut config = AcpAgentConfig::new(command);
    for arg in args {
        config = config.arg(arg.clone());
    }
    for (name, value) in env {
        config = config.env(name.clone(), value.clone());
    }
    // The low-level escape hatch, and not the SDK's own connected agent: the child's pipes have to pass
    // through us for the record to be kept, which means owning the process too.
    let (stdin, stdout, stderr, child) = AcpAgent::new(config).spawn_process()?;
    wire.note(&format!("{command} {args:?} started"));
    Ok(Started {
        transport: ByteStreams::new(
            ReportingWriter::new(stdin, Arc::clone(&wire)),
            ReportingReader::new(stdout, wire),
        ),
        stderr,
        child,
    })
}

/// Reads the agent's own words until the pipe closes, writing each line into the record.
///
/// Not a frame of the protocol: an agent that says why it is unhappy says it here, and a person reading the
/// record afterwards is the one who needs it.
pub async fn note_the_agent_words(stderr: ChildStderr, wire: Arc<dyn Wire>) {
    let mut lines = BufReader::new(stderr).lines();
    while let Some(Ok(line)) = lines.next().await {
        wire.note(&line);
    }
}

/// The agent's input: every whole line is written into the record as the pipe takes it.
pub struct ReportingWriter<W> {
    inner: W,
    wire: Arc<dyn Wire>,
    /// What has been written since the last newline: a line is the record's unit, not a write.
    pending: Vec<u8>,
}

impl<W> ReportingWriter<W> {
    /// Wraps `inner`, writing down what crosses it.
    pub fn new(inner: W, wire: Arc<dyn Wire>) -> Self {
        Self {
            inner,
            wire,
            pending: Vec::new(),
        }
    }

    /// Notes every line among `bytes`, keeping the last one if it is not whole yet.
    ///
    /// Called after the write and not before: a line the pipe refused did not cross the wire. (The session
    /// that owns its own threads notes a line before writing it, which is honest for a write that is one call
    /// and would be a lie for one that arrives in pieces.)
    fn note_what_left(&mut self, bytes: &[u8]) {
        take_lines(&mut self.pending, bytes, &self.wire, true);
    }
}

/// The agent's output: every whole line is written into the record as it arrives.
pub struct ReportingReader<R> {
    inner: R,
    wire: Arc<dyn Wire>,
    /// What has arrived since the last newline, for the same reason as the writer's.
    pending: Vec<u8>,
}

impl<R> ReportingReader<R> {
    /// Wraps `inner`, writing down what crosses it.
    pub fn new(inner: R, wire: Arc<dyn Wire>) -> Self {
        Self {
            inner,
            wire,
            pending: Vec::new(),
        }
    }

    fn note_what_arrived(&mut self, bytes: &[u8]) {
        take_lines(&mut self.pending, bytes, &self.wire, false);
    }
}

/// Splits `bytes` into lines, notes the whole ones, and keeps the rest for the next call.
fn take_lines(pending: &mut Vec<u8>, bytes: &[u8], wire: &Arc<dyn Wire>, sent: bool) {
    for byte in bytes {
        if *byte == b'\n' {
            let line = String::from_utf8_lossy(pending)
                .trim_end_matches('\r')
                .to_owned();
            if !line.is_empty() {
                if sent {
                    wire.sent(&line);
                } else {
                    wire.got(&line);
                }
            }
            pending.clear();
        } else {
            pending.push(*byte);
        }
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for ReportingWriter<W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        let poll = Pin::new(&mut this.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(count)) = poll {
            let left = buf[..count].to_vec();
            this.note_what_left(&left);
        }
        poll
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let poll = Pin::new(&mut this.inner).poll_close(cx);
        if poll.is_ready() && !this.pending.is_empty() {
            // A last line without its newline: the pipe is closed, so it is as whole as it will ever be.
            let left = std::mem::take(&mut this.pending);
            this.note_what_left(&left);
        }
        poll
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for ReportingReader<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        let poll = Pin::new(&mut this.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(count)) = poll {
            let arrived = buf[..count].to_vec();
            this.note_what_arrived(&arrived);
        }
        poll
    }
}
