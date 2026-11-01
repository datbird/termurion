//! No coprocess on Unix yet. `start` reports Unsupported, so a `Coprocess` can never exist.

use std::sync::Arc;

use crate::platform::Unsupported;

pub struct Coprocess {
    pub command: String,
    /// Run Silent Coprocess: the session's output is not drawn while it runs.
    pub mute: bool,
    never: std::convert::Infallible,
}

impl Coprocess {
    pub fn start(_command: &str, _mute: bool, _cwd: &std::path::Path, _notify: Arc<dyn Fn() + Send + Sync>) -> Result<Coprocess, String> {
        Err(Unsupported { feature: "Coprocesses" }.into())
    }

    pub fn pid(&self) -> u32 {
        match self.never {}
    }

    pub fn write(&mut self, _bytes: &[u8]) {
        match self.never {}
    }

    pub fn dropped(&self) -> usize {
        match self.never {}
    }

    pub fn try_output(&mut self) -> Option<Vec<u8>> {
        match self.never {}
    }

    pub fn finished(&mut self) -> bool {
        match self.never {}
    }

    pub fn stderr(&self) -> String {
        match self.never {}
    }

    pub fn close_input(&mut self) {
        match self.never {}
    }

    pub fn stop(&mut self) {
        match self.never {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_coprocess_start_is_unsupported() {
        let e = Coprocess::start("cat", false, std::path::Path::new("."), Arc::new(|| {})).err().unwrap();
        assert!(e.contains("Coprocesses is not available"), "{e}");
    }
}
