#[allow(unused_imports)]
pub mod archive;
pub mod extract;
pub mod fomod;
pub mod heuristics;
pub mod install;
pub mod links;
pub mod paths;
pub mod types;

// Flatten the public API so call sites don't change
pub use types::*;
#[allow(unused_imports)]
pub use install::{install_mod_from_archive, install_mod_from_archive_with_nexus,
                  install_mod_from_archive_with_nexus_ticking};
#[allow(unused_imports)]
pub use fomod::{parse_fomod_from_archive, parse_fomod_from_zip, resolve_file_conflicts};
#[allow(unused_imports)]
pub use heuristics::{detect_strategy, find_data_root_in_paths, is_bain_archive};
#[allow(unused_imports)]
pub use archive::{read_archive_files_bytes, read_archive_file_bytes,
                  list_archive_entries_with_7z};
#[allow(unused_imports)]
pub use links::determine_link_type;

#[cfg(test)]
mod tests;
