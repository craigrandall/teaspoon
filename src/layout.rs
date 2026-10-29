//! The names the archive layout is made of, in one place: the planner measures them against the
//! path budget, the renderers and the writer create them, and the body pipeline links to them.
//! Every length used for budgeting is derived from the name, so renaming one cannot leave a stale
//! number behind. Constants only; no I/O.

/// The metadata file in every folder directory and, at the archive root, the ownership marker.
pub(crate) const FOLDER_JSON: &str = "folder.json";

/// The rendered message in every message directory.
pub(crate) const MESSAGE_MD: &str = "message.md";

/// The metadata of one message.
pub(crate) const METADATA_JSON: &str = "metadata.json";

/// The directory, inside a message directory, that holds the message's extracted attachments and
/// its nested embedded messages.
pub(crate) const ATTACHMENTS_DIR: &str = "attachments";

/// UTF-16 code units in a layout name. The names are ASCII, so bytes and units agree; the test
/// below keeps it so.
pub(crate) const fn units_of(name: &str) -> usize {
    name.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_names_are_ascii_so_their_length_in_bytes_is_their_length_in_units() {
        for name in [FOLDER_JSON, MESSAGE_MD, METADATA_JSON, ATTACHMENTS_DIR] {
            assert!(name.is_ascii(), "{name}");
            assert_eq!(units_of(name), name.encode_utf16().count(), "{name}");
        }
    }

    #[test]
    fn the_names_are_the_documented_ones() {
        assert_eq!(FOLDER_JSON, "folder.json");
        assert_eq!(MESSAGE_MD, "message.md");
        assert_eq!(METADATA_JSON, "metadata.json");
        assert_eq!(ATTACHMENTS_DIR, "attachments");
    }
}
