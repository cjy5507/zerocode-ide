//! Logical input coordinates from Android's own display viewport metadata.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Geometry {
    pub size: (u32, u32),
    rotation: u32,
}

impl Geometry {
    /// The rotation the logical frame is in (0–3) — what a dump's own
    /// `rotation` is held against before a kept geometry is trusted.
    pub(super) const fn rotation(&self) -> u32 {
        self.rotation
    }
}

const UNAVAILABLE: &str = "Android has no unambiguous active default logical display";

/// `DisplayViewport::toString` in AOSP's `include/input/DisplayViewport.h`
/// writes the logical frame in its current rotation, independently of the
/// physical panel size. `input` motion events default to display 0.
pub(super) fn parse(dump: &str) -> Result<Geometry, String> {
    let mut display = None;
    for line in dump.lines() {
        let Some(viewport) = line.trim().strip_prefix("Viewport ") else {
            continue;
        };
        let Some((_, fields)) = viewport.split_once(": ") else {
            continue;
        };
        let Some(fields) = fields.strip_prefix("displayId=0, ") else {
            continue;
        };
        let frame = field(fields, "logicalFrame=[")
            .and_then(|rest| rest.split_once(']').map(|(frame, _)| frame))
            .and_then(logical_size);
        let rotation = field(fields, "orientation=")
            .and_then(|rest| rest.split(',').next())
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|rotation| *rotation < 4);
        if field(fields, "isActive=[") != Some("1]") {
            return Err(UNAVAILABLE.into());
        }
        let current = Geometry {
            size: frame.ok_or(UNAVAILABLE)?,
            rotation: rotation.ok_or(UNAVAILABLE)?,
        };
        // The dump can repeat a viewport in per-device diagnostics. Accept
        // agreeing witnesses, never whichever conflicting row came first.
        if display.is_some_and(|previous| previous != current) {
            return Err(UNAVAILABLE.into());
        }
        display = Some(current);
    }
    display.ok_or_else(|| UNAVAILABLE.into())
}

fn field<'a>(fields: &'a str, name: &str) -> Option<&'a str> {
    fields
        .match_indices(name)
        .find(|(index, _)| *index == 0 || fields[..*index].ends_with(", "))
        .map(|(index, _)| &fields[index + name.len()..])
}

fn logical_size(frame: &str) -> Option<(u32, u32)> {
    let mut coordinates = frame.split(',').map(str::trim);
    // The normalized input door covers the full default display, whose
    // logical origin is (0, 0). A cropped/offset viewport is not that proof.
    if coordinates.next()? != "0" || coordinates.next()? != "0" {
        return None;
    }
    let width = coordinates.next()?.parse::<u32>().ok()?;
    let height = coordinates.next()?.parse::<u32>().ok()?;
    (coordinates.next().is_none() && width > 1 && height > 1).then_some((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LANDSCAPE: &str = "Viewport INTERNAL: displayId=0, uniqueId=local:fixture, port=0, orientation=1, logicalFrame=[0, 0, 2400, 1080], physicalFrame=[0, 0, 1080, 2400], deviceSize=[1080, 2400], isActive=[1]";

    #[test]
    fn default_logical_frame_is_not_the_physical_size_or_another_display() {
        let external = LANDSCAPE.replace("displayId=0", "displayId=7");
        let dump = format!("{external}\n{LANDSCAPE}\n{LANDSCAPE}");
        assert_eq!(parse(&dump).unwrap().size, (2400, 1080));
    }

    #[test]
    fn absent_invalid_inactive_or_conflicting_geometry_is_refused() {
        for dump in [
            String::new(),
            "Physical size: 1080x2400\nOverride size: 900x1600".into(),
            LANDSCAPE.replace("displayId=0", "displayId=7"),
            LANDSCAPE.replace("isActive=[1]", "isActive=[0]"),
            LANDSCAPE.replace("orientation=1", "orientation=4"),
            LANDSCAPE.replace("[0, 0, 2400, 1080]", "[0, 0, 0, 1080]"),
            LANDSCAPE.replace("[0, 0, 2400, 1080]", "[1, 0, 2400, 1080]"),
            LANDSCAPE.replace("[0, 0, 2400, 1080]", "[0, 0, -1, 1080]"),
            LANDSCAPE.replace("[0, 0, 2400, 1080]", "[0, 0, 2400]"),
            LANDSCAPE.replace("[0, 0, 2400, 1080]", "[0, 0, 2400, 1080, 9]"),
            format!(
                "{LANDSCAPE}\n{}",
                LANDSCAPE.replace("2400, 1080", "1080, 2400")
            ),
        ] {
            assert!(parse(&dump).is_err(), "accepted {dump:?}");
        }
    }
}
