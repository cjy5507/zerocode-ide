use std::fmt;

use zeroize::Zeroizing;

/// One caller-supplied password whose buffer is zeroized when the connection
/// attempt returns.
///
/// The type is intentionally neither cloneable nor serializable. See the crate
/// documentation for the lifetime of russh's protocol-internal copy.
pub struct SshPassword(Zeroizing<String>);

impl SshPassword {
    #[must_use]
    pub fn new(password: String) -> Self {
        Self(Zeroizing::new(password))
    }

    pub(crate) fn expose(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for SshPassword {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SshPassword([REDACTED])")
    }
}
