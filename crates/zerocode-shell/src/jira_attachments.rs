//! Compatibility exports for state-owned Jira attachment policy.

#[allow(unused_imports)]
pub(crate) use zerocode_shell_state::jira_attachments::{
    COUNT_MAX, DIR_NAME, FILE_LIMIT_BYTES, MIME_ALLOW_EXACT, MIME_ALLOW_PREFIXES,
    PREVIEW_LIMIT_BYTES, TOTAL_LIMIT_BYTES, content_file, image_data_uri, issue_dir, mime_allowed,
    sniff_image, store_bytes, thumbnail_file,
};
