//! Desktop-browser routing at the window, before any helper request.

#[cfg(test)]
pub(super) mod tests;

#[cfg(test)]
fn registered_browser(_bundle_id: &str, _handlers: &[Vec<String>; 4]) -> Option<bool> {
    None
}
