//! The three names the artifact carries: its single top-level directory, its
//! file name, and its checksum sidecar line.

/// The archive's single top-level directory, so an extraction cannot scatter
/// files into whatever directory it was run from.
pub(in crate::cli::web::builds) fn top_level(product: &str, version: &str) -> String {
    format!("{product}-{version}")
}

/// The staged tarball's file name, which the recipe's `stage` map names
/// verbatim. One function so the sidecar and the path can never disagree.
pub(in crate::cli::web::builds) fn tarball_name(product: &str) -> String {
    format!("{product}-web.tar.gz")
}

/// The `sha256sum -c`-readable sidecar line: digest, two spaces, file name.
pub(in crate::cli::web::builds) fn sidecar_line(digest: &str, file_name: &str) -> String {
    format!("{digest}  {file_name}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_paths_are_named_after_the_product_and_version() {
        assert_eq!(top_level("preferences", "1.4.0"), "preferences-1.4.0");
        assert_eq!(tarball_name("preferences"), "preferences-web.tar.gz");
    }

    #[test]
    fn the_sidecar_line_is_sha256sum_readable() {
        let digest = "e".repeat(64);
        assert_eq!(
            sidecar_line(&digest, "preferences-web.tar.gz"),
            format!("{digest}  preferences-web.tar.gz\n")
        );
    }
}
