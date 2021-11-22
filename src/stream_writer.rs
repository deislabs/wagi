use std::{io::Write, sync::{Arc, RwLock}};

use async_stream::stream;

#[derive(Clone)]
pub struct StreamWriter {
    pending: Arc<RwLock<Vec<u8>>>,
    done: Arc<RwLock<bool>>,
    // A way for the write side to signal new data to the stream side
    // ETA: WHICH DOESN'T WORK AND I DON'T KNOW WHY
    // write_index: Arc<RwLock<i64>>,
    // write_index_sender: Arc<tokio::sync::watch::Sender<i64>>,
    // write_index_receiver: tokio::sync::watch::Receiver<i64>,
}

impl StreamWriter {
    pub fn new() -> Self {
        // let write_index = 0;
        // let (tx, rx) = tokio::sync::watch::channel(write_index);
        Self {
            pending: Arc::new(RwLock::new(vec![])),
            done: Arc::new(RwLock::new(false)),
            // write_index: Arc::new(RwLock::new(write_index)),
            // write_index_sender: Arc::new(tx),
            // write_index_receiver: rx,
        }
    }

    fn append(&mut self, buf: &[u8]) -> anyhow::Result<()> {
        let result = match self.pending.write().as_mut() {
            Ok(pending) => {
                pending.extend_from_slice(buf);
                Ok(())
            },
            Err(e) =>
                Err(anyhow::anyhow!("Internal error: StreamWriter::append can't take lock: {}", e))
        };
        // This was meant to wake up listener threads when there was new data but it ended up
        // just stalling until input was complete. TODO: investigate so we can get rid of the
        // duration-based polling.
        // {
        //     let mut write_index = self.write_index.write().unwrap();
        //     *write_index = *write_index + 1;
        //     self.write_index_sender.send(*write_index).unwrap();
        // }
        result
    }

    pub fn done(&mut self) -> anyhow::Result<()> {
        match self.done.write().as_deref_mut() {
            Ok(d) => {
                *d = true;
                Ok(())
            },
            Err(e) =>
                Err(anyhow::anyhow!("Internal error: StreamWriter::done can't take lock: {}", e))

        }
    }

    pub async fn header_block(&mut self) -> anyhow::Result<Vec<u8>> {
        loop {
            match self.pending.write().as_deref_mut() {
                Ok(pending) => match split_at_two_newlines(&pending) {
                    None => (),
                    Some((header_block, rest)) => {
                        *pending = rest;
                        return Ok(header_block);
                    }
                },
                Err(e) => {
                    return Err(anyhow::anyhow!("Internal error: StreamWriter::header_block can't take lock: {}", e));
                },
            }
            // See comments on the as_stream loop, though using the change signal
            // blocked this *completely* until end of writing!  (And everything else
            // waits on this.)
            tokio::time::sleep(tokio::time::Duration::from_micros(1)).await;
        }
    }

    pub fn as_stream(mut self) -> impl futures_core::stream::Stream<Item = anyhow::Result<Vec<u8>>> {
        stream! {
            loop {
                let data = self.pop();
                match data {
                    Ok(v) => {
                        if v.is_empty() {
                            if self.is_done() {
                                return;
                            } else {
                                // Not sure how to do this better.  I tried using a signal that data
                                // had changed (via tokio::sync::watch::channel()), but that effectively
                                // blocked - we got the first chunk quickly but then it stalled waiting
                                // for the change notification.  Polling is awful (and this interval is
                                // probably too aggressive) but I don't know how to get signalling
                                // to work!
                                tokio::time::sleep(tokio::time::Duration::from_micros(1)).await;

                                // For the record: this is what I tried:
                                // match self.write_index_receiver.changed().await {
                                //     Ok(_) => continue,
                                //     Err(e) => {
                                //         // If this ever happens (which it, cough, shouldn't), it means all senders have
                                //         // closed, which _should_ mean we are done.  Log the error
                                //         // but don't return it to the stream: the response as streamed so far
                                //         // _should_ be okay!
                                //         tracing::error!("StreamWriter::as_stream: error receiving write updates: {}", e);
                                //         return;
                                //     }
                                // }
                            }
                        } else {
                            yield Ok(v);
                        }
                    },
                    Err(e) => {
                        if self.is_done() {
                            return;
                        } else {
                            yield Err(e);
                            return;
                        }
                    },
                }
            }
        }
    }

    fn is_done(&self) -> bool {
        match self.done.read() {
            Ok(d) => *d,
            Err(_) => false,
        }
    }

    fn pop(&mut self) -> anyhow::Result<Vec<u8>> {
        let data = match self.pending.write().as_mut() {
            Ok(pending) => {
                let res = pending.clone();
                pending.clear();
                Ok(res)
            },
            Err(e) => {
                Err(anyhow::anyhow!("Internal error: StreamWriter::pop can't take lock: {}", e))
            }
        };
        data
    }
}

impl Write for StreamWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.append(buf).map_err(
            |e| std::io::Error::new(std::io::ErrorKind::Other, e)
        )?;

        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn split_at_two_newlines(source: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let mut buffer = vec![];
    let mut last: u8 = 0;
    for value in source {
        if *value == 10 && last == 10 {
            let rest_slice = &source[(buffer.len() + 1)..];
            let rest = Vec::from(rest_slice);
            return Some((buffer, rest));
        } else {
            buffer.push(*value);
            last = *value;
        }
    }
    None
}

#[cfg(test)]
mod test {
    use futures::StreamExt;

    use super::*;

    #[test]
    fn splits_at_two_newlines_if_pair_only() {
        let source: Vec<u8> = vec![0x41, 0x42, 0x0a, 0x0a, 0x43, 0x44];
        let result = split_at_two_newlines(&source).expect("did not split at all");
        assert_eq!(vec![0x41, 0x42, 0x0a], result.0);
        assert_eq!(vec![0x43, 0x44], result.1);
    }

    #[test]
    fn doesnt_splits_at_two_newlines_if_no_pair() {
        let source: Vec<u8> = vec![0x41, 0x42, 0x0a, 0x43, 0x44, 0x0a, 0x45, 0x46];
        let result = split_at_two_newlines(&source);
        assert_eq!(None, result);
    }

    #[test]
    fn splits_at_two_newlines_empty_rest_if_at_end() {
        let source: Vec<u8> = vec![0x41, 0x42, 0x0a, 0x43, 0x44, 0x0a, 0x0a];
        let result = split_at_two_newlines(&source).expect("did not split at all");
        assert_eq!(vec![0x41, 0x42, 0x0a, 0x43, 0x44, 0x0a], result.0);
        assert!(result.1.is_empty());
    }

    #[tokio::test]
    async fn streaming_splits_out_headers() {
        let mut sw = StreamWriter::new();
        let mut sw2 = sw.clone();
        tokio::spawn(async move {
            write!(sw2, "Header 1\n").unwrap();
            write!(sw2, "Header 2\n").unwrap();
            write!(sw2, "\n").unwrap();
            write!(sw2, "Body 1\n").unwrap();
            write!(sw2, "Body 2\n").unwrap();
            sw2.done().unwrap();
        });
        let header = sw.header_block().await.unwrap();
        let header_text = String::from_utf8(header).unwrap();
        assert!(header_text.contains("Header 1\n"));
        assert!(header_text.contains("Header 2\n"));

        let mut stm = Box::pin(sw.as_stream());
        let mut body = vec![];
        while let Some(Ok(v)) = stm.next().await {
            body.extend_from_slice(&v);
        }
        let body_text = String::from_utf8(body).unwrap();
        assert!(body_text.contains("Body 1\n"));
        assert!(body_text.contains("Body 2\n"));
    }
}
