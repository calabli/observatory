//! Local scan job bookkeeping. Callers must authenticate and authorize before using it.
//! Jobs deliberately live only in memory: a missing job after restart is unknown,
//! never evidence of success. The future passed here must scan this machine only.
use serde::Serialize;
use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex},
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanJob {
    pub job_id: String,
    pub state: &'static str,
    pub imported: Option<u32>,
    pub errors: Vec<String>,
    pub started_at: i64,
    pub completed_at: Option<i64>,
}

#[derive(Default)]
struct Jobs {
    jobs: HashMap<String, ScanJob>,
    keys: HashMap<(String, String), String>,
    active: Option<String>,
}

/// A manager is bound to one backend. It does not replace the backend's scan lock.
#[derive(Default, Clone)]
pub struct ScanJobs(Arc<Mutex<Jobs>>);

impl ScanJobs {
    /// Reuse the same job for a retry or a concurrent compatible scan.
    /// Authentication and input validation belong to the caller.
    pub fn start<F, Fut>(&self, requester: &str, key: &str, scan: F) -> Result<ScanJob, String>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<(u32, Vec<String>), String>> + Send + 'static,
    {
        if requester.is_empty() || key.is_empty() || requester.len() > 128 || key.len() > 128 {
            return Err("Invalid scan request".into());
        }
        let request_key = (requester.to_owned(), key.to_owned());
        let mut state = self.0.lock().map_err(|_| "Scan state unavailable")?;
        // Keep retry keys for one day. Running jobs are never evicted.
        let cutoff = chrono::Utc::now().timestamp_millis() - 86_400_000;
        let active = state.active.clone();
        state
            .jobs
            .retain(|id, job| active.as_ref() == Some(id) || job.started_at >= cutoff);
        let retained: std::collections::HashSet<String> = state.jobs.keys().cloned().collect();
        state.keys.retain(|_, id| retained.contains(id));
        if let Some(id) = state.keys.get(&request_key) {
            return state
                .jobs
                .get(id)
                .cloned()
                .ok_or_else(|| "Scan state unavailable".into());
        }
        if state.keys.len() >= 4096 {
            return Err("Scan job capacity reached".into());
        }
        if let Some(id) = state.active.clone() {
            let job = state
                .jobs
                .get(&id)
                .cloned()
                .ok_or("Scan state unavailable")?;
            state.keys.insert(request_key, id);
            return Ok(job);
        }
        let job = ScanJob {
            job_id: uuid::Uuid::new_v4().to_string(),
            state: "scanning",
            imported: None,
            errors: Vec::new(),
            started_at: chrono::Utc::now().timestamp_millis(),
            completed_at: None,
        };
        state.keys.insert(request_key, job.job_id.clone());
        state.active = Some(job.job_id.clone());
        state.jobs.insert(job.job_id.clone(), job.clone());
        drop(state);
        let manager = self.clone();
        let id = job.job_id.clone();
        tokio::spawn(async move {
            // A panic or cancellation is not a success and must release the active slot.
            let result = tokio::spawn(async move { scan().await }).await;
            if let Ok(mut state) = manager.0.lock() {
                if let Some(job) = state.jobs.get_mut(&id) {
                    match result {
                        Ok(Ok((imported, errors))) => {
                            job.state = if errors.is_empty() {
                                "completed"
                            } else {
                                "partial_failure"
                            };
                            job.imported = Some(imported);
                            job.errors = errors;
                        }
                        _ => {
                            job.state = "failed";
                            job.errors = vec!["本机扫描失败".into()];
                        }
                    }
                    job.completed_at = Some(chrono::Utc::now().timestamp_millis());
                }
                if state.active.as_deref() == Some(&id) {
                    state.active = None;
                }
            }
        });
        Ok(job)
    }

    /// The caller must verify the requesting device can access this job.
    pub fn get(&self, requester: &str, key: &str) -> Option<ScanJob> {
        let state = self.0.lock().ok()?;
        let id = state.keys.get(&(requester.to_owned(), key.to_owned()))?;
        state.jobs.get(id).cloned()
    }
}
