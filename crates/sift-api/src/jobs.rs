//! In-memory job registry.

use crate::error::ApiError;
use serde::Serialize;
use sift_core::ResultSet;
use sift_crawler::CrawlStats;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    Scrape,
    Crawl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Running,
    Completed,
    /// Cancelled while running; partial results are available.
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobSummary {
    pub id: Uuid,
    pub kind: JobKind,
    pub status: JobStatus,
    /// Seconds since the Unix epoch.
    pub created_at: u64,
    pub finished_at: Option<u64>,
    pub stats: CrawlStats,
    pub error: Option<String>,
}

struct Inner {
    summary: JobSummary,
    result: Option<Arc<ResultSet>>,
}

pub struct Job {
    pub id: Uuid,
    cancel: CancellationToken,
    inner: Mutex<Inner>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl Job {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().expect("job mutex poisoned")
    }

    pub fn summary(&self) -> JobSummary {
        self.lock().summary.clone()
    }

    pub fn set_stats(&self, stats: CrawlStats) {
        self.lock().summary.stats = stats;
    }

    pub fn finish(&self, rs: ResultSet) {
        let mut g = self.lock();
        g.summary.status = if rs.metadata.cancelled { JobStatus::Cancelled } else { JobStatus::Completed };
        g.summary.finished_at = Some(now());
        g.result = Some(Arc::new(rs));
    }

    pub fn fail(&self, message: String) {
        let mut g = self.lock();
        g.summary.status = JobStatus::Failed;
        g.summary.finished_at = Some(now());
        g.summary.error = Some(message);
    }

    pub fn results(&self) -> Result<Arc<ResultSet>, ApiError> {
        let g = self.lock();
        match (g.summary.status, &g.result) {
            (JobStatus::Running, _) => Err(ApiError::not_finished()),
            (JobStatus::Failed, _) => Err(ApiError::failed(g.summary.error.as_deref().unwrap_or("job failed"))),
            (_, Some(r)) => Ok(r.clone()),
            (_, None) => Err(ApiError::not_finished()),
        }
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    fn is_running(&self) -> bool {
        self.lock().summary.status == JobStatus::Running
    }
}

pub struct JobStore {
    jobs: Mutex<HashMap<Uuid, Arc<Job>>>,
    max_stored: usize,
}

impl JobStore {
    pub fn new(max_stored: usize) -> Self {
        Self { jobs: Mutex::default(), max_stored }
    }

    /// Register a new running job, evicting the oldest finished job if the store is full.
    pub fn create(&self, kind: JobKind, cancel: CancellationToken, max_running: usize) -> Result<Arc<Job>, ApiError> {
        let mut jobs = self.jobs.lock().expect("job store poisoned");
        let running = jobs.values().filter(|j| j.is_running()).count();
        if running >= max_running {
            return Err(ApiError::too_many_jobs(format!(
                "{running} jobs are already running (limit {max_running}); wait for one to finish or DELETE it"
            )));
        }
        if jobs.len() >= self.max_stored {
            let oldest_finished =
                jobs.values().filter(|j| !j.is_running()).min_by_key(|j| j.summary().created_at).map(|j| j.id);
            match oldest_finished {
                Some(id) => {
                    jobs.remove(&id);
                }
                None => return Err(ApiError::too_many_jobs("job store is full of running jobs")),
            }
        }
        let id = Uuid::new_v4();
        let job = Arc::new(Job {
            id,
            cancel,
            inner: Mutex::new(Inner {
                summary: JobSummary {
                    id,
                    kind,
                    status: JobStatus::Running,
                    created_at: now(),
                    finished_at: None,
                    stats: CrawlStats::default(),
                    error: None,
                },
                result: None,
            }),
        });
        jobs.insert(id, job.clone());
        Ok(job)
    }

    pub fn get(&self, id: Uuid) -> Option<Arc<Job>> {
        self.jobs.lock().expect("job store poisoned").get(&id).cloned()
    }

    pub fn remove(&self, id: Uuid) -> Option<Arc<Job>> {
        self.jobs.lock().expect("job store poisoned").remove(&id)
    }
}
