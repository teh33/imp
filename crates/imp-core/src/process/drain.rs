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
        let mut pending_utf8 = Vec::new();
        loop {
            let Ok(count) = reader.read(&mut bytes).await else {
                break;
            };
            if count == 0 {
                pending_utf8.extend_from_slice(&redactor.finish());
                publish_output(&record, &events, stream, &pending_utf8);
                break;
            }
            pending_utf8.extend_from_slice(&redactor.push(&bytes[..count]));
            let publishable = publishable_utf8_len(&pending_utf8);
            publish_output(&record, &events, stream, &pending_utf8[..publishable]);
            pending_utf8.drain(..publishable);
        }
    })
}

fn publishable_utf8_len(bytes: &[u8]) -> usize {
    match std::str::from_utf8(bytes) {
        Ok(_) => bytes.len(),
        Err(error) if error.error_len().is_none() => error.valid_up_to(),
        Err(_) => bytes.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::publishable_utf8_len;

    #[test]
    fn retains_incomplete_utf8_suffix() {
        let value = "page café".as_bytes();
        assert_eq!(
            publishable_utf8_len(&value[..value.len() - 1]),
            value.len() - 2
        );
        assert_eq!(publishable_utf8_len(value), value.len());
    }
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
