use std::sync::Arc;

pub(super) struct StreamRedactor {
    secrets: Arc<Vec<Vec<u8>>>,
    pending: Vec<u8>,
    hold_back: usize,
}

impl StreamRedactor {
    pub(super) fn new(secrets: Arc<Vec<Vec<u8>>>) -> Self {
        let hold_back = secrets
            .iter()
            .map(Vec::len)
            .max()
            .unwrap_or(1)
            .saturating_sub(1);
        Self {
            secrets,
            pending: Vec::new(),
            hold_back,
        }
    }

    pub(super) fn push(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.pending.extend_from_slice(bytes);
        let proposed = self.pending.len().saturating_sub(self.hold_back);
        let emit_len = self.secrets.iter().fold(proposed, |boundary, secret| {
            crossing_match_start(&self.pending, secret, boundary).unwrap_or(boundary)
        });
        if emit_len == 0 {
            return Vec::new();
        }
        let emitted = self.pending.drain(..emit_len).collect::<Vec<_>>();
        redact_bytes(emitted, &self.secrets)
    }

    pub(super) fn finish(mut self) -> Vec<u8> {
        redact_bytes(std::mem::take(&mut self.pending), &self.secrets)
    }
}

fn redact_bytes(mut bytes: Vec<u8>, secrets: &[Vec<u8>]) -> Vec<u8> {
    const REDACTION: &[u8] = b"[REDACTED_SECRET]";
    for secret in secrets {
        if secret.is_empty() {
            continue;
        }
        let mut redacted = Vec::with_capacity(bytes.len());
        let mut offset = 0;
        while let Some(found) = find_bytes(&bytes[offset..], secret) {
            let start = offset + found;
            redacted.extend_from_slice(&bytes[offset..start]);
            redacted.extend_from_slice(REDACTION);
            offset = start + secret.len();
        }
        redacted.extend_from_slice(&bytes[offset..]);
        bytes = redacted;
    }
    bytes
}

fn crossing_match_start(haystack: &[u8], needle: &[u8], boundary: usize) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
        .filter(|start| *start < boundary && start + needle.len() > boundary)
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hides_secret_split_across_reads() {
        let secrets = Arc::new(vec![b"private-token".to_vec()]);
        let mut redactor = StreamRedactor::new(secrets);
        let mut output = redactor.push(b"before private-");
        output.extend(redactor.push(b"token after"));
        output.extend(redactor.finish());
        let text = String::from_utf8(output).unwrap();
        assert_eq!(text, "before [REDACTED_SECRET] after");
    }
}
