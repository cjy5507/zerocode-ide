use super::PluginInstallSource;

pub(super) fn plugin_id(name: &str, marketplace: &str) -> String {
    format!("{name}@{marketplace}")
}

pub(super) fn sanitize_plugin_id(plugin_id: &str) -> String {
    plugin_id
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | '@' | ':' => '-',
            other => other,
        })
        .collect()
}

pub(super) fn describe_install_source(source: &PluginInstallSource) -> String {
    match source {
        PluginInstallSource::LocalPath { path } => path.display().to_string(),
        PluginInstallSource::GitUrl {
            url,
            reference: Some(reference),
        } => format!("{url}#{reference}"),
        PluginInstallSource::GitUrl {
            url,
            reference: None,
        } => url.clone(),
    }
}

pub(super) fn unix_time_ms() -> u128 {
    // A pre-epoch clock yields 0 instead of panicking — consistent with the
    // registry `now_secs` helpers across the workspace.
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
