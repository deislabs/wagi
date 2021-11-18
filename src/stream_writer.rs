use std::{io::Write, sync::{Arc, RwLock}};

use async_stream::stream;

#[derive(Clone)]
pub struct StreamWriter {
    pending: Arc<RwLock<Vec<u8>>>,
    done: Arc<RwLock<bool>>,
    // A way for the write side to signal new data to the stream side
    write_index: Arc<RwLock<i64>>,
    write_index_sender: Arc<tokio::sync::watch::Sender<i64>>,
    write_index_receiver: tokio::sync::watch::Receiver<i64>,
}

impl StreamWriter {
    pub fn new() -> Self {
        let write_index = 0;
        let (tx, rx) = tokio::sync::watch::channel(write_index);
        Self {
            pending: Arc::new(RwLock::new(vec![])),
            done: Arc::new(RwLock::new(false)),
            write_index: Arc::new(RwLock::new(write_index)),
            write_index_sender: Arc::new(tx),
            write_index_receiver: rx,
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
        {
            let mut write_index = self.write_index.write().unwrap();
            *write_index = *write_index + 1;
            self.write_index_sender.send(*write_index).unwrap();
        }
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
                                // Not sure this is the smoothest way to do it. The oldest way was:
                                // tokio::time::sleep(tokio::time::Duration::from_micros(20)).await;
                                // which is a hideous kludge but subjectively felt quicker (but the
                                // number say not, so what is truth anyway)
                                match self.write_index_receiver.changed().await {
                                    Ok(_) => continue,
                                    Err(e) => {
                                        // If this ever happens (which it, cough, shouldn't), it means all senders have
                                        // closed, which _should_ mean we are done.  Log the error
                                        // but don't return it to the stream: the response as streamed so far
                                        // _should_ be okay!
                                        tracing::error!("StreamWriter::as_stream: error receiving write updates: {}", e);
                                        return;
                                    }
                                }
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
