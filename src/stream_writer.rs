use std::{convert::Infallible, io::Write, sync::{Arc, RwLock}};

use async_stream::stream;

#[derive(Clone)]
pub struct StreamWriter {
    pending: Arc<RwLock<Vec<u8>>>,
    done: Arc<RwLock<bool>>,
}

impl StreamWriter {
    pub fn new() -> Self {
        Self {
            pending: Arc::new(RwLock::new(vec![])),
            done: Arc::new(RwLock::new(false)),
        }
    }

    fn append(&mut self, buf: &[u8]) -> anyhow::Result<()> {
        match self.pending.write().as_mut() {
            Ok(pending) => {
                pending.extend_from_slice(buf);
                Ok(())
            },
            Err(e) =>
                Err(anyhow::anyhow!("Can't append to W2 buffer: {}", e))
        }
    }

    pub fn done(&mut self) -> anyhow::Result<()> {
        match self.done.write().as_deref_mut() {
            Ok(d) => {
                *d = true;
                println!("marked done");
                Ok(())
            },
            Err(e) =>
                Err(anyhow::anyhow!("Can't done the W2: {}", e))
        }
    }

    pub fn as_stream(mut self) -> impl futures_core::stream::Stream<Item = Result<Vec<u8>, Infallible>> {
        stream! {
            loop {
                let data = self.pop();
                match data {
                    Ok(v) => {
                        println!("yielding {} bytes", v.len());
                        if v.is_empty() {
                            if self.is_done() {
                                println!("doneburger");
                                return;
                            } else {
                                tokio::time::sleep(tokio::time::Duration::from_micros(20)).await;
                            }
                        } else {
                            yield Ok(v);
                        }
                    },
                    Err(e) => {
                        if self.is_done() {
                            println!("done!!!!");
                            return;
                        } else {
                            ()
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
                let res = Ok(pending.clone());
                pending.clear();
                res
            },
            Err(e) => {
                Err(anyhow::anyhow!("Error gaining write access: {}", e))
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
