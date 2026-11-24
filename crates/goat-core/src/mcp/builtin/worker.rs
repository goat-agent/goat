use std::sync::mpsc;

use tokio::sync::oneshot;

type Job<P> = Box<dyn FnOnce(&mut P) + Send>;

pub(crate) struct Worker<P: ?Sized> {
    name: &'static str,
    jobs: mpsc::Sender<Job<P>>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("the {name} thread did not start")]
    #[cfg_attr(
        all(not(target_os = "macos"), not(test)),
        expect(dead_code, reason = "only macOS adapters start a worker so far")
    )]
    NotStarted {
        name: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("the {0} thread stopped")]
    Stopped(&'static str),
}

impl<P: ?Sized + 'static> Worker<P> {
    #[cfg_attr(
        all(not(target_os = "macos"), not(test)),
        expect(dead_code, reason = "only macOS adapters start a worker so far")
    )]
    pub(crate) fn start<E: From<Error> + Send + 'static>(
        name: &'static str,
        make: impl FnOnce() -> Result<Box<P>, E> + Send + 'static,
    ) -> Result<Self, E> {
        let (jobs, inbox) = mpsc::channel::<Job<P>>();
        let (ready, started) = mpsc::channel();
        std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                let mut part = match make() {
                    Ok(part) => {
                        ready.send(Ok(())).ok();
                        part
                    }
                    Err(err) => {
                        ready.send(Err(err)).ok();
                        return;
                    }
                };
                for job in inbox {
                    job(&mut part);
                }
            })
            .map_err(|source| Error::NotStarted { name, source })?;
        started.recv().map_err(|_| Error::Stopped(name))??;
        Ok(Self { name, jobs })
    }

    pub(crate) async fn run<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut P) -> T + Send + 'static,
    ) -> Result<T, Error> {
        let (reply, answer) = oneshot::channel();
        self.jobs
            .send(Box::new(move |part| {
                reply.send(work(part)).ok();
            }))
            .map_err(|_| Error::Stopped(self.name))?;
        answer.await.map_err(|_| Error::Stopped(self.name))
    }
}

impl<P: ?Sized> std::fmt::Debug for Worker<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Worker")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}
