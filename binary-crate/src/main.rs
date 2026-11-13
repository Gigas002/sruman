fn main() {
    let name = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "world".to_owned());
    println!("{}", library_crate::greet(&name));

    #[cfg(feature = "extra")]
    println!("{}", library_crate::extra_greet(&name));
}

#[cfg(test)]
mod tests {
    #[test]
    fn main_logic_works() {
        assert_eq!(library_crate::greet("world"), "Hello, world!");
    }
}
