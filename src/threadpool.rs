#![allow(dead_code)]

use std::thread::{self, JoinHandle};
use std::sync::mpsc::{channel, Sender, Receiver, sync_channel, SyncSender};
use std::sync::{Arc, Mutex};
use std::marker::PhantomData;
use std::mem;

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
    thread_sync_sender: SyncSender<()>,
}

struct Pool {
    threads: Vec<ThreadData>,
    job_sender: Option<Sender<Message>>,
}

impl Drop for Pool {
    fn drop(&mut self) {
        // println!("Dropping Pool");
        self.job_sender = None;
    }
}

impl Pool {
    fn new(n: usize) -> Pool {
        assert!(n >= 1);

        let (job_sender, job_receiver) = channel();
        let job_receiver = Arc::new(Mutex::new(job_receiver));

        let mut threads = Vec::with_capacity(n);

        for _ in 0..n {
            let job_receiver = job_receiver.clone();

            let (pool_sync_tx, pool_sync_rx) = sync_channel::<()>(0);
            let (thread_sync_tx, thread_sync_rx) = sync_channel::<()>(0);

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

                            if thread_sync_rx.recv().is_err() {
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
                thread_sync_sender: thread_sync_tx,
            });
        }
        
        Pool {
            threads: threads,
            job_sender: Some(job_sender),
        }
    }

    fn scoped<'pool, 'scope, F, R>(&'pool mut self, f: F) -> R
        where F: FnOnce(&Scope<'pool, 'scope>) -> R {
        let scope = Scope {
            pool: self,
            _marker: PhantomData,
        };
        f(&scope)
    }
}

struct Scope<'pool, 'scope> {
    pool: &'pool mut Pool,
    _marker: PhantomData<&'scope ()>,
}

impl<'pool, 'scope> Scope<'pool, 'scope> {
    fn execute<F>(&self, f: F) where F: FnOnce() + Send + 'scope {
        let ctb = unsafe {
            mem::transmute::<Thunk<'scope>, Thunk<'static>>(Box::new(f))
        };
        self.pool.job_sender.as_ref().unwrap().send(Message::Job(ctb)).unwrap();
    }

    fn join_all(&self) {
        for _ in 0..self.pool.threads.len() {
            self.pool.job_sender.as_ref().unwrap().send(Message::Join).unwrap();
        }

        let mut panic = false;
        for thread_data in self.pool.threads.iter() {
            if let Err(_) = thread_data.pool_sync_receiver.recv() {
                panic = true;
            }
        }

        if panic {
            panic!("Threaed pool worker panicked");
        }

        for thread_data in self.pool.threads.iter() {
            thread_data.thread_sync_sender.send(()).unwrap();
        }
    }
}

impl<'pool, 'scope> Drop for Scope<'pool, 'scope> {
    fn drop(&mut self) {
        println!("Dropping scope");
        self.join_all();
    }
}

#[cfg(test)]
mod tests {
    #![allow(unused_variables)]
    use super::Pool;

    #[test]
    fn sometest() {
        let mut pool = Pool::new(4);
        for i in 1..7 {
            let mut vec = vec![0, 1, 2, 3, 4];
            pool.scoped(|s| {
                for e in vec.iter_mut() {
                    s.execute(move || {
                        *e += i;
                    });
                }
            });
            println!("hello");
            let mut vec2 = vec![0, 1, 2, 3, 4];
            for e in vec2.iter_mut() {
                *e += i;
            }

            assert_eq!(vec, vec2);
        }
    }

    #[test]
    #[should_panic]
    fn thread_panic() {
        let mut pool = Pool::new(4);
        pool.scoped(|scoped| {
            scoped.execute(move || {
                panic!();
            });
        });
    }

    #[test]
    #[should_panic]
    fn scope_panic() {
        let mut pool = Pool::new(4);
        pool.scoped(|_| {
            panic!();
        });
    }

    #[test]
    #[should_panic]
    fn pool_panic() {
        let pool = Pool::new(4);
        panic!();
    }

    fn sleep_ms(ms: u64) {
        use std::thread;
        use std::time;
        thread::sleep(time::Duration::from_millis(ms));
    }

    #[test]
    fn join_all() {
        use std::sync::mpsc::channel;
        
        let mut pool = Pool::new(4);
        let (tx_, rx) = channel();

        pool.scoped(|s| {
            let tx = tx_.clone();
            s.execute(move || {
                sleep_ms(2000);
                tx.send(2).unwrap();
            });

            let tx = tx_.clone();
            s.execute(move || {
                tx.send(1).unwrap();
            });

            s.join_all();

            let tx = tx_.clone();
            s.execute(move || {
                tx.send(4).unwrap();
            });

            assert_eq!(rx.iter().take(3).collect::<Vec<_>>(), vec![1, 2, 4]);
        });
    }

    #[test]
    fn join_all_with_thread_panic() {
        use std::sync::mpsc::channel;
        use std::sync::mpsc::Sender;
        use std::thread;
        
        struct OnScopeEnd(Sender<u8>);
        impl Drop for OnScopeEnd {
            fn drop(&mut self)  {
                self.0.send(1).unwrap();
                sleep_ms(2000);
            }
        }

        let (tx_, rx) = channel();

        let handle = thread::spawn(move || {
            let mut pool = Pool::new(5);
            let _on_scope_end = OnScopeEnd(tx_.clone());
            pool.scoped(|scope| {
                scope.execute(move || {
                    sleep_ms(100);
                    panic!();
                });

                for _ in 1..8 {
                    let tx = tx_.clone();
                    scope.execute(move || {
                        sleep_ms(200);
                        tx.send(0).unwrap();
                    });
                }                
            });
        });

        if let Ok(..) = handle.join() {
            panic!("Pool did not panic as expected");
        }

        let values: Vec<u8> = rx.into_iter().collect();
        assert_eq!(&values[..], &[0, 0, 0, 0, 0, 0, 0, 1]);
    }

}
