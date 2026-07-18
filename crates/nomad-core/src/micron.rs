//! Minimal Micron helpers for default and error pages.

/// Max characters kept when interpolating operator-controlled text into Micron.
pub const MAX_MICRON_TEXT_CHARS: usize = 128;

/// Sanitize text interpolated into Micron pages (display names, routes).
///
/// Strips control characters, replaces backticks (Micron code spans), and
/// truncates length so operator-controlled strings cannot inject directives.
pub fn sanitize_micron_text(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .map(|c| if c == '`' { '\'' } else { c })
        .take(MAX_MICRON_TEXT_CHARS)
        .collect()
}

/// Placeholder `index.mu` when the host has no pages yet.
pub fn default_index_page(display_name: &str) -> String {
    let name = sanitize_micron_text(display_name.trim());
    let name = if name.is_empty() {
        "Nomad node".to_string()
    } else {
        name
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
    let route = sanitize_micron_text(route);
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

    #[test]
    fn sanitize_strips_backticks_and_controls() {
        assert_eq!(sanitize_micron_text("a`b\nc"), "a'bc");
        assert!(sanitize_micron_text(&"x".repeat(200)).len() == MAX_MICRON_TEXT_CHARS);
    }
}
