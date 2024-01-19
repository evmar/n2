use std::{sync::{Arc, Mutex}, num::NonZeroUsize};


type Job<'a> = Box<dyn FnOnce() + Send + 'a>;

pub struct ThreadPoolExecutor<'a> {
    sender: std::sync::mpsc::Sender<Job<'a>>,
    num_threads: NonZeroUsize,
}

impl<'a> ThreadPoolExecutor<'a> {
    pub fn execute<F: FnOnce() -> () + Send + 'a>(&self, f: F) {
        let job = Box::new(f);
        self.sender.send(job).unwrap();
    }

    pub fn get_num_threads(&self) -> NonZeroUsize {
        self.num_threads
    }
}

pub fn scoped_thread_pool<'env, T, F: FnOnce(&ThreadPoolExecutor<'env>) -> T>(num_threads: NonZeroUsize, scope: F) -> T {
    std::thread::scope(|s| {
        let (sender, receiver) = std::sync::mpsc::channel::<Job>();
        let receiver = Arc::new(Mutex::new(receiver));

        let mut workers = Vec::with_capacity(num_threads.get());
        for _ in 0..num_threads.get() {
            let receiver = receiver.clone();
            workers.push(s.spawn(move || {
                loop {
                    let message = receiver.lock().unwrap().recv();
                    match message {
                        Ok(job) => job(),
                        Err(_) => break,
                    }
                }
            }));
        }

        let pool: ThreadPoolExecutor<'env> = ThreadPoolExecutor {
            sender,
            num_threads,
        };
        scope(&pool)
    })
}
