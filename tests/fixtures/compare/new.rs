// Sample Rust file for comparing.
use std::collections::HashMap;

/// A point in 3D.
pub struct Point {
    x: f64,
    y: f64,
    z: f64,
}

fn main() {
    let mut m: HashMap<&str, i64> = HashMap::new();
    m.insert("answer", 42);
    let total = m.values().sum::<i64>();
    println!("{total}");
}
