use std::sync::{Arc, Mutex};
use config::*;
use jsonrpc_core::*;
use jsonrpc_core::futures::sync::mpsc::*;
use jsonrpc_core::futures::*;
use std::net::SocketAddr;
use std::time::*;
use std::sync::atomic::*;
use lru_time_cache::*;
use blocktemplate::*;

pub struct Miner {
  pub miner_id: String,
  pub login: String,
  pub password: String,
  pub peer_addr: SocketAddr,
  pub connection: Sender<String>,
  pub difficulty: AtomicUsize,
  pub jobs: Mutex<LruCache<String, Job>>,
  pub session_shares: AtomicUsize,
  pub session_start: SystemTime,
}

impl Miner {
  pub fn get_job(&self, job_provider: &Arc<JobProvider>) -> Result<Value> {
    // Notes on the block template:
    // - reserve_size (8) is the amount of bytes to reserve so the pool can throw in an extra nonce
    // - the daemon returns result.reserved_offset, and that many bytes into
    //   result.blocktemplate_blob, we can write our 8 byte extra nonce
    // - the node pools use a global counter, but we might want the counter to be per-miner
    // - it might not even be necessary to use any counters
    //   (and just go with the first 8 bytes of the miner id)
    if let Some(new_job) = job_provider.get_job(self.difficulty.load(Ordering::Relaxed) as u64) {
      let response = Ok(json!({
        "job_id": new_job.id,
        "blob": new_job.hashing_blob,
        "target": new_job.diff_hex,
      }));
      self.jobs.lock().unwrap().insert(new_job.id.to_owned(), new_job);
      return response;
    }
    Err(Error::internal_error())
  }

  pub fn adjust_difficulty(&self, new_shares: u64, config: &ServerConfig) {
    let total_shares = self.session_shares.fetch_add(new_shares as usize, Ordering::SeqCst) as u64;
    let secs_since_start = SystemTime::now().duration_since(self.session_start)
      .expect("Session start is in the future, this shouldn't happen")
      .as_secs();
    let buffer_seconds = 60 * 5;
    let buffer_shares = config.starting_difficulty * buffer_seconds;
    let miner_hashrate = (total_shares + buffer_shares) / (secs_since_start + buffer_seconds);
    let ideal_difficulty = miner_hashrate * config.target_time;
    let actual_difficulty = self.difficulty.load(Ordering::Relaxed) as f64;
    let difficulty_ratio = (ideal_difficulty as f64) / actual_difficulty;
    if (difficulty_ratio - 1.0).abs() > 0.25 {
      debug!("Adjusting miner to difficulty {}, address {}", ideal_difficulty, self.login);
      // Each time we get a new block template, the miners need new jobs anyways - so we just leave
      // the retargeting to that process.  Calling retarget_job here would be slightly tricky since
      // we don't want to interrupt an in-progress RPC call from the miner.
      self.difficulty.store(ideal_difficulty as usize, Ordering::Relaxed);
    }
  }

  pub fn retarget_job(&self, job_provider: &Arc<JobProvider>) {
    let miner_job = self.get_job(job_provider);
    if let Ok(miner_job) = miner_job {
      let job_to_send = serde_json::to_string(&json!({
          "jsonrpc": 2.0,
          "method": "job",
          "params": miner_job,
        }));
      let connection = self.connection.clone();
      if let &Ok(ref job) = &job_to_send {
        connection.send(job.to_owned())
          .poll();
      }
      if let Err(err) = job_to_send {
        debug!("Failed to write job to {}: {:?}", &self.peer_addr, err);
      }
    }
  }
}