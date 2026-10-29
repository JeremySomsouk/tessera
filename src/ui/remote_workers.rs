use std::thread::JoinHandle;

#[derive(Default)]
pub(super) struct Workers {
    handles: Vec<JoinHandle<()>>,
}

impl Workers {
    pub(super) fn spawn(&mut self, work: impl FnOnce() + Send + 'static) {
        self.reap();
        self.handles.push(std::thread::spawn(work));
    }

    pub(super) fn reap(&mut self) {
        let mut index = 0;
        while index < self.handles.len() {
            if self.handles[index].is_finished() {
                let _ = self.handles.swap_remove(index).join();
            } else {
                index += 1;
            }
        }
    }

    pub(super) fn finish(&mut self) {
        for handle in self.handles.drain(..) {
            let _ = handle.join();
        }
    }
}

impl Drop for Workers {
    fn drop(&mut self) {
        self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_runs::{CommentPreview, Gh, post_comment};
    use std::{
        path::PathBuf,
        sync::{Mutex, mpsc::channel},
    };

    struct Posting {
        ready: std::sync::mpsc::Sender<PathBuf>,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
        fail: bool,
    }

    impl Gh for Posting {
        fn run(&self, args: &[String]) -> Result<String, String> {
            let path = PathBuf::from(args.last().unwrap());
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                "Please rerun ACME-42."
            );
            self.ready.send(path).unwrap();
            self.release.lock().unwrap().recv().unwrap();
            if self.fail {
                Err("Request failed".into())
            } else {
                Ok(String::new())
            }
        }
    }

    #[test]
    fn app_owner_retains_confirmed_posts_after_panel_closure_and_waits_for_body_cleanup() {
        for fail in [false, true] {
            let mut workers = Workers::default();
            let (ready, created) = channel();
            let (release, wait) = channel();
            let (result, panel_receiver) = channel();
            workers.spawn(move || {
                let comment = CommentPreview::new("acme/api", 42, "Please rerun ACME-42.".into())
                    .unwrap()
                    .confirm();
                let gh = Posting {
                    ready,
                    release: Mutex::new(wait),
                    fail,
                };
                let _ = result.send(post_comment(&gh, &comment));
            });
            let path = created.recv().unwrap();
            drop(panel_receiver);
            workers.reap();
            assert_eq!(workers.handles.len(), 1);
            assert!(path.exists());
            release.send(()).unwrap();
            drop(workers);
            assert!(
                !path.exists(),
                "normal exit must wait for private body deletion"
            );
        }
    }

    #[test]
    fn panicking_worker_does_not_interrupt_cleanup_of_other_workers() {
        let mut workers = Workers::default();
        let (done, received) = channel();
        workers.spawn(|| panic!("fixture worker failure"));
        workers.spawn(move || {
            done.send(()).unwrap();
        });
        workers.finish();
        received.recv().unwrap();
        assert!(workers.handles.is_empty());
    }
}
