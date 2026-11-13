//! Workspace library crate boilerplate.

/// Returns a greeting for the given name.
pub fn greet(name: &str) -> String {
    format!("Hello, {name}!")
}

#[cfg(feature = "extra")]
/// Returns an extra greeting when the `extra` feature is enabled.
pub fn extra_greet(name: &str) -> String {
    format!("Hello again, {name}!")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greet_works() {
        assert_eq!(greet("world"), "Hello, world!");
    }

    #[cfg(feature = "extra")]
    #[test]
    fn extra_greet_works() {
        assert_eq!(extra_greet("world"), "Hello again, world!");
    }
}
