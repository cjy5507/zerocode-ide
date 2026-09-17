//! Compatibility exports for the state-owned durable file primitives.

#[allow(unused_imports)]
pub(crate) use zerocode_shell_state::durable_file::{
    CommitOutcome, Durability, atomic_replace, ensure_private_directory,
    ensure_private_directory_durable, is_plain_directory, is_plain_file, open_plain_file,
    private_lock_file, read_plain_file, remove_directory, remove_file, replace_bytes,
    replace_staged, require_plain_directory_if_present, require_plain_file_if_present,
    sync_visible_parent,
};
