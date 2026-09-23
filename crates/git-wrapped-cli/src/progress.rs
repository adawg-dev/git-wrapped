use std::{
    cell::Cell,
    io::{self, IsTerminal},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

#[derive(Clone, Default)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    pub fn check(&self) -> Result<(), String> {
        if self.is_cancelled() {
            Err("cancelled".into())
        } else {
            Ok(())
        }
    }

    pub fn install_ctrlc(&self) -> Result<(), String> {
        let flag = self.clone();
        ctrlc::set_handler(move || flag.cancel())
            .map_err(|error| format!("install Ctrl+C handler: {error}"))
    }
}

pub struct Progress {
    enabled: bool,
    last: Cell<Option<Instant>>,
}

impl Progress {
    pub fn new(verbose: bool) -> Self {
        Self {
            enabled: verbose || io::stderr().is_terminal(),
            last: Cell::new(None),
        }
    }

    pub fn phase(&self, name: &str, done: Option<u64>, total: Option<u64>) {
        if !self.enabled {
            return;
        }
        let now = Instant::now();
        if self
            .last
            .get()
            .is_some_and(|last| now.duration_since(last) < Duration::from_millis(250))
        {
            return;
        }
        self.last.set(Some(now));
        let name: String = name
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        match (done, total) {
            (Some(done), Some(total)) => eprintln!("Progress: {name} ({done}/{total})"),
            (Some(done), None) => eprintln!("Progress: {name} ({done})"),
            _ => eprintln!("Progress: {name}"),
        }
    }
}
