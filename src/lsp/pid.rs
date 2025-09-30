use memchr::memmem::find;
use serde_json::{Value, json};
use sysinfo::{Pid, System};
use tokio_util::{bytes::Bytes, sync::CancellationToken};
use tracing::{debug, error, info, trace, warn};

pub struct PidHandler {
    pid: Option<u64>,
    cancel_provider: CancellationToken,
}

impl PidHandler {
    pub fn new(cancel_provider: CancellationToken) -> Self {
        Self {
            pid: None,
            cancel_provider,
        }
    }

    /// Take the processId parameter from the client and store it in the `pid` attribute; set it to null
    /// in the LSP request
    ///
    /// Patching the PID is necessary because if it is passed to an LSP located inside a Docker
    /// container, the LSP will try to detect the PID, and if it is missing inside the container,
    /// the LSP will close and break the pipe.
    pub fn try_take_initialize_process_id(
        &mut self,
        raw_bytes: &mut Bytes,
    ) -> serde_json::error::Result<bool> {
        if find(raw_bytes, br#""method":"initialize""#).is_some() {
            debug!("Initialize method found, patching");
            trace!(?raw_bytes, "before patch");

            let mut v: Value = serde_json::from_slice(&raw_bytes)?;
            if let Some(process_id) = v
                .get_mut("params")
                .and_then(|params| params.get_mut("processId"))
            {
                self.pid = process_id.as_u64();
                trace!(self.pid, "captured PID");
                *process_id = json!("null");
            }

            if let Some(vstr) = v.as_str() {
                *raw_bytes = Bytes::from(vstr.as_bytes().to_owned());
            } else {
                error!(%v ,"error converting to str");
            }

            trace!(?raw_bytes, "patched");
            return Ok(true);
        }
        trace!("Initialize method not found, skipping patch");
        return Ok(false);
    }

    /// Monitor periodically if the PID is running
    pub async fn monitor_pid(&self) {
        debug!(?self.pid, "Initializing PID monitoring");

        loop {
            if !self.is_pid_running() {
                info!(
                    "PID {:?} is not running; shutting down the process",
                    self.pid
                );
                self.cancel_provider.cancel();
                break;
            }
            trace!(?self.pid, "Is running");
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    }

    /// Check if the PID is running
    fn is_pid_running(&self) -> bool {
        if let Some(pid) = self.pid {
            trace!("Checking if PID {} is running", pid);

            let mut system = System::new_all();
            system.refresh_all();

            let target_pid = Pid::from_u32(pid as u32);
            system.process(target_pid).is_some()
        } else {
            warn!("No PID for capturing");
            // By default indicate that the IDE is running because
            // We can't know if it is running
            true
        }
    }
}
