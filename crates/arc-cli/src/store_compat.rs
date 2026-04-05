use arc_algebra::BlobStore;
use arc_algebra::apply::{BlameState, MaterializedState};
use arc_algebra_types::Blake3Hash;
use arc_change::Change;
use arc_store_cas::ObjectStore;
use ignore::gitignore::Gitignore;

struct CliBlobStore<'a>(&'a ObjectStore);

impl BlobStore for CliBlobStore<'_> {
    fn read_blob(&self, hash: &Blake3Hash) -> Result<Vec<u8>, String> {
        self.0
            .read_blob(hash)
            .map(|bytes| bytes.to_vec())
            .map_err(|e| e.to_string())
    }

    fn contains_blob(&self, hash: &Blake3Hash) -> bool {
        self.0.contains_blob(hash)
    }
}

pub(crate) fn apply_change(
    state: &mut MaterializedState,
    change: &Change,
    store: &ObjectStore,
    agent_ignore: &Gitignore,
    blame: Option<&mut BlameState>,
) -> Result<(), String> {
    arc_algebra::apply::apply_change(state, change, &CliBlobStore(store), agent_ignore, blame)
}

pub(crate) trait ObjectStoreChangeExt {
    fn write_change(&self, change: &Change) -> Result<Blake3Hash, String>;
    fn read_change(&self, hash: &Blake3Hash) -> Result<Change, String>;
}

impl ObjectStoreChangeExt for ObjectStore {
    fn write_change(&self, change: &Change) -> Result<Blake3Hash, String> {
        let bytes = bincode::serialize(change).map_err(|e| e.to_string())?;
        self.write_object(&change.id, &bytes)
            .map_err(|e| e.to_string())
    }

    fn read_change(&self, hash: &Blake3Hash) -> Result<Change, String> {
        let bytes = self.read_object(hash).map_err(|e| e.to_string())?;
        bincode::deserialize(&bytes).map_err(|e| e.to_string())
    }
}
