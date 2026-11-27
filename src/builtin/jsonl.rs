//! The conversation in a file, one JSON record per line.

use crate::{InMemoryStorage, Message, Storage, StorageError, StorageFuture};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

/// The format this build reads and writes. Bumped when a stored record stops
/// deserializing, so an old file is refused rather than misread.
const VERSION: u32 = 1;

/// One line of the file.
///
/// `Message` is borrowed on the way out and owned on the way in, so an append
/// serializes the run's turns without cloning them.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Record<'a> {
    Header {
        version: u32,
    },
    Message(Cow<'a, Message>),
    /// A state slot value. The last one per key wins.
    State {
        key: String,
        value: serde_json::Value,
    },
}

/// The conversation in a file, which survives the process.
///
/// One file is one conversation. The whole transcript is read at
/// [`JsonlStorage::open`] and held in an [`InMemoryStorage`], and
/// every append is written through before it is held. Windows and summaries
/// belong to [`crate::Agent::memory`], and a summary is kept in the file as
/// state.
///
/// ```no_run
/// # async fn run(agent: freyja::Agent) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
/// use freyja::JsonlStorage;
///
/// let mut chat = agent.memory(freyja::Window::groups(20)).conversation(JsonlStorage::open("chat.jsonl")?);
/// // A later process opening the same path continues this conversation.
/// chat.send("Where were we?").await?;
/// # Ok(())
/// # }
/// ```
///
/// The file is read and written with blocking `std::fs` calls, since Freyja
/// has no runtime to hand them to. Each is one small write, next to a model
/// call that takes seconds.
///
/// Nothing locks the file, so two processes over one path race the same way
/// two conversations over one backend do. A path built from a conversation id
/// a client supplied is a path the client chose, so validate the id first.
#[derive(Debug)]
pub struct JsonlStorage {
    file: File,
    inner: InMemoryStorage,
}

impl JsonlStorage {
    /// Opens the conversation at `path`, creating it if it does not exist.
    ///
    /// A final line the process died while writing is cut off, so a crash
    /// loses at most the tail of the last run. A line that does not parse
    /// anywhere else is an error, and so is a file written by a newer format.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        // A transcript is the conversation itself, so a new one is readable by
        // its owner alone. An existing file keeps the mode it already has.
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(path)?;

        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;

        // Every write ends in a newline, so bytes after the last one are an
        // append the process died inside. Cut them off, or the next append
        // would be glued onto the fragment and corrupt a line mid-file.
        let complete = bytes
            .iter()
            .rposition(|&byte| byte == b'\n')
            .map_or(0, |last| last + 1);
        if complete < bytes.len() {
            file.set_len(complete as u64)?;
        }

        let mut messages = Vec::new();
        let mut state = std::collections::HashMap::new();
        for line in bytes[..complete]
            .split(|&byte| byte == b'\n')
            .filter(|line| !line.is_empty())
        {
            let record: Record<'_> = serde_json::from_slice(line)?;
            match record {
                Record::Header { version } if version == VERSION => {}
                Record::Header { version } => {
                    return Err(format!("unsupported transcript version {version}").into());
                }
                Record::Message(message) => messages.push(message.into_owned()),
                Record::State { key, value } => {
                    state.insert(key, value);
                }
            }
        }

        if complete == 0 {
            write(&mut file, &[Record::Header { version: VERSION }])?;
        }

        Ok(Self {
            file,
            inner: InMemoryStorage::restore(messages, state),
        })
    }

    /// Everything held.
    pub fn messages(&self) -> &[Message] {
        self.inner.messages()
    }
}

/// One buffer and one `write_all`, so a crash tears at most the tail, which
/// [`JsonlStorage::open`] cuts off.
fn write(file: &mut File, records: &[Record<'_>]) -> std::io::Result<()> {
    let mut buffer = Vec::new();
    for record in records {
        serde_json::to_writer(&mut buffer, record)?;
        buffer.push(b'\n');
    }
    // Not append mode: Windows refuses `set_len` on a handle opened that way,
    // and both `open` and `clear` need it.
    file.seek(SeekFrom::End(0))?;
    file.write_all(&buffer)?;
    file.sync_data()
}

impl Storage for JsonlStorage {
    fn load(&mut self) -> StorageFuture<'_, Vec<Message>> {
        self.inner.load()
    }

    fn append(&mut self, messages: Vec<Message>) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            // The file first, so memory never holds a turn the disk lacks.
            write(
                &mut self.file,
                &messages
                    .iter()
                    .map(|message| Record::Message(Cow::Borrowed(message)))
                    .collect::<Vec<_>>(),
            )?;
            self.inner.append(messages).await
        })
    }

    fn clear(&mut self) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            self.file.set_len(0)?;
            write(&mut self.file, &[Record::Header { version: VERSION }])?;
            self.inner.clear().await
        })
    }

    fn state<'a>(&'a mut self, key: &'a str) -> StorageFuture<'a, Option<serde_json::Value>> {
        self.inner.state(key)
    }

    fn set_state<'a>(
        &'a mut self,
        key: &'a str,
        value: serde_json::Value,
    ) -> StorageFuture<'a, ()> {
        Box::pin(async move {
            // The file first, so memory never holds state the disk lacks.
            write(
                &mut self.file,
                &[Record::State {
                    key: key.to_string(),
                    value: value.clone(),
                }],
            )?;
            self.inner.set_state(key, value).await
        })
    }
}
