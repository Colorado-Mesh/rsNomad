//! Minimal Micron helpers for default and error pages.

/// Placeholder `index.mu` when the host has no pages yet.
pub fn default_index_page(display_name: &str) -> String {
    let name = if display_name.trim().is_empty() {
        "Nomad node"
    } else {
        display_name.trim()
    };
    format!(
        "#!c=30\n\
         > {name}\n\
         \n\
         This node is hosted by `rsNomad` (Rust Nomad Network for Reticulum).\n\
         \n\
         Add pages under `pages/` to replace this placeholder.\n"
    )
}

/// Micron 404 body for a missing page route.
pub fn not_found_page(route: &str) -> String {
    format!(
        "#!c=0\n\
         > Not found\n\
         \n\
         The page `{route}` is not available on this node.\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_index_includes_name() {
        let page = default_index_page("Test Node");
        assert!(page.contains("Test Node"));
        assert!(page.starts_with("#!c="));
    }

    #[test]
    fn not_found_includes_route() {
        let page = not_found_page("/page/missing.mu");
        assert!(page.contains("/page/missing.mu"));
    }
}
