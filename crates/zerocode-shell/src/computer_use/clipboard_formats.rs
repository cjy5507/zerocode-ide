//! Which Windows clipboard formats can be copied out and put back by a
//! process that does not own them, kept away from the calls so a machine
//! without the platform can still prove the table.
//!
//! A format's data is one of three things. A movable global block
//! (`HGLOBAL`) — every registered format, the text and image standards,
//! file drops — is bytes: `GlobalSize` + `GlobalLock` read them, and a fresh
//! block of the same bytes puts them back. A GDI handle (`CF_BITMAP`,
//! `CF_PALETTE`, the metafile pair, the `CF_GDIOBJ*` range) is an object
//! whose copy would need GDI to duplicate it, and Windows synthesizes the
//! `HGLOBAL` twin of the two that matter (`CF_DIB` for a bitmap) so those are
//! preserved through the twin instead. And an owner-only format
//! (`CF_OWNERDISPLAY`, the `CF_PRIVATE*` range) is meaningful to the owning
//! process alone: it cannot be copied, and a clipboard carrying one must not
//! be emptied by anyone who cannot restore it.

/// How a format's data can be handled by a process that does not own it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FormatClass {
    /// Bytes in a global block: copied out and put back exactly.
    Global,
    /// A GDI object; its global twin (when Windows synthesizes one) carries
    /// the same picture and is preserved in its place.
    GdiHandle,
    /// Meaningful only to the owner: not copyable, so not preservable.
    OwnerOnly,
}

const CF_TEXT: u32 = 1;
const CF_BITMAP: u32 = 2;
const CF_METAFILEPICT: u32 = 3;
const CF_SYLK: u32 = 4;
const CF_DIF: u32 = 5;
const CF_TIFF: u32 = 6;
const CF_OEMTEXT: u32 = 7;
const CF_DIB: u32 = 8;
const CF_PALETTE: u32 = 9;
const CF_PENDATA: u32 = 10;
const CF_RIFF: u32 = 11;
const CF_WAVE: u32 = 12;
const CF_UNICODETEXT: u32 = 13;
const CF_ENHMETAFILE: u32 = 14;
const CF_HDROP: u32 = 15;
const CF_LOCALE: u32 = 16;
const CF_DIBV5: u32 = 17;
const CF_OWNERDISPLAY: u32 = 0x80;
const CF_DSPTEXT: u32 = 0x81;
const CF_DSPBITMAP: u32 = 0x82;
const CF_DSPMETAFILEPICT: u32 = 0x83;
const CF_DSPENHMETAFILE: u32 = 0x8E;
const CF_PRIVATEFIRST: u32 = 0x200;
const CF_PRIVATELAST: u32 = 0x2FF;
const CF_GDIOBJFIRST: u32 = 0x300;
const CF_GDIOBJLAST: u32 = 0x3FF;

/// The format `paste-text` writes.
pub(super) const UNICODE_TEXT: u32 = CF_UNICODETEXT;

#[must_use]
pub(super) fn classify(format: u32) -> FormatClass {
    match format {
        CF_BITMAP | CF_METAFILEPICT | CF_PALETTE | CF_ENHMETAFILE | CF_DSPBITMAP
        | CF_DSPMETAFILEPICT | CF_DSPENHMETAFILE => FormatClass::GdiHandle,
        CF_OWNERDISPLAY => FormatClass::OwnerOnly,
        CF_PRIVATEFIRST..=CF_PRIVATELAST => FormatClass::OwnerOnly,
        CF_GDIOBJFIRST..=CF_GDIOBJLAST => FormatClass::GdiHandle,
        _ => FormatClass::Global,
    }
}

/// The GDI formats whose picture Windows also offers as a global block,
/// so leaving the handle behind loses nothing once the twin is saved.
#[must_use]
pub(super) fn global_twin(format: u32) -> Option<u32> {
    match format {
        CF_BITMAP | CF_DSPBITMAP => Some(CF_DIB),
        _ => None,
    }
}

/// The standard name of a predefined format; registered formats have their
/// own names, read from the system.
#[must_use]
pub(super) fn standard_name(format: u32) -> Option<&'static str> {
    Some(match format {
        CF_TEXT => "CF_TEXT",
        CF_BITMAP => "CF_BITMAP",
        CF_METAFILEPICT => "CF_METAFILEPICT",
        CF_SYLK => "CF_SYLK",
        CF_DIF => "CF_DIF",
        CF_TIFF => "CF_TIFF",
        CF_OEMTEXT => "CF_OEMTEXT",
        CF_DIB => "CF_DIB",
        CF_PALETTE => "CF_PALETTE",
        CF_PENDATA => "CF_PENDATA",
        CF_RIFF => "CF_RIFF",
        CF_WAVE => "CF_WAVE",
        CF_UNICODETEXT => "CF_UNICODETEXT",
        CF_ENHMETAFILE => "CF_ENHMETAFILE",
        CF_HDROP => "CF_HDROP",
        CF_LOCALE => "CF_LOCALE",
        CF_DIBV5 => "CF_DIBV5",
        CF_OWNERDISPLAY => "CF_OWNERDISPLAY",
        CF_DSPTEXT => "CF_DSPTEXT",
        CF_DSPBITMAP => "CF_DSPBITMAP",
        CF_DSPMETAFILEPICT => "CF_DSPMETAFILEPICT",
        CF_DSPENHMETAFILE => "CF_DSPENHMETAFILE",
        _ => return None,
    })
}

/// Whether a clipboard holding exactly `formats` can be emptied and put
/// back without losing anything: every GDI handle on it has its global twin
/// there too, and nothing on it is owner-only. This is the decision
/// `paste-text` makes before it touches the clipboard at all.
#[must_use]
pub(super) fn fully_preservable(formats: &[u32]) -> bool {
    formats.iter().all(|format| match classify(*format) {
        FormatClass::Global => true,
        FormatClass::OwnerOnly => false,
        FormatClass::GdiHandle => global_twin(*format).is_some_and(|twin| formats.contains(&twin)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_blocks_are_the_rule_and_handles_the_exceptions() {
        for format in [
            CF_TEXT,
            CF_UNICODETEXT,
            CF_DIB,
            CF_DIBV5,
            CF_HDROP,
            CF_LOCALE,
            CF_OEMTEXT,
            0xC0F3, // a registered format such as "HTML Format"
            0xFFFF,
        ] {
            assert_eq!(classify(format), FormatClass::Global, "{format:#x}");
        }
        for format in [
            CF_BITMAP,
            CF_PALETTE,
            CF_METAFILEPICT,
            CF_ENHMETAFILE,
            CF_DSPBITMAP,
            CF_DSPMETAFILEPICT,
            CF_DSPENHMETAFILE,
            CF_GDIOBJFIRST,
            CF_GDIOBJLAST,
        ] {
            assert_eq!(classify(format), FormatClass::GdiHandle, "{format:#x}");
        }
        for format in [CF_OWNERDISPLAY, CF_PRIVATEFIRST, 0x2AB, CF_PRIVATELAST] {
            assert_eq!(classify(format), FormatClass::OwnerOnly, "{format:#x}");
        }
        assert_eq!(standard_name(CF_UNICODETEXT), Some("CF_UNICODETEXT"));
        assert_eq!(standard_name(0xC0F3), None);
        assert_eq!(UNICODE_TEXT, 13);
    }

    /// What `paste-text` may and may not empty: text with HTML and RTF, a
    /// file drop, a bitmap whose DIB twin is there — yes; an enhanced
    /// metafile alone, an owner-display painter, a private format — never.
    #[test]
    fn a_clipboard_is_emptied_only_when_every_format_comes_back() {
        assert!(fully_preservable(&[
            CF_UNICODETEXT,
            CF_LOCALE,
            CF_TEXT,
            0xC0F3
        ]));
        assert!(fully_preservable(&[CF_HDROP, 0xC0A1]));
        assert!(fully_preservable(&[CF_BITMAP, CF_DIB, CF_DIBV5]));
        assert!(fully_preservable(&[]));
        assert!(
            !fully_preservable(&[CF_BITMAP]),
            "a bitmap with no DIB twin"
        );
        assert!(!fully_preservable(&[CF_ENHMETAFILE, CF_METAFILEPICT]));
        assert!(!fully_preservable(&[CF_UNICODETEXT, CF_OWNERDISPLAY]));
        assert!(!fully_preservable(&[CF_UNICODETEXT, CF_PRIVATEFIRST + 3]));
        assert_eq!(global_twin(CF_BITMAP), Some(CF_DIB));
        assert_eq!(global_twin(CF_ENHMETAFILE), None);
    }
}
