use anyhow::{bail, Result};

pub struct CapabilityIndex;

impl CapabilityIndex {
    pub fn new() -> Self {
        Self
    }

    pub fn find_room(&self, _required_caps: &[String]) -> Option<String> {
        None
    }

    pub fn get_or_create_room(&self, _required_caps: &[String]) -> Result<String> {
        bail!("capability index not yet implemented")
    }
}

impl Default for CapabilityIndex {
    fn default() -> Self {
        Self::new()
    }
}
