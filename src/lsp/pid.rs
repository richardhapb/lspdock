use tracing::{debug, trace};

/// Patch the processId parameter from the client.
///
/// Patching the PID is necessary because if it is passed to an LSP located inside a Docker
/// container, the LSP will try to detect the PID, and if it is missing inside the container,
/// the LSP will close and break the pipe.
pub fn patch_initialize_process_id(raw_str: &mut String) -> bool {
    if raw_str.contains(r#""method":"initialize""#) {
        debug!("Initialize method found, patching");
        trace!(%raw_str, "before patch");

        let re = regex::Regex::new(r#""processId":\s*\d+"#).expect("compile regex");
        *raw_str = re.replace_all(raw_str, r#""processId":null"#).to_string();

        trace!(%raw_str, "patched");
        return true;
    }
    return false;
}


