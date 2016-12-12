use std::thread::{self, JoinHandle};
use std::sync::mpsc::{channel, Sender, Receiver, sync_channel};
use std::sync::{Arc, Mutex};
use std::marker::PhantomData;

trait FnBox {
    fn call_box(self: Box<Self>);
}

impl <F: FnOnce()> FnBox for F {
    fn call_box(self: Box<F>) {
        (*self)()
    }
}

type Thunk<'a> = Box<FnBox + Send + 'a>;

enum Message {
    Job(Thunk<'static>),
    Join,
}

struct ThreadData {
    join_handle: JoinHandle<()>,
    pool_sync_receiver: Receiver<()>,
}

struct Pool {
    is_serial: bool,
    threads: Vec<ThreadData>,
    job_sender: Option<Sender<Message>>,
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.job_sender = None;
    }
}

impl Pool {
    fn serial_queue() -> Pool {
        Pool::new(1)
    }

    fn new(n: usize) -> Pool {
        assert!(n >= 1);

        let (job_sender, job_receiver) = channel();
        let job_receiver = Arc::new(Mutex::new(job_receiver));

        let mut threads = Vec::with_capacity(n);

        for _ in 0..n {
            let job_receiver = job_receiver.clone();

            let (pool_sync_tx, pool_sync_rx) = sync_channel::<()>(0);

            let thread = thread::spawn(move || {
                loop {
                    let message = {
                        let lock = job_receiver.lock().unwrap();
                        lock.recv()
                    };

                    match message {
                        Ok(Message::Job(task)) => {
                            task.call_box();
                        },
                        Ok(Message::Join) => {
                            if pool_sync_tx.send(()).is_err() {
                                break;
                            }
                        },
                        Err(_) => {
                            break;
                        } 
                    }
                }
            });
            
            threads.push(ThreadData {
                join_handle: thread,
                pool_sync_receiver: pool_sync_rx,
            });
        }

        let is_serial = match n {
            1 => true,
            _ => false,
        };
        
        Pool {
            is_serial: is_serial,
            threads: threads,
            job_sender: Some(job_sender),
        }
    }    
}
