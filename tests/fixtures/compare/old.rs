// Sample Rust file for comparing.
use std::collections::HashMap;

/// A point.
pub struct Point {
    x: f64,
    y: f64,
}

fn main() {
    let mut m: HashMap<&str, i32> = HashMap::new();
    m.insert("answer", 42);
    let total = m.values().sum::<i32>();
    println!("{total}");
    // removed comment
    // another removed line
}
