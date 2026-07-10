use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::broadcast;

use super::manager::{lock, ProcessRecord};
use super::redaction::StreamRedactor;
use super::{OutputCursor, OutputStream, ProcessEvent};

pub(super) fn spawn_drain(
    mut reader: impl AsyncRead + Unpin + Send + 'static,
    stream: OutputStream,
    record: Arc<ProcessRecord>,
    events: broadcast::Sender<ProcessEvent>,
) -> tokio::task::JoinHandle<()> {
    let redactions = Arc::clone(&record.output_redactions);
    tokio::spawn(async move {
        let mut bytes = vec![0; 8192];
        let mut redactor = StreamRedactor::new(redactions);
        loop {
            let Ok(count) = reader.read(&mut bytes).await else {
                break;
            };
            if count == 0 {
                publish_output(&record, &events, stream, &redactor.finish());
                break;
            }
            publish_output(&record, &events, stream, &redactor.push(&bytes[..count]));
        }
    })
}

fn publish_output(
    record: &ProcessRecord,
    events: &broadcast::Sender<ProcessEvent>,
    stream: OutputStream,
    bytes: &[u8],
) {
    if bytes.is_empty() {
        return;
    }
    let next = {
        let mut output = lock(&record.output);
        output.push(stream, bytes);
        output.next_position()
    };
    let process_id = {
        let mut info = lock(&record.info);
        info.next_cursor.position = next;
        info.id
    };
    let _ = events.send(ProcessEvent::OutputAvailable {
        process_id,
        next_cursor: OutputCursor {
            process_id,
            position: next,
        },
    });
}
