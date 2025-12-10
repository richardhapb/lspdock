use serde_json::{Value, json};
use tokio_util::bytes::Bytes;
use tracing::{debug, trace};

pub struct PidHandler {
    pid: Option<u64>,
}

impl PidHandler {
    pub fn new() -> Self {
        Self { pid: None }
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
    ) -> serde_json::error::Result<()> {
        debug!("Initialize method found, patching");
        trace!(?raw_bytes, "before patch");

        let mut v: Value = serde_json::from_slice(raw_bytes.as_ref())?;
        if let Some(process_id) = v.pointer_mut("/params/processId") {
            debug!(
                "The PID has been captured from the initialize method, setting pid_handler to None"
            );
            self.pid = process_id.as_u64();
            trace!(self.pid, "captured PID");
            *process_id = json!(null);
            *raw_bytes = Bytes::from(serde_json::to_vec(&v)?);
            trace!(?raw_bytes, "patched");
        }

        Ok(())
    }
}
