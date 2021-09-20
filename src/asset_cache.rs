use std::path::{Path, PathBuf};

pub struct Cache {
    cache_path: PathBuf,
}

impl Cache {
    pub fn new(cache_path: impl AsRef<Path>) -> Self {
        Self {
            cache_path: cache_path.as_ref().to_path_buf()
        }
    }

    fn asset_path(&self) -> PathBuf {
        self.cache_path.join("_ASSETS")
    }
}
